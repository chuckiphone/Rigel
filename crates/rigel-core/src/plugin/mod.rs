//! Plugins: a manifest, a capability grant, and a process that speaks one small protocol.
//!
//! A plugin is a directory with a `plugin.json` and an entry command. Rigel never imports its
//! code and never gives it the project: the host starts it inside the plugin directory, scrubs
//! the environment, and answers only the capabilities the manifest declares. A tool call is one
//! short-lived process, so a crashed or hanging plugin cannot keep a turn's file handles or
//! sockets alive.
//!
//! The protocol is newline-delimited JSON on stdin/stdout (protocol version 1):
//!
//! ```text
//! host → {"type":"hello","protocol":1,"plugin":"name"}
//! plug → {"type":"ready","protocol":1}
//! host → {"type":"call","id":1,"tool":"tool_name","arguments":{…}}
//! plug → {"type":"capability","id":2,"capability":"fs.read","path":"notes.txt"}
//! host → {"type":"capability_result","id":2,"ok":true,"content":"…"}
//! plug → {"type":"result","id":1,"ok":true,"content":{…}}
//! ```
//!
//! **What the boundary proves and what it does not.** Every `fs.read`/`fs.write` the plugin asks
//! the host to perform is checked against the manifest's roots, and a request outside them is
//! refused before any file is opened. That is the fixture in `tests/plugins.rs`. A subprocess
//! that calls the operating system directly is outside this broker: the manifest's filesystem
//! grant is enforced by the host, not by an OS sandbox, and `network`/`process` are declarations
//! shown in review rather than intercepted calls. Use [`crate::sandbox`] for OS-level containment
//! where the platform supports it.
//!
//! The catalogue is a curated list shipped with the binary and read from a local file when the
//! user has one. Entries are pinned by name, repository and exact commit; the marketplace never
//! installs a branch.

pub mod host;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::migrate::now_ms;
pub use host::{Broker, CapabilityError, PluginHost, PluginReply, PluginTransport, StdioTransport};

pub const MANIFEST_FILE: &str = "plugin.json";
pub const MANIFEST_SCHEMA: u32 = 1;
pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_PLUGIN_FILES: usize = 2_000;
pub const MAX_PLUGIN_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_TOOL_DESCRIPTION: usize = 400;
pub const MAX_REPLY_CHARS: usize = 24_000;

/// The curated catalogue, compiled in. `rigel plugins browse` always works offline; a newer
/// catalogue can be read from `<index>/plugins/catalog.json` without a release.
pub const CATALOG_JSON: &str = include_str!("catalog.json");

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("plugin JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid plugin manifest: {0}")]
    Manifest(String),

    #[error("plugin {0} is not installed")]
    NotInstalled(String),

    #[error("plugin {name} is broken: {reason}")]
    Broken { name: String, reason: String },

    #[error("plugin {0} is already installed; pass --force to replace it")]
    AlreadyInstalled(String),

    #[error("plugin {0} is disabled")]
    Disabled(String),

    #[error("plugin {0} has no tool named {1}")]
    UnknownTool(String, String),

    #[error("plugin {0} did not answer within {1:?}")]
    Timeout(String, Duration),

    #[error("plugin protocol error: {0}")]
    Protocol(String),

    #[error("no catalogue entry named {0}")]
    UnknownCatalogEntry(String),

    #[error("catalogue entry {0} cannot be installed automatically: {1}")]
    NotInstallable(String, String),

    #[error("git is required to install a pinned plugin: {0}")]
    Git(String),

    #[error("a pinned install needs a full 40-character commit SHA, not {0:?}")]
    Unpinned(String),
}

impl PluginError {
    /// §7.2: `1` is a caller error, `2` is a failed precondition.
    pub fn exit_code(&self) -> u8 {
        match self {
            PluginError::UnknownTool(..)
            | PluginError::UnknownCatalogEntry(_)
            | PluginError::Manifest(_)
            | PluginError::Unpinned(_) => 1,
            _ => 2,
        }
    }
}

