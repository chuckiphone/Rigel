//! A local map of the project: files, the symbols they define, and what connects them.
//!
//! This is deliberately not a compiler. It reads the same project files a checkpoint reads
//! ([`crate::tree::project_files`]), takes the top-level declarations and imports it can see in
//! each language, and resolves relative imports to real files. A symbol body that names another
//! symbol becomes a `references` edge, which is enough to answer "what is connected to this?" in
//! the window without shipping a parser per language.
//!
//! The scan is bounded in every direction: files, bytes per file, nodes and edges. When a bound is
//! hit the graph says so ([`CodeGraph::truncated`]) rather than silently dropping the rest.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::tree;

pub mod watch;

pub const MAX_FILES: usize = 320;
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
pub const MAX_NODES: usize = 900;
pub const MAX_EDGES: usize = 2400;
const MAX_REFS_PER_SYMBOL: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    /// `file`, `symbol`, or `concept` (a free-agent label for a cluster of files).
    pub kind: String,
    /// Project-relative path. Empty for an unresolved import.
    pub path: String,
    pub line: Option<usize>,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// `imports`, `defines` or `references`.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeGraph {
    pub root: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub files: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Symbol {
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) body_start: usize,
    pub(crate) body_end: usize,
}

#[derive(Debug, Clone)]
struct ParsedFile {
    path: String,
    language: String,
    text: String,
    symbols: Vec<Symbol>,
    imports: Vec<String>,
}

