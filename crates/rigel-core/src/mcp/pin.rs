//! Roadmap 68: project MCP server pins and schema diffs.
//!
//! A project server's declaration is not its identity. After the first `tools/list`, the server's
//! version, a hash of its tool catalogue and every tool's full schema are stored in
//! `.rigel/mcp-pins.json`. On a later read, a changed catalogue produces a diff; a changed or
//! newly added tool stays blocked until that diff is accepted. Removed tools remain absent, and a
//! malformed pin file is an error, never a silent reason to trust the new list.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Project-relative pin file. It sits next to the approval and network files.
pub const PATH: &str = ".rigel/mcp-pins.json";
const VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum PinError {
    #[error("MCP pin file could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP pin file is not valid JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("MCP pin version {0} is not supported")]
    Version(u32),
    #[error("malformed MCP tool catalogue: {0}")]
    Catalogue(String),
    #[error("MCP pin lock is poisoned")]
    Lock,
}

/// One server's `tools/list` result, with the version its `initialize` response named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub server_version: String,
    /// Canonical registry order: sorted by tool name.
    pub tools: Vec<Value>,
}

impl Catalog {
    pub fn new(server_version: impl Into<String>, tools: Vec<Value>) -> Result<Self, PinError> {
        let mut by_name = BTreeMap::new();
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .ok_or_else(|| PinError::Catalogue("a tool has no name".to_string()))?;
            if by_name.insert(name.clone(), tool).is_some() {
                return Err(PinError::Catalogue(format!(
                    "the server listed {name} more than once"
                )));
            }
        }
        Ok(Self {
            server_version: server_version.into(),
            tools: by_name.into_values().collect(),
        })
    }

    pub fn tool(&self, name: &str) -> Option<&Value> {
        self.tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
    }

    pub fn names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPin {
    pub sha256: String,
    /// The complete tool definition, so a later diff can show what changed rather than only a
    /// hash that disagrees.
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPin {
    pub server_version: String,
    pub hash: String,
    pub pinned_at: i64,
    pub tools: BTreeMap<String, ToolPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PinFile {
    version: u32,
    #[serde(default)]
    servers: BTreeMap<String, ServerPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolDiff {
    pub name: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaDiff {
    pub server: String,
    pub server_version_before: String,
    pub server_version_after: String,
    pub hash_before: String,
    pub hash_after: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<ToolDiff>,
    /// Added and changed tools. These are not callable until the diff is accepted.
    pub blocked_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinObservation {
    /// No pin existed. The caller records it after this first listing.
    FirstUse,
    /// The version, hash and every tool schema match.
    Unchanged,
    /// The catalogue moved. `blocked_tools` is empty for a version-only or removal-only change,
    /// which can be pinned without a schema card; otherwise those tools must stay blocked.
    Changed(Box<SchemaDiff>),
}

impl SchemaDiff {
    pub fn blocks(&self, tool: &str) -> bool {
        self.blocked_tools.iter().any(|name| name == tool)
    }
}

/// A lazily loaded pin file, shared by one MCP process. The lock covers the read-modify-write so
/// two tool calls cannot both accept different catalogues.
#[derive(Clone)]
pub struct PinStore {
    path: PathBuf,
    state: Arc<Mutex<Option<PinFile>>>,
    /// A schema diff accepted at the session scope. It lets this process use the new catalogue
    /// without changing the project pin; the next process asks again.
    accepted_for_session: Arc<Mutex<BTreeMap<String, String>>>,
}

impl std::fmt::Debug for PinStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PinStore {
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        Self {
            path: project_root.as_ref().join(PATH),
            state: Arc::new(Mutex::new(None)),
            accepted_for_session: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Compare a fresh catalogue with the stored pin. This never writes.
    pub fn observe(&self, server: &str, catalog: &Catalog) -> Result<PinObservation, PinError> {
        let file = self.load()?;
        let Some(previous) = file.servers.get(server) else {
            return Ok(PinObservation::FirstUse);
        };
        let next = pin_for(catalog);
        let accepted_here = self
            .accepted_for_session
            .lock()
            .map_err(|_| PinError::Lock)?
            .get(server)
            .is_some_and(|hash| hash == &next.hash);
        if previous.hash == next.hash || accepted_here {
            return Ok(PinObservation::Unchanged);
        }
        Ok(PinObservation::Changed(Box::new(diff(
            server, previous, &next,
        ))))
    }

    /// Accept a diff for this MCP process only. The project pin is left alone, so another process
    /// asks again; this is the `session` scope of the schema card.
    pub fn accept_for_session(&self, server: &str, catalog: &Catalog) -> Result<(), PinError> {
        let hash = pin_for(catalog).hash;
        self.accepted_for_session
            .lock()
            .map_err(|_| PinError::Lock)?
            .insert(server.to_string(), hash);
        Ok(())
    }

    /// Record the current catalogue as the accepted pin. First use and an accepted schema diff
    /// both land here.
    pub fn accept(&self, server: &str, catalog: &Catalog) -> Result<ServerPin, PinError> {
        let mut slot = self.state.lock().map_err(|_| PinError::Lock)?;
        if slot.is_none() {
            *slot = Some(load_file(&self.path)?);
        }
        let file = slot.as_mut().ok_or(PinError::Lock)?;
        let pin = pin_for(catalog);
        let mut next = file.clone();
        next.servers.insert(server.to_string(), pin.clone());
        save_file(&self.path, &next)?;
        *file = next;
        Ok(pin)
    }

    /// The stored pin for one server, for diagnostics and tests.
    pub fn server(&self, server: &str) -> Result<Option<ServerPin>, PinError> {
        Ok(self.load()?.servers.get(server).cloned())
    }

    fn load(&self) -> Result<PinFile, PinError> {
        let mut slot = self.state.lock().map_err(|_| PinError::Lock)?;
        if slot.is_none() {
            *slot = Some(load_file(&self.path)?);
        }
        slot.clone().ok_or(PinError::Lock)
    }
}

fn load_file(path: &Path) -> Result<PinFile, PinError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PinFile {
                version: VERSION,
                servers: BTreeMap::new(),
            });
        }
        Err(error) => return Err(PinError::Io(error)),
    };
    let file: PinFile = serde_json::from_str(&text)?;
    if file.version != VERSION {
        return Err(PinError::Version(file.version));
    }
    Ok(file)
}

fn save_file(path: &Path, file: &PinFile) -> Result<(), PinError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(file)?;
    fs::write(path, text)?;
    Ok(())
}

fn pin_for(catalog: &Catalog) -> ServerPin {
    let tools = catalog
        .tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?.to_string();
            Some((
                name,
                ToolPin {
                    sha256: hash_value(tool),
                    schema: tool.clone(),
                },
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let hash = hash_catalog(&catalog.server_version, &tools);
    ServerPin {
        server_version: catalog.server_version.clone(),
        hash,
        pinned_at: now_ms(),
        tools,
    }
}

fn diff(server: &str, previous: &ServerPin, next: &ServerPin) -> SchemaDiff {
    let old_names = previous.tools.keys().cloned().collect::<BTreeSet<_>>();
    let new_names = next.tools.keys().cloned().collect::<BTreeSet<_>>();
    let added = new_names
        .difference(&old_names)
        .cloned()
        .collect::<Vec<_>>();
    let removed = old_names
        .difference(&new_names)
        .cloned()
        .collect::<Vec<_>>();
    let mut changed = Vec::new();
    for name in old_names.intersection(&new_names) {
        let before = &previous.tools[name];
        let after = &next.tools[name];
        if before.sha256 != after.sha256 {
            changed.push(ToolDiff {
                name: name.clone(),
                before: before.schema.clone(),
                after: after.schema.clone(),
            });
        }
    }
    let mut blocked_tools = added.clone();
    blocked_tools.extend(changed.iter().map(|tool| tool.name.clone()));
    blocked_tools.sort();
    SchemaDiff {
        server: server.to_string(),
        server_version_before: previous.server_version.clone(),
        server_version_after: next.server_version.clone(),
        hash_before: previous.hash.clone(),
        hash_after: next.hash.clone(),
        added,
        removed,
        changed,
        blocked_tools,
    }
}

/// Hash the server version and every tool's canonical definition. Object key order from a
/// server must not create a false diff.
pub fn hash_catalog(server_version: &str, tools: &BTreeMap<String, ToolPin>) -> String {
    let body = tools
        .iter()
        .map(|(name, tool)| (name.clone(), canonical(&tool.schema)))
        .collect::<BTreeMap<_, _>>();
    hash_value(&serde_json::json!({
        "server_version": server_version,
        "tools": body,
    }))
}

pub fn hash_value(value: &Value) -> String {
    let canonical = canonical(value);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

/// Recursively sort object keys so semantically identical JSON hashes identically.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog(version: &str, tools: Vec<Value>) -> Catalog {
        Catalog::new(version, tools).unwrap()
    }

    #[test]
    fn first_use_pins_the_version_and_every_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinStore::new(tmp.path());
        let first = catalog(
            "1",
            vec![json!({
                "name": "echo",
                "description": "Echo one line",
                "inputSchema": {"type": "object"}
            })],
        );
        assert_eq!(
            store.observe("demo", &first).unwrap(),
            PinObservation::FirstUse
        );
        store.accept("demo", &first).unwrap();
        let pin = store.server("demo").unwrap().unwrap();
        assert_eq!(pin.server_version, "1");
        assert_eq!(pin.tools["echo"].schema["description"], "Echo one line");
        assert_eq!(
            store.observe("demo", &first).unwrap(),
            PinObservation::Unchanged
        );
    }

    #[test]
    fn a_changed_schema_blocks_only_the_changed_tool_until_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinStore::new(tmp.path());
        let first = catalog(
            "1",
            vec![
                json!({"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}}),
                json!({"name": "read", "description": "Read", "inputSchema": {"type": "object"}}),
            ],
        );
        store.accept("demo", &first).unwrap();

        let second = catalog(
            "2",
            vec![
                json!({"name": "echo", "description": "Echo changed", "inputSchema": {"type": "object"}}),
                json!({"name": "read", "description": "Read", "inputSchema": {"type": "object"}}),
                json!({"name": "write", "description": "Write", "inputSchema": {"type": "object"}}),
            ],
        );
        let PinObservation::Changed(diff) = store.observe("demo", &second).unwrap() else {
            panic!("the update must produce a schema diff");
        };
        assert_eq!(diff.added, ["write"]);
        assert_eq!(diff.changed[0].name, "echo");
        assert!(diff.blocks("echo"));
        assert!(diff.blocks("write"));
        assert!(!diff.blocks("read"));

        // Reading again before acceptance keeps the old pin and the same block.
        let PinObservation::Changed(again) = store.observe("demo", &second).unwrap() else {
            panic!("the unaccepted change must still be a diff");
        };
        assert!(again.blocks("echo"));

        store.accept("demo", &second).unwrap();
        assert_eq!(
            store.observe("demo", &second).unwrap(),
            PinObservation::Unchanged
        );
    }

    #[test]
    fn a_removal_is_reported_but_blocks_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinStore::new(tmp.path());
        let first = catalog(
            "1",
            vec![
                json!({"name": "echo", "inputSchema": {"type": "object"}}),
                json!({"name": "removed", "inputSchema": {"type": "object"}}),
            ],
        );
        store.accept("demo", &first).unwrap();
        let second = catalog(
            "1",
            vec![json!({"name": "echo", "inputSchema": {"type": "object"}})],
        );
        let PinObservation::Changed(diff) = store.observe("demo", &second).unwrap() else {
            panic!("the removal must produce a diff");
        };
        assert_eq!(diff.removed, ["removed"]);
        assert!(diff.blocked_tools.is_empty());
    }

    #[test]
    fn a_session_acceptance_allows_this_process_without_rewriting_the_project_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let project = PinStore::new(tmp.path());
        let first = catalog(
            "1",
            vec![
                json!({"name": "echo", "description": "before", "inputSchema": {"type": "object"}}),
            ],
        );
        project.accept("demo", &first).unwrap();
        let altered = catalog(
            "1",
            vec![
                json!({"name": "echo", "description": "after", "inputSchema": {"type": "object"}}),
            ],
        );
        assert!(matches!(
            project.observe("demo", &altered).unwrap(),
            PinObservation::Changed(diff) if diff.blocks("echo")
        ));
        project.accept_for_session("demo", &altered).unwrap();
        assert_eq!(
            project.observe("demo", &altered).unwrap(),
            PinObservation::Unchanged
        );
        let reopened = PinStore::new(tmp.path());
        assert!(matches!(
            reopened.observe("demo", &altered).unwrap(),
            PinObservation::Changed(diff) if diff.blocks("echo")
        ));
    }

    #[test]
    fn a_version_only_change_is_a_diff_with_nothing_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let store = PinStore::new(tmp.path());
        let first = catalog(
            "1",
            vec![json!({"name": "echo", "inputSchema": {"type": "object"}})],
        );
        store.accept("demo", &first).unwrap();
        let second = catalog(
            "2",
            vec![json!({"name": "echo", "inputSchema": {"type": "object"}})],
        );
        let PinObservation::Changed(diff) = store.observe("demo", &second).unwrap() else {
            panic!("a version change must be recorded");
        };
        assert_eq!(diff.server_version_before, "1");
        assert_eq!(diff.server_version_after, "2");
        assert!(diff.blocked_tools.is_empty());
    }

    #[test]
    fn hashes_ignore_object_key_order_and_malformed_files_fail_closed() {
        let one = hash_value(&json!({"a": 1, "b": {"c": 2}}));
        let two = hash_value(&json!({"b": {"c": 2}, "a": 1}));
        assert_eq!(one, two);
        assert!(one.starts_with("sha256:"));

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".rigel")).unwrap();
        std::fs::write(tmp.path().join(PATH), "{not json").unwrap();
        let store = PinStore::new(tmp.path());
        assert!(store.load().is_err(), "a malformed pin must not fall open");

        std::fs::write(tmp.path().join(PATH), r#"{"servers":{}}"#).unwrap();
        let store = PinStore::new(tmp.path());
        assert!(
            store.load().is_err(),
            "a pin with no version must not be read as the current format"
        );
    }

    #[test]
    fn duplicate_tool_names_are_refused() {
        let error =
            Catalog::new("1", vec![json!({"name": "echo"}), json!({"name": "echo"})]).unwrap_err();
        assert!(error.to_string().contains("more than once"));
    }
}