/* --------------------------------- manifest -------------------------------- */

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub entry: Option<PluginEntry>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginTool {
    pub name: String,
    pub description: String,
    #[serde(default = "empty_schema")]
    pub parameters: Value,
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub filesystem: Filesystem,
    /// Declared, shown in review, and never inferred from a tool description.
    #[serde(default)]
    pub network: bool,
    /// Declared for review. The host does not intercept a plugin's own child processes.
    #[serde(default)]
    pub process: bool,
    /// Extra environment variables, by name, passed to the child. Credentials are never added
    /// implicitly: the base set below carries paths and locale, not keys.
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Filesystem {
    /// Relative to the plugin directory, or absolute. `..` is refused at validation time.
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

impl Capabilities {
    /// A short, stable summary for a CLI row or a marketplace card.
    pub fn summary(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if !self.filesystem.read.is_empty() {
            parts.push(format!("read: {}", self.filesystem.read.join(", ")));
        }
        if !self.filesystem.write.is_empty() {
            parts.push(format!("write: {}", self.filesystem.write.join(", ")));
        }
        if self.network {
            parts.push("network".to_string());
        }
        if self.process {
            parts.push("subprocess".to_string());
        }
        if !self.env.is_empty() {
            parts.push(format!("env: {}", self.env.join(", ")));
        }
        if parts.is_empty() {
            parts.push("none".to_string());
        }
        parts
    }
}

impl PluginManifest {
    /// Read and validate one manifest. JSON, not YAML: one parser, one error shape, no
    /// indentation-sensitive security policy.
    pub fn load(dir: &Path) -> Result<Self, PluginError> {
        let path = dir.join(MANIFEST_FILE);
        let text = fs::read_to_string(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PluginError::Manifest(format!("{} has no {MANIFEST_FILE}", dir.display()))
            } else {
                PluginError::Io(error)
            }
        })?;
        let manifest: Self = serde_json::from_str(&text)
            .map_err(|error| PluginError::Manifest(format!("{}: {error}", path.display())))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn parse(text: &str) -> Result<Self, PluginError> {
        let manifest: Self =
            serde_json::from_str(text).map_err(|error| PluginError::Manifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Everything a manifest can get wrong, checked before a byte is copied or a process is
    /// started. Unknown fields are refused by serde; this covers the values.
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(PluginError::Manifest(format!(
                "schema {} is not supported; this build reads schema {MANIFEST_SCHEMA}",
                self.schema
            )));
        }
        validate_name("plugin", &self.name)?;
        if self.version.trim().is_empty() || self.version.len() > 64 {
            return Err(PluginError::Manifest(
                "version must be 1–64 characters".to_string(),
            ));
        }
        if !self
            .version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
        {
            return Err(PluginError::Manifest(
                "version may contain letters, digits, '.', '-', '+' and '_'".to_string(),
            ));
        }
        if self.description.chars().count() > 500 {
            return Err(PluginError::Manifest(
                "description is capped at 500 characters".to_string(),
            ));
        }
        if !self.tools.is_empty() && self.entry.is_none() {
            return Err(PluginError::Manifest(
                "a plugin with tools needs an entry command".to_string(),
            ));
        }
        if let Some(entry) = &self.entry {
            if entry.command.trim().is_empty() {
                return Err(PluginError::Manifest(
                    "entry.command must not be empty".to_string(),
                ));
            }
            if entry.command.contains('\0') || entry.args.iter().any(|arg| arg.contains('\0')) {
                return Err(PluginError::Manifest(
                    "entry command and args must not contain NUL".to_string(),
                ));
            }
        }

        let mut names = BTreeSet::new();
        for tool in &self.tools {
            validate_name("tool", &tool.name)?;
            if !names.insert(tool.name.clone()) {
                return Err(PluginError::Manifest(format!(
                    "tool {} is declared twice",
                    tool.name
                )));
            }
            if tool.description.trim().is_empty() {
                return Err(PluginError::Manifest(format!(
                    "tool {} needs a description",
                    tool.name
                )));
            }
            if tool.description.chars().count() > MAX_TOOL_DESCRIPTION {
                return Err(PluginError::Manifest(format!(
                    "tool {} has a description over {MAX_TOOL_DESCRIPTION} characters",
                    tool.name
                )));
            }
            if !tool.parameters.is_object() {
                return Err(PluginError::Manifest(format!(
                    "tool {} parameters must be a JSON schema object",
                    tool.name
                )));
            }
        }

        for root in self
            .capabilities
            .filesystem
            .read
            .iter()
            .chain(self.capabilities.filesystem.write.iter())
        {
            validate_root(root)?;
        }
        for key in &self.capabilities.env {
            if key.is_empty()
                || !key
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(PluginError::Manifest(format!(
                    "env name {key:?} is not a portable environment variable"
                )));
            }
        }
        Ok(())
    }

    pub fn tool(&self, name: &str) -> Option<&PluginTool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// The model-facing names. Prefixing keeps a plugin from shadowing a core tool and makes a
    /// transcript line say which plugin answered.
    pub fn qualified_tools(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| format!("{}.{}", self.name, tool.name))
            .collect()
    }
}

fn validate_name(kind: &str, name: &str) -> Result<(), PluginError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(PluginError::Manifest(format!(
            "{kind} name {name:?} must be 1–64 ASCII letters, digits, '-' or '_', starting with a letter or digit"
        )))
    }
}