/// Build the graph for one project. `focus` keeps the connected neighbourhood around a matching
/// node within `depth` hops; `None` returns the whole bounded graph. `limit` caps the nodes
/// returned after filtering.
pub fn snapshot(
    root: &Path,
    focus: Option<&str>,
    depth: usize,
    limit: usize,
) -> Result<CodeGraph, String> {
    let limit = limit.clamp(100, MAX_NODES);
    let depth = depth.clamp(1, 4);
    let files = tree::project_files(root).map_err(|error| error.to_string())?;
    let mut truncated = false;

    let mut paths = Vec::new();
    for path in files {
        if paths.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        if !is_source(&path) || is_skipped(&path) {
            continue;
        }
        paths.push(path);
    }
    let known: HashSet<String> = paths.iter().cloned().collect();

    let mut parsed = Vec::new();
    for path in &paths {
        let full = root.join(path);
        let Ok(meta) = std::fs::metadata(&full) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            truncated = true;
            continue;
        }
        let Ok(bytes) = std::fs::read(&full) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(language) = language(path) else {
            continue;
        };
        let symbols = symbols(path, &text);
        let imports = imports(path, &text, &known);
        parsed.push(ParsedFile {
            path: path.clone(),
            language: language.to_string(),
            text: text.into_owned(),
            symbols,
            imports,
        });
    }

    let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
    let mut edges: BTreeSet<(String, String, String)> = BTreeSet::new();

    for file in &parsed {
        let file_node = file_id(&file.path);
        nodes.insert(
            file_node.clone(),
            GraphNode {
                id: file_node.clone(),
                label: file.path.clone(),
                kind: "file".into(),
                path: file.path.clone(),
                line: None,
                language: file.language.clone(),
            },
        );
        for import in &file.imports {
            edges.insert((file_node.clone(), file_id(import), "imports".into()));
        }
        let mut seen = HashSet::new();
        for symbol in &file.symbols {
            if !seen.insert(symbol.name.clone()) {
                // A repeated name (an overload, a nested `new`) keeps the first definition.
                continue;
            }
            let id = symbol_id(&file.path, &symbol.name);
            nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    label: symbol.name.clone(),
                    kind: "symbol".into(),
                    path: file.path.clone(),
                    line: Some(symbol.line),
                    language: file.language.clone(),
                },
            );
            edges.insert((file_node.clone(), id, "defines".into()));
        }
    }

    // A symbol that mentions another symbol's name in its own body is a reference. The scan is
    // intentionally shallow and capped: this is a map, not a call graph with types.
    let by_name: HashMap<&str, Vec<&str>> = {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for file in &parsed {
            for symbol in &file.symbols {
                map.entry(symbol.name.as_str())
                    .or_default()
                    .push(&file.path);
            }
        }
        map
    };
    for file in &parsed {
        for symbol in &file.symbols {
            let body = body(&file.text, symbol);
            let mut added = 0;
            for (name, owners) in &by_name {
                if added >= MAX_REFS_PER_SYMBOL {
                    break;
                }
                if *name == symbol.name || name.len() < 3 || is_common(name) {
                    continue;
                }
                if !contains_word(body, name) {
                    continue;
                }
                for owner in owners {
                    let target = symbol_id(owner, name);
                    if target != symbol_id(&file.path, &symbol.name) {
                        edges.insert((
                            symbol_id(&file.path, &symbol.name),
                            target,
                            "references".into(),
                        ));
                        added += 1;
                    }
                }
            }
        }
    }

    // Focus is a breadth-first walk over both directions, so "what connects to this" includes
    // callers and imports as well as what the node itself reaches.
    let focus_id = focus.and_then(|needle| {
        let needle = needle.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return None;
        }
        nodes
            .values()
            .find(|node| node.label.to_ascii_lowercase() == needle)
            .or_else(|| {
                nodes
                    .values()
                    .find(|node| node.label.to_ascii_lowercase().contains(&needle))
            })
            .map(|node| node.id.clone())
    });
    let keep: Option<HashSet<String>> = focus_id.as_ref().map(|seed| {
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for (from, to, _) in &edges {
            adjacency
                .entry(from.as_str())
                .or_default()
                .push(to.as_str());
            adjacency
                .entry(to.as_str())
                .or_default()
                .push(from.as_str());
        }
        let mut keep = HashSet::new();
        let mut queue = VecDeque::new();
        keep.insert(seed.clone());
        queue.push_back((seed.as_str(), 0usize));
        while let Some((id, hops)) = queue.pop_front() {
            if hops >= depth {
                continue;
            }
            for next in adjacency.get(id).into_iter().flatten() {
                if keep.insert((*next).to_string()) {
                    queue.push_back((next, hops + 1));
                }
            }
        }
        keep
    });

    let mut nodes: Vec<GraphNode> = nodes
        .into_values()
        .filter(|node| keep.as_ref().is_none_or(|keep| keep.contains(&node.id)))
        .take(limit)
        .collect();
    if keep.is_some_and(|keep| keep.len() > limit) {
        truncated = true;
    }
    let ids: HashSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let edges: Vec<GraphEdge> = edges
        .into_iter()
        .filter(|(from, to, _)| ids.contains(from.as_str()) && ids.contains(to.as_str()))
        .take(MAX_EDGES)
        .map(|(from, to, kind)| GraphEdge { from, to, kind })
        .collect();
    if edges.len() >= MAX_EDGES {
        truncated = true;
    }
    nodes.sort_by(|a, b| b.kind.cmp(&a.kind).then_with(|| a.label.cmp(&b.label)));

    Ok(CodeGraph {
        root: root.display().to_string(),
        nodes,
        edges,
        files: paths.len(),
        truncated,
    })
}

fn is_source(path: &str) -> bool {
    language(path).is_some()
}

fn is_skipped(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    [
        "target/",
        "node_modules/",
        "vendor/",
        "dist/",
        "build/",
        ".git/",
        ".next/",
        "coverage/",
        "pods/",
        ".venv/",
        "venv/",
        "__pycache__/",
        ".freebuff/",
    ]
    .iter()
    .any(|dir| lower.starts_with(dir) || lower.contains(&format!("/{dir}")))
}

