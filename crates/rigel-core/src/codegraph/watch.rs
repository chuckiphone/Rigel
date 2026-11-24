//! The kept-fresh half of the code graph.
//!
//! [`snapshot`] is a pure read the window can call at any time. This module adds what a surface
//! cannot: a bounded, persistent index the app refreshes when files change, plus optional
//! architecture concepts named by a live free OpenRouter row. Everything is local except that
//! concept pass, whose prompt contains project-relative paths only.
//!
//! The index is written under `.rigel/` (already ignored by the project) as `codegraph.json`.
//! A failed pass records the reason in the status and leaves the previous index in place, so a
//! transient model or filesystem failure never empties the graph.

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{CodeGraph, GraphEdge, GraphNode, MAX_NODES, snapshot};
use crate::agent::catalog;
use crate::agent::freemode;
use crate::agent::model::{ChatMessage, ChatRequest, Delta};
use crate::agent::providers;
use crate::credentials::CredentialStore;
use crate::tree;

/// How many files one concept request may name. The prompt is paths, not contents.
const MAX_PROMPT_FILES: usize = 120;
const MAX_CONCEPTS: usize = 12;
const CONCEPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatus {
    pub project: String,
    /// Unix seconds of the last successful index.
    pub updated_unix: u64,
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    /// A change counter; the window uses it to know the file on disk moved.
    pub revision: u64,
    pub concepts: usize,
    /// The free model that named the concepts, or the reason none did.
    pub agent: Option<String>,
    pub error: Option<String>,
}