fn validate_root(root: &str) -> Result<(), PluginError> {
    if root.trim().is_empty() {
        return Err(PluginError::Manifest(
            "a filesystem root must not be empty".to_string(),
        ));
    }
    if root.contains('\0') {
        return Err(PluginError::Manifest(
            "a filesystem root must not contain NUL".to_string(),
        ));
    }
    let path = Path::new(root);
    if path
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(PluginError::Manifest(format!(
            "filesystem root {root:?} contains '..'; name the directory it resolves to"
        )));
    }
    Ok(())
}

/* ---------------------------------- store --------------------------------- */

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Receipt {
    pub version: String,
    pub source: String,
    #[serde(default)]
    pub sha256: Option<String>,
    /// The exact `plugin.json` that was reviewed. A changed manifest is refused at load, so a
    /// plugin that can write cannot grant itself a wider capability set.
    #[serde(default)]
    pub manifest_sha: Option<String>,
    #[serde(default)]
    pub pinned_sha: Option<String>,
    pub installed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    receipts: BTreeMap<String, Receipt>,
}

#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub directory: PathBuf,
    pub enabled: bool,
    pub receipt: Option<Receipt>,
}

/// Every readable plugin and every directory that failed to load, by name.
pub type PluginScan = (Vec<InstalledPlugin>, Vec<(String, String)>);

#[derive(Debug, Clone, Serialize)]
pub struct PluginRow {
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub tools: Vec<String>,
    pub capabilities: Vec<String>,
    pub source: Option<String>,
    pub license: Option<String>,
    pub installed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub name: String,
    pub version: String,
    pub directory: PathBuf,
    pub tools: Vec<String>,
    pub capabilities: Vec<String>,
    pub digest: String,
    pub replaced: bool,
}

/// `<index>/plugins`, with one directory per plugin and a `state.json` for enablement and
/// receipts. Directories are the authority on what is installed; the state file only records
/// what the user turned off and where it came from, so deleting it never deletes a plugin.
#[derive(Debug, Clone)]
pub struct PluginStore {
    root: PathBuf,
}

impl PluginStore {
    pub fn new(index_dir: &Path) -> Self {
        Self {
            root: index_dir.join("plugins"),
        }
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn load_state(&self) -> Result<State, PluginError> {
        match fs::read_to_string(self.state_path()) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                PluginError::Manifest(format!("plugins/state.json is unreadable: {error}"))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(PluginError::Io(error)),
        }
    }