pub(crate) fn language(path: &str) -> Option<&'static str> {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "py" => Some("python"),
        "go" => Some("go"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "java" => Some("java"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Some("cpp"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "vue" | "svelte" => Some("web"),
        _ => None,
    }
}

fn file_id(path: &str) -> String {
    format!("file:{path}")
}

fn symbol_id(path: &str, name: &str) -> String {
    format!("sym:{path}#{name}")
}

fn regex_for(language: &str) -> &'static [(&'static str, Regex)] {
    static RUST: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static TS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static PY: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static GO: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    static OTHER: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();

    match language {
        "rust" => RUST.get_or_init(|| {
            vec![(
                "symbol",
                Regex::new(
                    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(fn|struct|enum|trait|impl|mod|const|static|type|union)\s+([A-Za-z_][A-Za-z0-9_]*)",
                )
                .expect("the Rust symbol pattern"),
            )]
        }),
        "typescript" | "javascript" | "web" => TS.get_or_init(|| {
            vec![(
                "symbol",
                Regex::new(
                    r#"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?(?:function|class|interface|type|enum|const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)"#,
                )
                .expect("the TypeScript symbol pattern"),
            )]
        }),
        "python" => PY.get_or_init(|| {
            vec![(
                "symbol",
                Regex::new(r"^\s*(?:async\s+)?(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)")
                    .expect("the Python symbol pattern"),
            )]
        }),
        "go" => GO.get_or_init(|| {
            vec![(
                "symbol",
                Regex::new(
                    r"^\s*(?:func|type)\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)",
                )
                .expect("the Go symbol pattern"),
            )]
        }),
        _ => OTHER.get_or_init(|| {
            vec![(
                "symbol",
                Regex::new(
                    r"^\s*(?:pub\s+)?(?:public|private|protected|internal|static|final|abstract|override|async|\s)*(?:fn|fun|func|class|struct|interface|enum|trait|record|object)\s+([A-Za-z_][A-Za-z0-9_]*)",
                )
                .expect("the generic symbol pattern"),
            )]
        }),
    }
}

pub(crate) fn symbols(path: &str, text: &str) -> Vec<Symbol> {
    let language = language(path).unwrap_or_default();
    let patterns = regex_for(language);
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        for (kind, pattern) in patterns {
            if let Some(captures) = pattern.captures(line) {
                // A two-group pattern is `kind name`; a one-group pattern is `name`.
                let _ = kind;
                let name = captures
                    .iter()
                    .flatten()
                    .last()
                    .map(|capture| capture.as_str())
                    .unwrap_or_default();
                if name.is_empty() || name.len() > 120 {
                    continue;
                }
                found.push(Symbol {
                    name: name.to_string(),
                    line: index + 1,
                    body_start: index,
                    body_end: lines.len(),
                });
                break;
            }
        }
    }
    for index in 0..found.len() {
        found[index].body_end = found
            .get(index + 1)
            .map(|next| next.body_start)
            .unwrap_or(lines.len());
    }
    found
}

fn body<'a>(text: &'a str, symbol: &Symbol) -> &'a str {
    let lines: Vec<&str> = text.lines().collect();
    let start = symbol.body_start.min(lines.len());
    let end = symbol.body_end.min(lines.len());
    // A slice of the whole text is cheaper than rebuilding it; take the line range.
    let line_slice = &lines[start..end];
    let first = line_slice.first().copied().unwrap_or_default();
    let last = line_slice.last().copied().unwrap_or_default();
    if line_slice.len() <= 1 {
        return first;
    }
    // The returned lifetime is tied to `text`, and these slices are inside it.
    let start_byte = first.as_ptr() as usize - text.as_ptr() as usize;
    let end_byte = last.as_ptr() as usize - text.as_ptr() as usize + last.len();
    &text[start_byte..end_byte]
}