impl WatchStatus {
    pub fn age_seconds(&self) -> u64 {
        now_unix().saturating_sub(self.updated_unix)
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub fn graph_path(root: &Path) -> PathBuf {
    root.join(".rigel").join("codegraph.json")
}

pub fn status_path(root: &Path) -> PathBuf {
    root.join(".rigel").join("codegraph-status.json")
}

/// A cheap change detector over the project's source files: path, size and modification time.
/// It reads metadata only, never bodies, and caps at the graph's file bound.
pub fn fingerprint(root: &Path) -> Result<u64, String> {
    let mut files = tree::project_files(root).map_err(|error| error.to_string())?;
    files.sort();
    files.truncate(super::MAX_FILES);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in files {
        if !super::is_source(&path) || super::is_skipped(&path) {
            continue;
        }
        path.hash(&mut hasher);
        if let Ok(metadata) = root.join(&path).metadata() {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(since) = modified.duration_since(UNIX_EPOCH)
            {
                since.as_nanos().hash(&mut hasher);
            }
        }
    }
    Ok(hasher.finish())
}

/// Build and persist one index pass. Concepts from the previous index are kept only when they
/// still point at files this pass contains.
pub fn index(root: &Path) -> Result<CodeGraph, String> {
    let mut graph = snapshot(root, None, 2, MAX_NODES)?;
    if let Some(previous) = read_graph(root) {
        merge_concepts(&mut graph, &previous);
    }
    write_graph(root, &graph)?;
    Ok(graph)
}

/// Merge the concept layer from `source` into `target`, ignoring nodes whose files vanished.
pub fn merge_concepts(target: &mut CodeGraph, source: &CodeGraph) {
    if target.nodes.iter().any(|node| node.kind == "concept") {
        return;
    }
    let files: HashSet<String> = target
        .nodes
        .iter()
        .filter(|node| node.kind == "file")
        .map(|node| node.id.clone())
        .collect();
    let mut added = 0;
    for node in source.nodes.iter().filter(|node| node.kind == "concept") {
        let edges: Vec<&GraphEdge> = source
            .edges
            .iter()
            .filter(|edge| edge.from == node.id && files.contains(edge.to.as_str()))
            .collect();
        if edges.is_empty() || added >= MAX_CONCEPTS {
            continue;
        }
        target.nodes.push(node.clone());
        target.edges.extend(edges.into_iter().cloned());
        added += 1;
    }
}

pub fn read_graph(root: &Path) -> Option<CodeGraph> {
    let text = std::fs::read_to_string(graph_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_graph(root: &Path, graph: &CodeGraph) -> Result<(), String> {
    let directory = root.join(".rigel");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create .rigel: {error}"))?;
    let text = serde_json::to_string(graph).map_err(|error| error.to_string())?;
    let temporary = directory.join("codegraph.json.tmp");
    std::fs::write(&temporary, text)
        .map_err(|error| format!("could not write the index: {error}"))?;
    let destination = graph_path(root);
    let _ = std::fs::remove_file(&destination);
    std::fs::rename(&temporary, &destination)
        .map_err(|error| format!("could not replace the index: {error}"))
}

pub fn write_status(root: &Path, status: &WatchStatus) {
    let Ok(text) = serde_json::to_string(status) else {
        return;
    };
    let directory = root.join(".rigel");
    let _ = std::fs::create_dir_all(&directory);
    let temporary = directory.join("codegraph-status.json.tmp");
    if std::fs::write(&temporary, text).is_ok() {
        let _ = std::fs::remove_file(status_path(root));
        let _ = std::fs::rename(temporary, status_path(root));
    }
}

pub fn read_status(root: &Path) -> Option<WatchStatus> {
    let text = std::fs::read_to_string(status_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Debug, Deserialize)]
struct Draft {
    name: String,
    #[serde(default)]
    files: Vec<String>,
}

/// Ask one live zero-priced OpenRouter row to name architecture concepts for the indexed files.
/// The model sees project-relative paths and at most one symbol per file, never file bodies or
/// credentials. Only concepts that point at files this graph actually contains are returned.
pub async fn concepts(
    root: &Path,
    graph: &CodeGraph,
    store: &dyn CredentialStore,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>, String), String> {
    let rows = catalog::roster("openrouter", store).await?;
    let option = freemode::candidates("openrouter", rows)
        .into_iter()
        .find(|row| row.provider == "openrouter" && row.free && row.chat)
        .ok_or_else(|| "OpenRouter lists no live free chat model".to_string())?;
    let model = providers::build(&option.provider, store).await?;

    let mut symbols: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in graph.nodes.iter().filter(|node| node.kind == "symbol") {
        symbols
            .entry(node.path.as_str())
            .or_default()
            .push(node.label.as_str());
    }
    let mut paths: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "file")
        .map(|node| node.path.as_str())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let listing = paths
        .iter()
        .take(MAX_PROMPT_FILES)
        .map(|path| {
            let names = symbols
                .get(path)
                .map(|names| names.join(", "))
                .unwrap_or_default();
            if names.is_empty() {
                (*path).to_string()
            } else {
                format!("{path}\t{names}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let request = ChatRequest {
        model: option.id.clone(),
        messages: vec![
            ChatMessage::system(
                "You index a software repository for a local code-graph viewer. The user message \
                 is one tab-separated file path and optional top-level symbol names per line. \
                 Name at most 8 architecture concepts (subsystems, layers, flows) that explain how \
                 these files fit together. Return only a JSON array of objects shaped \
                 {\"name\":\"short concept name\",\"files\":[\"exact path from the list\"]}. Every \
                 path must be copied exactly from the list; omit a concept that has no list path. \
                 No prose, no markdown.",
            ),
            ChatMessage::user(listing),
        ],
        tools: Vec::new(),
        max_tokens: Some(900),
        temperature: 0.0,
        effort: None,
    };
    let cancel = CancellationToken::new();
    let mut answer = String::new();
    let mut sink = |delta: Delta| {
        if let Delta::Text(text) = delta {
            answer.push_str(&text);
        }
    };
    tokio::time::timeout(CONCEPT_TIMEOUT, model.chat(request, &cancel, &mut sink))
        .await
        .map_err(|_| format!("{} did not answer in time", option.id))?
        .map_err(|error| format!("{}: {error}", option.id))?;

    let start = answer
        .find('[')
        .ok_or_else(|| "the free model returned no JSON array".to_string())?;
    let end = answer
        .rfind(']')
        .ok_or_else(|| "the free model returned an unterminated array".to_string())?;
    let drafts: Vec<Draft> = serde_json::from_str(&answer[start..=end])
        .map_err(|error| format!("the free model returned unreadable JSON: {error}"))?;

    let files: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "file")
        .map(|node| node.path.as_str())
        .collect();
    let mut names = HashSet::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for draft in drafts.into_iter().take(MAX_CONCEPTS) {
        let name = draft.name.trim();
        if name.len() < 2 || name.len() > 60 {
            continue;
        }
        let id = format!("concept:{}", slug(name));
        if !names.insert(id.clone()) {
            continue;
        }
        let linked: Vec<&str> = draft
            .files
            .iter()
            .map(|path| path.trim())
            .filter(|path| files.contains(path))
            .take(8)
            .collect();
        if linked.is_empty() {
            continue;
        }
        nodes.push(GraphNode {
            id: id.clone(),
            label: name.to_string(),
            kind: "concept".to_string(),
            path: linked[0].to_string(),
            line: None,
            language: "concept".to_string(),
        });
        for path in linked {
            let file_id = graph
                .nodes
                .iter()
                .find(|node| node.kind == "file" && node.path == path)
                .map(|node| node.id.clone());
            if let Some(file_id) = file_id {
                edges.push(GraphEdge {
                    from: id.clone(),
                    to: file_id,
                    kind: "references".to_string(),
                });
            }
        }
    }
    if nodes.is_empty() {
        return Err(format!("{} named no usable concepts", option.id));
    }
    let _ = root;
    Ok((nodes, edges, option.id))
}

/// Replace the concept layer in `graph`. Deterministic nodes are untouched.
pub fn apply_concepts(graph: &mut CodeGraph, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) {
    let ids: HashSet<String> = nodes.iter().map(|node| node.id.clone()).collect();
    graph.nodes.retain(|node| node.kind != "concept");
    graph
        .edges
        .retain(|edge| !ids.contains(&edge.from) && !ids.contains(&edge.to));
    graph.nodes.extend(nodes);
    graph.edges.extend(edges);
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, path: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            label: id.to_string(),
            kind: kind.to_string(),
            path: path.to_string(),
            line: None,
            language: "rust".to_string(),
        }
    }

    #[test]
    fn a_concept_without_a_live_file_is_dropped() {
        let mut target = CodeGraph {
            root: ".".to_string(),
            nodes: vec![node("file:a.rs", "file", "a.rs")],
            edges: Vec::new(),
            files: 1,
            truncated: false,
        };
        let source = CodeGraph {
            root: ".".to_string(),
            nodes: vec![
                node("concept:core", "concept", "a.rs"),
                node("concept:gone", "concept", "gone.rs"),
            ],
            edges: vec![
                GraphEdge {
                    from: "concept:core".to_string(),
                    to: "file:a.rs".to_string(),
                    kind: "references".to_string(),
                },
                GraphEdge {
                    from: "concept:gone".to_string(),
                    to: "file:gone.rs".to_string(),
                    kind: "references".to_string(),
                },
            ],
            files: 2,
            truncated: false,
        };
        merge_concepts(&mut target, &source);
        assert!(target.nodes.iter().any(|node| node.id == "concept:core"));
        assert!(!target.nodes.iter().any(|node| node.id == "concept:gone"));
        assert_eq!(target.edges.len(), 1);
    }

    #[test]
    fn applying_a_new_layer_replaces_the_old_one() {
        let mut graph = CodeGraph {
            root: ".".to_string(),
            nodes: vec![
                node("file:a.rs", "file", "a.rs"),
                node("concept:old", "concept", "a.rs"),
            ],
            edges: vec![GraphEdge {
                from: "concept:old".to_string(),
                to: "file:a.rs".to_string(),
                kind: "references".to_string(),
            }],
            files: 1,
            truncated: false,
        };
        apply_concepts(
            &mut graph,
            vec![node("concept:new", "concept", "a.rs")],
            vec![GraphEdge {
                from: "concept:new".to_string(),
                to: "file:a.rs".to_string(),
                kind: "references".to_string(),
            }],
        );
        assert!(!graph.nodes.iter().any(|node| node.id == "concept:old"));
        assert!(graph.nodes.iter().any(|node| node.id == "concept:new"));
    }

    #[test]
    fn a_slug_is_stable_and_bounded() {
        assert_eq!(slug("Turn pipeline"), "turn-pipeline");
        assert_eq!(slug("  UI / IPC  "), "ui-ipc");
    }
}