    fn save_state(&self, state: &State) -> Result<(), PluginError> {
        fs::create_dir_all(&self.root)?;
        let path = self.state_path();
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    /// Every directory with a readable manifest, plus the directories that look like plugins but
    /// are not valid. A broken plugin is reported, not silently omitted: an install that
    /// vanished is a support question, not an empty list.
    pub fn scan(&self) -> Result<PluginScan, PluginError> {
        let state = self.load_state()?;
        let mut plugins = Vec::new();
        let mut broken = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((plugins, broken));
            }
            Err(error) => return Err(PluginError::Io(error)),
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.path())
            .filter(|path| {
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
            })
            .collect();
        paths.sort();
        for dir in paths {
            let shown = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_string();
            match PluginManifest::load(&dir) {
                Ok(manifest) => {
                    let receipt = state.receipts.get(&manifest.name).cloned();
                    let current = manifest_digest(&dir).ok();
                    if let Some(expected) =
                        receipt.as_ref().and_then(|row| row.manifest_sha.as_deref())
                        && current.as_deref() != Some(expected)
                    {
                        broken.push((
                            shown,
                            "plugin.json changed since install; reinstall to review the new capabilities"
                                .to_string(),
                        ));
                        continue;
                    }
                    let enabled = !state.disabled.iter().any(|name| name == &manifest.name);
                    plugins.push(InstalledPlugin {
                        manifest,
                        directory: dir,
                        enabled,
                        receipt,
                    });
                }
                Err(error) => broken.push((shown, error.to_string())),
            }
        }
        Ok((plugins, broken))
    }

    pub fn list(&self) -> Result<Vec<InstalledPlugin>, PluginError> {
        Ok(self.scan()?.0)
    }

    pub fn rows(&self) -> Result<Vec<PluginRow>, PluginError> {
        let (plugins, _) = self.scan()?;
        Ok(plugins
            .into_iter()
            .map(|plugin| PluginRow {
                name: plugin.manifest.name.clone(),
                version: plugin.manifest.version.clone(),
                description: plugin.manifest.description.clone(),
                enabled: plugin.enabled,
                tools: plugin.manifest.qualified_tools(),
                capabilities: plugin.manifest.capabilities.summary(),
                source: plugin.receipt.as_ref().map(|row| row.source.clone()),
                license: plugin.manifest.license.clone(),
                installed_at: plugin.receipt.as_ref().map(|row| row.installed_at),
            })
            .collect())
    }

    pub fn get(&self, name: &str) -> Result<InstalledPlugin, PluginError> {
        let (plugins, broken) = self.scan()?;
        if let Some(plugin) = plugins
            .into_iter()
            .find(|plugin| plugin.manifest.name == name)
        {
            return Ok(plugin);
        }
        if let Some((_, reason)) = broken.into_iter().find(|(shown, _)| shown == name) {
            return Err(PluginError::Broken {
                name: name.to_string(),
                reason,
            });
        }
        Err(PluginError::NotInstalled(name.to_string()))
    }

    pub fn enabled(&self, name: &str) -> Result<InstalledPlugin, PluginError> {
        let plugin = self.get(name)?;
        if plugin.enabled {
            Ok(plugin)
        } else {
            Err(PluginError::Disabled(name.to_string()))
        }
    }

    /// Copy a directory into the store. The manifest is validated before the copy, the copy is
    /// capped and refuses symlinks, and the receipt is written only after the tree is complete.
    pub fn install_local(&self, source: &Path, force: bool) -> Result<InstallReport, PluginError> {
        let manifest = PluginManifest::load(source)?;
        let destination = self.root.join(&manifest.name);
        let replaced = destination.exists();
        if replaced && !force {
            return Err(PluginError::AlreadyInstalled(manifest.name.clone()));
        }
        let staging = self
            .root
            .join(format!(".install-{}-{}", manifest.name, now_ms()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        if let Err(error) = copy_tree(source, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        // The copied manifest is the one that will run, not the source's.
        let copied = PluginManifest::load(&staging)?;
        if copied != manifest {
            let _ = fs::remove_dir_all(&staging);
            return Err(PluginError::Manifest(
                "the manifest changed while it was being copied".to_string(),
            ));
        }
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(&staging, &destination)?;
        let digest = tree_digest(&destination)?;

        let mut state = self.load_state()?;
        state.disabled.retain(|name| name != &manifest.name);
        state.receipts.insert(
            manifest.name.clone(),
            Receipt {
                version: manifest.version.clone(),
                source: source.display().to_string(),
                sha256: Some(digest.clone()),
                manifest_sha: manifest_digest(&destination).ok(),
                pinned_sha: None,
                installed_at: now_ms(),
            },
        );
        self.save_state(&state)?;

        let tools = manifest.qualified_tools();
        let capabilities = manifest.capabilities.summary();
        Ok(InstallReport {
            name: manifest.name,
            version: manifest.version,
            directory: destination,
            tools,
            capabilities,
            digest,
            replaced,
        })
    }

    /// Clone a repository at an exact commit, validate its manifest, then copy the tree.
    /// Branches, tags and short SHAs are refused: a marketplace listing is a review of a tree,
    /// and "main today" is a different tree tomorrow.
    pub fn install_git(
        &self,
        repo: &str,
        sha: &str,
        subdir: Option<&str>,
        force: bool,
    ) -> Result<InstallReport, PluginError> {
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PluginError::Unpinned(sha.to_string()));
        }
        fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(".git-{}", now_ms()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let git = |args: &[&str], cwd: Option<&Path>| -> Result<String, PluginError> {
            let mut command = std::process::Command::new("git");
            command.args(args).env("GIT_TERMINAL_PROMPT", "0");
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
            crate::process::hide(&mut command);
            let output = command.output().map_err(|error| {
                PluginError::Git(format!("could not start git ({}): {error}", args.join(" ")))
            })?;
            if !output.status.success() {
                return Err(PluginError::Git(format!(
                    "git {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        };
        let result = (|| {
            git(
                &["clone", "--quiet", "--no-checkout", repo, "."],
                Some(&staging),
            )?;
            git(
                &["fetch", "--quiet", "--depth", "1", "origin", sha],
                Some(&staging),
            )?;
            git(&["checkout", "--quiet", sha], Some(&staging))?;
            let head = git(&["rev-parse", "HEAD"], Some(&staging))?;
            if !head.eq_ignore_ascii_case(sha) {
                return Err(PluginError::Git(format!(
                    "the checkout is {head}, not the pinned {sha}"
                )));
            }
            let source = match subdir.filter(|value| !value.is_empty()) {
                Some(subdir) => staging.join(subdir),
                None => staging.clone(),
            };
            self.install_local(&source, force)
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }

    pub fn remove(&self, name: &str) -> Result<bool, PluginError> {
        let (plugins, broken) = self.scan()?;
        let healthy = plugins
            .into_iter()
            .find(|plugin| plugin.manifest.name == name);
        let broken_here = broken.iter().any(|(shown, _)| shown == name);
        let manifest_name = healthy.as_ref().map(|plugin| plugin.manifest.name.clone());
        let mut state = self.load_state()?;
        let receipt_here = state.receipts.contains_key(name)
            || manifest_name
                .as_ref()
                .is_some_and(|manifest| state.receipts.contains_key(manifest));

        // A broken install has no usable manifest, so its directory name is the only handle the
        // list could have shown. Remove it only when it is a direct, non-symlinked child that the
        // scan itself reported broken; an arbitrary sibling directory is never a plugin.
        let directory = healthy.map(|plugin| plugin.directory).or_else(|| {
            (broken_here)
                .then(|| child_path(&self.root, name))
                .flatten()
                .filter(|path| path.is_dir())
        });
        if directory.is_none() && !receipt_here && !broken_here {
            return Ok(false);
        }
        if let Some(directory) = &directory {
            let metadata = fs::symlink_metadata(directory)?;
            if metadata.file_type().is_symlink() {
                return Err(PluginError::Manifest(format!(
                    "refusing to remove symlinked plugin directory {}",
                    directory.display()
                )));
            }
            fs::remove_dir_all(directory)?;
        }
        state.disabled.retain(|entry| entry != name);
        state.receipts.remove(name);
        if let Some(manifest) = manifest_name {
            state.disabled.retain(|entry| entry != &manifest);
            state.receipts.remove(&manifest);
        }
        self.save_state(&state)?;
        Ok(true)
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<PluginRow, PluginError> {
        let plugin = self.get(name)?;
        let mut state = self.load_state()?;
        state.disabled.retain(|entry| entry != name);
        if !enabled {
            state.disabled.push(name.to_string());
        }
        self.save_state(&state)?;
        Ok(PluginRow {
            name: plugin.manifest.name.clone(),
            version: plugin.manifest.version.clone(),
            description: plugin.manifest.description.clone(),
            enabled,
            tools: plugin.manifest.qualified_tools(),
            capabilities: plugin.manifest.capabilities.summary(),
            source: plugin.receipt.as_ref().map(|row| row.source.clone()),
            license: plugin.manifest.license.clone(),
            installed_at: plugin.receipt.as_ref().map(|row| row.installed_at),
        })
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), PluginError> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    copy_tree_inner(source, destination, &mut files, &mut bytes)
}

fn copy_tree_inner(
    source: &Path,
    destination: &Path,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), PluginError> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = destination.join(&name);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.file_type().is_symlink() {
            return Err(PluginError::Manifest(format!(
                "{} is a symlink; a plugin tree must be plain files so a receipt covers what it runs",
                from.display()
            )));
        }
        if metadata.is_dir() {
            fs::create_dir_all(&to)?;
            copy_tree_inner(&from, &to, files, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        *files += 1;
        *bytes = bytes.saturating_add(metadata.len());
        if *files > MAX_PLUGIN_FILES {
            return Err(PluginError::Manifest(format!(
                "plugin has more than {MAX_PLUGIN_FILES} files"
            )));
        }
        if *bytes > MAX_PLUGIN_BYTES {
            return Err(PluginError::Manifest(format!(
                "plugin is larger than {} MB",
                MAX_PLUGIN_BYTES / 1024 / 1024
            )));
        }
        fs::copy(&from, &to)?;
    }
    Ok(())
}

/// SHA-256 over sorted `path\0sha256\0` rows. Stable across platforms and independent of file
/// timestamps, so a receipt can be compared to a published one.
pub fn tree_digest(root: &Path) -> Result<String, PluginError> {
    let mut rows = Vec::new();
    collect_digest_rows(root, root, &mut rows)?;
    rows.sort();
    let mut hasher = Sha256::new();
    for row in rows {
        hasher.update(row.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// SHA-256 of the manifest file alone, recorded so a later load can prove the capability
/// grant is still the one that was reviewed.
/// One normal path component below `root`, or `None` for `..`, an absolute path, a drive prefix
/// or a name with a separator. Removal must never be able to walk out of the plugin store.
fn child_path(root: &Path, name: &str) -> Option<PathBuf> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(part)), None) if part == std::ffi::OsStr::new(name) => {
            Some(root.join(name))
        }
        _ => None,
    }
}

fn manifest_digest(dir: &Path) -> Result<String, PluginError> {
    let bytes = fs::read(dir.join(MANIFEST_FILE))?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn collect_digest_rows(root: &Path, dir: &Path, rows: &mut Vec<String>) -> Result<(), PluginError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_digest_rows(root, &path, rows)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path)?;
        rows.push(format!("{relative}\0{:x}\0", Sha256::digest(&bytes)));
    }
    Ok(())
}

/* --------------------------------- runtime -------------------------------- */

/// The enabled plugins this machine has, and the one call path the agent's `plugin_call` tool
/// uses. Scanning is filesystem only; no plugin is started until a tool is actually called.
#[derive(Debug, Clone)]
pub struct PluginRuntime {
    store: PluginStore,
}

impl PluginRuntime {
    pub fn open(index_dir: &Path) -> Self {
        Self {
            store: PluginStore::new(index_dir),
        }
    }

    pub fn from_store(store: PluginStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &PluginStore {
        &self.store
    }

    /// Enabled manifests only. A broken install is omitted from the model's view and remains
    /// visible in `rigel plugins list`.
    pub fn enabled(&self) -> Vec<InstalledPlugin> {
        self.store
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|plugin| plugin.enabled)
            .collect()
    }

    /// One line per enabled tool, for the `plugin_call` description. Kept short because it is
    /// billed on every turn.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for plugin in self.enabled() {
            for tool in &plugin.manifest.tools {
                parts.push(format!("{}.{}", plugin.manifest.name, tool.name));
            }
        }
        if parts.is_empty() {
            "No plugins are installed. Run `rigel plugins browse`.".to_string()
        } else {
            format!("Installed tools: {}", parts.join(", "))
        }
    }

    pub fn find_tool(&self, qualified: &str) -> Option<(InstalledPlugin, PluginTool)> {
        let (plugin_name, tool_name) = qualified.split_once('.')?;
        let plugin = self.store.enabled(plugin_name).ok()?;
        let tool = plugin.manifest.tool(tool_name)?.clone();
        Some((plugin, tool))
    }

    /// Run one tool. The tool name is either `plugin.tool` or a bare tool when exactly one
    /// enabled plugin declares it.
    pub async fn call(
        &self,
        plugin: Option<&str>,
        tool: &str,
        arguments: Value,
    ) -> Result<PluginReply, PluginError> {
        let (plugin, tool_name) = match plugin {
            Some(name) => (self.store.enabled(name)?, tool.trim().to_string()),
            None => {
                let mut matches = self
                    .enabled()
                    .into_iter()
                    .filter(|plugin| plugin.manifest.tool(tool).is_some());
                let first = matches.next();
                let Some(plugin) = first else {
                    return Err(PluginError::UnknownTool(
                        "<any>".to_string(),
                        tool.to_string(),
                    ));
                };
                if matches.next().is_some() {
                    return Err(PluginError::Manifest(format!(
                        "tool {tool} is declared by more than one plugin; name the plugin too"
                    )));
                }
                (plugin, tool.to_string())
            }
        };
        let manifest_tool = plugin
            .manifest
            .tool(&tool_name)
            .ok_or_else(|| {
                PluginError::UnknownTool(plugin.manifest.name.clone(), tool_name.clone())
            })?
            .clone();
        let host = PluginHost::new(plugin.directory.clone(), plugin.manifest.clone());
        host.call(&manifest_tool, arguments).await
    }
}

/* --------------------------------- catalog -------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogKind {
    Skill,
    Plugin,
    External,
}

impl CatalogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogKind::Skill => "skill",
            CatalogKind::Plugin => "plugin",
            CatalogKind::External => "external",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatalogTier {
    Official,
    Verified,
    Community,
}

impl CatalogTier {
    pub fn as_str(self) -> &'static str {
        match self {
            CatalogTier::Official => "official",
            CatalogTier::Verified => "verified",
            CatalogTier::Community => "community",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub name: String,
    pub title: String,
    pub kind: CatalogKind,
    pub tier: CatalogTier,
    pub category: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// The publisher named by an external marketplace listing.
    #[serde(default)]
    pub author: Option<String>,
    /// The upstream plugin version, when the listing names one.
    #[serde(default)]
    pub version: Option<String>,
    /// Provenance for an external listing, such as `claude-plugins-official`.
    #[serde(default)]
    pub source: Option<String>,
    /// `owner/repo` for a pinned install.
    #[serde(default)]
    pub repo: Option<String>,
    /// The full commit SHA. Branches and tags are refused by the installer.
    #[serde(default)]
    pub sha: Option<String>,
    /// A directory inside the pinned repository that holds `plugin.json`.
    #[serde(default)]
    pub subdir: Option<String>,
    /// A plugin tree compiled into this build, installed without a network call.
    #[serde(default)]
    pub bundled: Option<String>,
    /// Capability names this entry is known to need, for the card.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Built-in skill ids this entry installs. Present on `kind: skill` entries.
    #[serde(default)]
    pub skills: Vec<String>,
    /// What to run when a listing is discovery only.
    #[serde(default)]
    pub install: Option<String>,
    /// An honest qualification: licence, review scope, or why it is evaluate-only.
    #[serde(default)]
    pub note: Option<String>,
}

impl CatalogEntry {
    pub fn installable(&self) -> bool {
        match self.kind {
            CatalogKind::Skill => !self.skills.is_empty(),
            CatalogKind::Plugin => {
                self.bundled.as_deref().is_some_and(bundled_available)
                    || (self.repo.is_some()
                        && self.sha.as_deref().is_some_and(|sha| {
                            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())
                        }))
            }
            CatalogKind::External => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub updated: String,
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn embedded() -> Result<Self, PluginError> {
        Self::parse(CATALOG_JSON)
    }

    pub fn load(index_dir: &Path) -> Result<Self, PluginError> {
        let path = index_dir.join("plugins").join("catalog.json");
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::embedded(),
            Err(error) => Err(PluginError::Io(error)),
        }
    }

    pub fn parse(text: &str) -> Result<Self, PluginError> {
        let catalog: Self = serde_json::from_str(text)
            .map_err(|error| PluginError::Manifest(format!("catalogue: {error}")))?;
        if catalog.schema != 1 {
            return Err(PluginError::Manifest(format!(
                "catalogue schema {} is not supported",
                catalog.schema
            )));
        }
        for entry in &catalog.entries {
            validate_name("catalogue entry", &entry.name)?;
            if entry.kind == CatalogKind::Plugin && entry.sha.is_some() && !entry.installable() {
                return Err(PluginError::Manifest(format!(
                    "catalogue entry {} has a SHA that is not 40 hex characters",
                    entry.name
                )));
            }
        }
        Ok(catalog)
    }

    pub fn get(&self, name: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Case-insensitive match over name, title, description, category and capability names.
    /// Deterministic: tier first, then name, so a card order cannot hop between reads.
    pub fn search(
        &self,
        query: &str,
        category: Option<&str>,
        tier: Option<&str>,
    ) -> Vec<CatalogEntry> {
        let needle = query.trim().to_lowercase();
        let category = category.map(str::to_lowercase);
        let tier = tier.map(str::to_lowercase);
        let mut entries: Vec<CatalogEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                category
                    .as_deref()
                    .is_none_or(|wanted| entry.category.eq_ignore_ascii_case(wanted))
            })
            .filter(|entry| {
                tier.as_deref()
                    .is_none_or(|wanted| entry.tier.as_str() == wanted)
            })
            .filter(|entry| {
                needle.is_empty()
                    || entry.name.to_lowercase().contains(&needle)
                    || entry.title.to_lowercase().contains(&needle)
                    || entry.description.to_lowercase().contains(&needle)
                    || entry.category.to_lowercase().contains(&needle)
                    || entry
                        .author
                        .as_deref()
                        .is_some_and(|author| author.to_lowercase().contains(&needle))
                    || entry
                        .version
                        .as_deref()
                        .is_some_and(|version| version.to_lowercase().contains(&needle))
                    || entry
                        .source
                        .as_deref()
                        .is_some_and(|source| source.to_lowercase().contains(&needle))
                    || entry
                        .capabilities
                        .iter()
                        .any(|capability| capability.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();
        entries.sort_by(|a, b| {
            tier_rank(a.tier)
                .cmp(&tier_rank(b.tier))
                .then_with(|| a.name.cmp(&b.name))
        });
        entries
    }

    pub fn categories(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .map(|entry| entry.category.clone())
            .collect()
    }
}

fn tier_rank(tier: CatalogTier) -> u8 {
    match tier {
        CatalogTier::Official => 0,
        CatalogTier::Verified => 1,
        CatalogTier::Community => 2,
    }
}

/* --------------------------------- bundled -------------------------------- */

/// First-party plugin trees compiled into the binary. Installing one writes plain files into the
/// plugin store, so the receipt covers exactly what will run and the tree can be read before it
/// is enabled.
pub struct BundledPlugin {
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub const BUNDLED_PLUGINS: &[BundledPlugin] = &[BundledPlugin {
    name: "rigel-hello",
    version: "1.0.0",
    description: "Two small tools that show the protocol and the capability broker working.",
    files: &[
        (
            "plugin.json",
            include_str!("bundled/rigel-hello/plugin.json"),
        ),
        ("main.py", include_str!("bundled/rigel-hello/main.py")),
    ],
}];

impl PluginStore {
    /// Write a bundled tree into the store through the same validation and receipt path as a
    /// local install. The tree is staged first; a failure leaves nothing installed.
    pub fn install_bundled(&self, name: &str, force: bool) -> Result<InstallReport, PluginError> {
        let bundled = BUNDLED_PLUGINS
            .iter()
            .find(|plugin| plugin.name == name)
            .ok_or_else(|| PluginError::UnknownCatalogEntry(name.to_string()))?;
        fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(".bundled-{name}-{}", now_ms()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let result = (|| {
            for (path, contents) in bundled.files {
                let target = staging.join(path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, contents)?;
            }
            let mut report = self.install_local(&staging, force)?;
            report.digest = tree_digest(&report.directory)?;
            // The staging directory is gone by the time anyone reads the receipt. Record the
            // bundled origin, not a temporary path that no longer exists.
            let mut state = self.load_state()?;
            if let Some(receipt) = state.receipts.get_mut(name) {
                receipt.source = format!("bundled:{name}");
            }
            self.save_state(&state)?;
            Ok(report)
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }
}

/// The catalogue and the bundled trees must not drift: an installable entry names a tree this
/// build actually carries.
pub fn bundled_available(name: &str) -> bool {
    BUNDLED_PLUGINS.iter().any(|plugin| plugin.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> PluginManifest {
        PluginManifest::parse(json).unwrap()
    }

    #[test]
    fn a_minimal_manifest_needs_no_capabilities() {
        let plugin = manifest(
            r#"{"schema":1,"name":"hello-world","version":"1.0.0","description":"says hello",
                "entry":{"command":"python","args":["main.py"]},
                "tools":[{"name":"hello","description":"Say hello."}]}"#,
        );
        assert_eq!(plugin.capabilities.summary(), vec!["none"]);
        assert_eq!(plugin.qualified_tools(), vec!["hello-world.hello"]);
        assert!(plugin.tool("hello").is_some());
    }

    #[test]
    fn unknown_fields_are_refused_rather_than_ignored() {
        let error = PluginManifest::parse(
            r#"{"schema":1,"name":"x","version":"1","entry":{"command":"x"},"toolz":[]}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn a_traversing_or_empty_root_is_refused() {
        for root in ["../outside", ""] {
            let text = format!(
                r#"{{"schema":1,"name":"x","version":"1","entry":{{"command":"x"}},
                    "capabilities":{{"filesystem":{{"read":["{root}"]}}}}}}"#
            );
            assert!(PluginManifest::parse(&text).is_err(), "accepted {root:?}");
        }
    }

    #[test]
    fn tools_need_an_entry_unique_names_and_an_object_schema() {
        for text in [
            r#"{"schema":1,"name":"x","version":"1","tools":[{"name":"a","description":"A"}]}"#,
            r#"{"schema":1,"name":"x","version":"1","entry":{"command":"x"},
                "tools":[{"name":"a","description":"A"},{"name":"a","description":"B"}]}"#,
            r#"{"schema":1,"name":"x","version":"1","entry":{"command":"x"},
                "tools":[{"name":"a","description":"A","parameters":[]}]}"#,
        ] {
            assert!(PluginManifest::parse(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn a_pinned_entry_requires_a_full_sha() {
        let catalog = Catalog::parse(
            r#"{"schema":1,"updated":"2026-11-22","entries":[
                {"name":"short","title":"Short","kind":"plugin","tier":"community",
                 "category":"Tools","description":"d","repo":"owner/repo","sha":"abc123"}]}"#,
        );
        assert!(catalog.is_err());
    }

    #[test]
    fn catalogue_search_is_case_insensitive_and_tier_ordered() {
        let catalog = Catalog::parse(
            r#"{"schema":1,"updated":"2026-11-22","entries":[
                {"name":"zeta","title":"Zeta","kind":"external","tier":"community",
                 "category":"Memory","description":"graph memory"},
                {"name":"alpha","title":"Alpha","kind":"external","tier":"verified",
                 "category":"Memory","description":"graph memory"}]}"#,
        )
        .unwrap();
        let hits = catalog.search("GRAPH", Some("memory"), None);
        assert_eq!(hits[0].name, "alpha");
        assert_eq!(hits[1].name, "zeta");
        assert_eq!(catalog.categories().len(), 1);
    }

    #[test]
    fn the_embedded_catalogue_parses_and_every_skill_entry_names_a_builtin() {
        let catalog = Catalog::embedded().expect("the shipped catalogue parses");
        assert!(
            catalog.entries.len() >= 500,
            "the shipped catalogue has only {} entries",
            catalog.entries.len()
        );
        let mut names = std::collections::HashSet::new();
        for entry in &catalog.entries {
            assert!(
                names.insert(entry.name.to_lowercase()),
                "duplicate catalogue name {}",
                entry.name
            );
        }
        for entry in catalog
            .entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Skill)
        {
            for skill in &entry.skills {
                assert!(
                    crate::skills::builtin::get(skill).is_some(),
                    "{} names missing builtin skill {skill}",
                    entry.name
                );
            }
        }
        assert!(
            catalog
                .get("superpowers")
                .is_some_and(CatalogEntry::installable),
            "superpowers must be installable from the bundled skill set"
        );
        assert!(
            catalog
                .get("untangle-codebase")
                .is_some_and(CatalogEntry::installable),
            "untangle-codebase must be installable from the bundled skill set"
        );
    }

    #[test]
    fn a_tree_digest_is_stable_and_changes_with_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one").unwrap();
        let first = tree_digest(dir.path()).unwrap();
        assert_eq!(first, tree_digest(dir.path()).unwrap());
        fs::write(dir.path().join("a.txt"), "two").unwrap();
        assert_ne!(first, tree_digest(dir.path()).unwrap());
    }
}