fn imports(path: &str, text: &str, known: &HashSet<String>) -> Vec<String> {
    let language = language(path).unwrap_or_default();
    let mut out = BTreeSet::new();
    match language {
        "rust" => {
            static USE: OnceLock<Regex> = OnceLock::new();
            static MOD: OnceLock<Regex> = OnceLock::new();
            let use_re = USE.get_or_init(|| {
                Regex::new(r"^\s*(?:pub\s+)?use\s+([A-Za-z_][A-Za-z0-9_:]*)")
                    .expect("the Rust use pattern")
            });
            let mod_re = MOD.get_or_init(|| {
                Regex::new(r"^\s*(?:pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
                    .expect("the Rust mod pattern")
            });
            for line in text.lines() {
                if let Some(captures) = use_re.captures(line) {
                    let spec = captures.get(1).unwrap().as_str();
                    if let Some(resolved) = resolve_rust(path, spec, known) {
                        out.insert(resolved);
                    }
                }
                if let Some(captures) = mod_re.captures(line) {
                    let name = captures.get(1).unwrap().as_str();
                    if let Some(resolved) = resolve_rust(path, name, known) {
                        out.insert(resolved);
                    }
                }
            }
        }
        "typescript" | "javascript" | "web" => {
            static FROM: OnceLock<Regex> = OnceLock::new();
            let from = FROM.get_or_init(|| {
                Regex::new(r#"(?:from\s*|require\s*\(\s*)["']([^"']+)["']"#)
                    .expect("the module import pattern")
            });
            for captures in from.captures_iter(text) {
                if let Some(resolved) =
                    resolve_relative(path, captures.get(1).unwrap().as_str(), known)
                {
                    out.insert(resolved);
                }
            }
        }
        "python" => {
            static PY_IMPORT: OnceLock<Regex> = OnceLock::new();
            let pattern = PY_IMPORT.get_or_init(|| {
                Regex::new(r"^\s*(?:from\s+([A-Za-z_][\w.]*)\s+import|import\s+([A-Za-z_][\w.]*))")
                    .expect("the Python import pattern")
            });
            for line in text.lines() {
                if let Some(captures) = pattern.captures(line) {
                    let module = captures
                        .get(1)
                        .or_else(|| captures.get(2))
                        .map(|value| value.as_str())
                        .unwrap_or_default();
                    if let Some(resolved) = resolve_python(module, known) {
                        out.insert(resolved);
                    }
                }
            }
        }
        _ => {}
    }
    out.into_iter().collect()
}

fn resolve_relative(from: &str, spec: &str, known: &HashSet<String>) -> Option<String> {
    if !spec.starts_with('.') {
        return None;
    }
    let base = Path::new(from).parent().unwrap_or(Path::new(""));
    let joined = normalize(&base.join(spec));
    candidates(&joined, known)
}

fn resolve_rust(from: &str, spec: &str, known: &HashSet<String>) -> Option<String> {
    let spec = spec.trim_start_matches("crate::").replace("::", "/");
    let base = Path::new(from).parent().unwrap_or(Path::new(""));
    let parts: Vec<&str> = spec.split('/').filter(|part| !part.is_empty()).collect();
    // `use crate::libmod::Thing` names a symbol, not a file: drop trailing segments until a real
    // module file matches (`src/libmod.rs`).
    for take in (1..=parts.len()).rev() {
        let prefix = parts[..take].join("/");
        let mut candidates = Vec::new();
        if prefix.starts_with("self/") || prefix.starts_with("super/") {
            candidates.push(
                normalize(&base.join(&prefix))
                    .to_string_lossy()
                    .into_owned(),
            );
        } else {
            candidates.push(format!("src/{prefix}"));
            candidates.push(
                normalize(&base.join(&prefix))
                    .to_string_lossy()
                    .into_owned(),
            );
            candidates.push(prefix.clone());
        }
        for candidate in candidates {
            if let Some(found) = file_candidates(&candidate, known) {
                return Some(found);
            }
        }
    }
    None
}

fn resolve_python(module: &str, known: &HashSet<String>) -> Option<String> {
    let path = module.replace('.', "/");
    [format!("{path}.py"), format!("{path}/__init__.py")]
        .into_iter()
        .find(|candidate| known.contains(candidate))
}

fn candidates(joined: &Path, known: &HashSet<String>) -> Option<String> {
    let rendered = normalize(joined).to_string_lossy().replace('\\', "/");
    let rendered = rendered.trim_start_matches("./").to_string();
    file_candidates(&rendered, known)
}

fn file_candidates(base: &str, known: &HashSet<String>) -> Option<String> {
    let base = base.trim_start_matches("./");
    for suffix in [
        "",
        ".rs",
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".py",
        ".go",
        ".vue",
        ".svelte",
        "/index.ts",
        "/index.tsx",
        "/index.js",
        "/mod.rs",
    ] {
        let candidate = format!("{base}{suffix}");
        if known.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn normalize(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub(crate) fn contains_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || needle.len() > bytes.len() {
        return false;
    }
    for at in 0..=bytes.len() - needle.len() {
        if &bytes[at..at + needle.len()] != needle {
            continue;
        }
        let before = at == 0 || !is_word(bytes[at - 1]);
        let after = at + needle.len() == bytes.len() || !is_word(bytes[at + needle.len()]);
        if before && after {
            return true;
        }
    }
    false
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) fn is_common(name: &str) -> bool {
    matches!(
        name,
        "new"
            | "get"
            | "set"
            | "run"
            | "main"
            | "len"
            | "iter"
            | "into"
            | "from"
            | "default"
            | "clone"
            | "fmt"
            | "drop"
            | "next"
            | "build"
            | "open"
            | "close"
            | "read"
            | "write"
            | "send"
            | "load"
            | "save"
            | "show"
            | "update"
            | "with"
            | "value"
            | "error"
            | "test"
            | "parse"
            | "render"
            | "init"
            | "start"
            | "stop"
            | "path"
            | "name"
            | "id"
            | "kind"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/libmod.rs"),
            "pub struct Thing {\n    pub value: u32,\n}\n\npub fn helper(input: u32) -> u32 {\n    input + 1\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "use crate::libmod::Thing;\n\nfn main() {\n    let thing = Thing { value: 1 };\n    println!(\"{}\", helper(thing.value));\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("ui.ts"),
            "export function render() { return 1; }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("README.md"), "# not source\n").unwrap();
        dir
    }

    #[test]
    fn it_finds_files_symbols_and_imports() {
        let dir = project();
        let graph = snapshot(dir.path(), None, 2, 500).unwrap();
        assert!(graph.nodes.iter().any(|node| node.label == "src/main.rs"));
        assert!(graph.nodes.iter().any(|node| node.label == "Thing"));
        assert!(graph.nodes.iter().any(|node| node.label == "helper"));
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == "imports" && edge.to == "file:src/libmod.rs"),
            "the Rust use resolves to the file: {:?}",
            graph.edges
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.kind == "defines" && edge.to.ends_with("#Thing")),
            "the file defines its symbol"
        );
        assert!(!graph.nodes.iter().any(|node| node.label.ends_with(".md")));
        assert_eq!(graph.files, 3);
    }

    #[test]
    fn focus_keeps_the_neighbourhood() {
        let dir = project();
        let graph = snapshot(dir.path(), Some("Thing"), 1, 500).unwrap();
        assert!(graph.nodes.iter().any(|node| node.label == "Thing"));
        assert!(
            graph.nodes.iter().any(|node| node.label == "src/libmod.rs"),
            "the defining file stays"
        );
        assert!(
            !graph.nodes.iter().any(|node| node.label == "ui.ts"),
            "an unrelated file is outside the focus"
        );
    }

    #[test]
    fn a_missing_focus_returns_the_whole_graph() {
        let dir = project();
        let graph = snapshot(dir.path(), Some("no-such-symbol"), 1, 500).unwrap();
        assert!(graph.nodes.len() > 2);
    }

    #[test]
    fn an_empty_or_missing_project_is_an_empty_graph_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let graph = snapshot(dir.path(), None, 2, 500).unwrap();
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }
}
