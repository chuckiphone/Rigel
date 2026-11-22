//! The host side of the plugin protocol: one process per call, one capability broker.
//!
//! The broker is the only way a plugin can touch a file through Rigel. It canonicalises the
//! requested path and refuses it unless the resolved target is under a root the manifest
//! declares. A refusal is an error object; it never carries bytes. Direct operating-system
//! calls are outside this boundary, which is why the manifest's grant is shown on the install
//! card and why `docs/plugins.md` says so in the same words.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::Child;

use super::{
    Capabilities, DEFAULT_TIMEOUT, PROTOCOL_VERSION, PluginEntry, PluginError, PluginManifest,
    PluginTool,
};

/// Base variables a child needs to start at all. No provider key, no token, no credentials:
/// a plugin that wants one names it in `capabilities.env`, where it is visible before install.
const BASE_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "APPDATA",
    "LOCALAPPDATA",
];

const MAX_PROTOCOL_MESSAGES: usize = 256;
const MAX_CAPABILITY_BYTES: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("the manifest does not grant {0} for {1}")]
    Denied(&'static str, String),
    #[error("cannot resolve {0}: {1}")]
    Path(String, String),
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub struct PluginReply {
    pub plugin: String,
    pub tool: String,
    pub content: Value,
}

impl PluginReply {
    /// The text form a transcript or a CLI prints. Structured replies keep their JSON.
    pub fn text(&self) -> String {
        match &self.content {
            Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginHost {
    directory: PathBuf,
    manifest: PluginManifest,
}

impl PluginHost {
    pub fn new(directory: PathBuf, manifest: PluginManifest) -> Self {
        Self {
            directory,
            manifest,
        }
    }

    /// Start the entry command, handshake, call one tool, and stop the process. The timeout
    /// covers the handshake and the call together.
    pub async fn call(
        &self,
        tool: &PluginTool,
        arguments: Value,
    ) -> Result<PluginReply, PluginError> {
        let entry = self.manifest.entry.clone().ok_or_else(|| {
            PluginError::Manifest(format!(
                "plugin {} has tools but no entry command",
                self.manifest.name
            ))
        })?;
        let mut transport =
            StdioTransport::spawn(&self.directory, &entry, &self.manifest.capabilities).await?;
        let deadline = DEFAULT_TIMEOUT;
        let result =
            tokio::time::timeout(deadline, self.exchange(&mut transport, tool, arguments)).await;
        transport.close().await;
        match result {
            Ok(value) => value,
            Err(_) => Err(PluginError::Timeout(self.manifest.name.clone(), deadline)),
        }
    }

    /// The transport-generic half, so a test can drive a scripted plugin through the same
    /// capability path without a child process.
    pub async fn exchange(
        &self,
        transport: &mut dyn PluginTransport,
        tool: &PluginTool,
        arguments: Value,
    ) -> Result<PluginReply, PluginError> {
        let broker = Broker::new(self.directory.clone(), self.manifest.capabilities.clone());
        transport
            .send(&json!({
                "type": "hello",
                "protocol": PROTOCOL_VERSION,
                "plugin": self.manifest.name,
            }))
            .await?;

        let mut ready = false;
        for _ in 0..MAX_PROTOCOL_MESSAGES {
            let message = transport.recv().await?;
            match message.get("type").and_then(Value::as_str) {
                Some("ready") => {
                    ready = true;
                    break;
                }
                Some("log") => continue,
                _ => {
                    return Err(PluginError::Protocol(format!(
                        "{} sent {:?} before ready",
                        self.manifest.name,
                        message
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("a message")
                    )));
                }
            }
        }
        if !ready {
            return Err(PluginError::Protocol(format!(
                "{} never answered hello",
                self.manifest.name
            )));
        }

        let call_id = 1u64;
        transport
            .send(&json!({
                "type": "call",
                "id": call_id,
                "tool": tool.name,
                "arguments": arguments,
            }))
            .await?;

        for _ in 0..MAX_PROTOCOL_MESSAGES {
            let message = transport.recv().await?;
            match message.get("type").and_then(Value::as_str) {
                Some("capability") => {
                    let reply = broker.handle(&message);
                    transport.send(&reply).await?;
                }
                Some("result") => {
                    if message.get("id").and_then(Value::as_u64) != Some(call_id) {
                        continue;
                    }
                    if message.get("ok").and_then(Value::as_bool) != Some(true) {
                        let detail = message
                            .get("error")
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| "the plugin refused the call".to_string());
                        return Err(PluginError::Protocol(format!(
                            "{}: {detail}",
                            self.manifest.name
                        )));
                    }
                    let content = message.get("content").cloned().unwrap_or(Value::Null);
                    return Ok(PluginReply {
                        plugin: self.manifest.name.clone(),
                        tool: tool.name.clone(),
                        content,
                    });
                }
                Some("log") => continue,
                other => {
                    return Err(PluginError::Protocol(format!(
                        "{} sent unexpected {:?}",
                        self.manifest.name,
                        other.unwrap_or("message")
                    )));
                }
            }
        }
        Err(PluginError::Protocol(format!(
            "{} exceeded {MAX_PROTOCOL_MESSAGES} messages",
            self.manifest.name
        )))
    }
}

/// What a plugin may ask the host to do. Roots are resolved against the plugin directory and
/// canonicalised before a request is compared to them.
#[derive(Debug, Clone)]
pub struct Broker {
    root: PathBuf,
    capabilities: Capabilities,
}

impl Broker {
    pub fn new(root: PathBuf, capabilities: Capabilities) -> Self {
        Self { root, capabilities }
    }

    /// Answer one `capability` message. The shape is the protocol's error shape, so a plugin
    /// can branch on `ok` without parsing prose.
    pub fn handle(&self, message: &Value) -> Value {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let capability = message
            .get("capability")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reply = match capability {
            "fs.read" => {
                let path = message
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match self.read(path) {
                    Ok(content) => json!({ "ok": true, "content": content }),
                    Err(error) => json!({ "ok": false, "error": {
                        "code": error_code(&error), "message": error.to_string()
                    }}),
                }
            }
            "fs.write" => {
                let path = message
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let content = message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match self.write(path, content) {
                    Ok(bytes) => json!({ "ok": true, "bytes": bytes }),
                    Err(error) => json!({ "ok": false, "error": {
                        "code": error_code(&error), "message": error.to_string()
                    }}),
                }
            }
            "env.get" => {
                let name = message
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let allowed = self
                    .capabilities
                    .env
                    .iter()
                    .any(|candidate| candidate == name);
                if allowed {
                    match std::env::var(name) {
                        Ok(value) => json!({ "ok": true, "content": value }),
                        Err(_) => json!({ "ok": false, "error": {
                            "code": "ENV_MISSING", "message": format!("{name} is not set")
                        }}),
                    }
                } else {
                    json!({ "ok": false, "error": {
                        "code": "CAPABILITY_DENIED",
                        "message": format!("the manifest does not grant env.get for {name}")
                    }})
                }
            }
            other => json!({ "ok": false, "error": {
                "code": "CAPABILITY_DENIED",
                "message": format!("unknown capability {other:?}; the host grants fs.read, fs.write and env.get")
            }}),
        };
        let mut reply = reply;
        if let Some(object) = reply.as_object_mut() {
            object.insert("type".to_string(), json!("capability_result"));
            object.insert("id".to_string(), id);
        }
        reply
    }

    pub fn read(&self, requested: &str) -> Result<String, CapabilityError> {
        let (target, roots) = self.resolve(requested, false)?;
        let bytes = fs::read(&target)
            .map_err(|error| CapabilityError::Io(format!("{}: {error}", target.display())))?;
        if bytes.len() as u64 > MAX_CAPABILITY_BYTES {
            return Err(CapabilityError::Io(format!(
                "{} is larger than {} bytes",
                target.display(),
                MAX_CAPABILITY_BYTES
            )));
        }
        let _ = roots;
        String::from_utf8(bytes)
            .map_err(|_| CapabilityError::Io(format!("{} is not UTF-8", target.display())))
    }

    pub fn write(&self, requested: &str, content: &str) -> Result<usize, CapabilityError> {
        let (target, _roots) = self.resolve(requested, true)?;
        if target.is_dir() {
            return Err(CapabilityError::Io(format!(
                "{} is a directory",
                target.display()
            )));
        }
        fs::write(&target, content)
            .map_err(|error| CapabilityError::Io(format!("{}: {error}", target.display())))?;
        Ok(content.len())
    }

    /// Resolve the request and the declared roots to the same canonical form before comparing.
    fn resolve(
        &self,
        requested: &str,
        write: bool,
    ) -> Result<(PathBuf, Vec<PathBuf>), CapabilityError> {
        if requested.trim().is_empty() || requested.contains('\0') {
            return Err(CapabilityError::Denied(
                if write { "fs.write" } else { "fs.read" },
                requested.to_string(),
            ));
        }
        let declared = if write {
            &self.capabilities.filesystem.write
        } else {
            &self.capabilities.filesystem.read
        };
        if declared.is_empty() {
            return Err(CapabilityError::Denied(
                if write { "fs.write" } else { "fs.read" },
                requested.to_string(),
            ));
        }
        let requested_path = Path::new(requested);
        let lexical = if requested_path.is_absolute() {
            requested_path.to_path_buf()
        } else {
            self.root.join(requested_path)
        };
        if lexical
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(CapabilityError::Denied(
                if write { "fs.write" } else { "fs.read" },
                requested.to_string(),
            ));
        }
        let target = if write {
            canonical_for_write(&lexical)?
        } else {
            lexical
                .canonicalize()
                .map_err(|error| CapabilityError::Path(requested.to_string(), error.to_string()))?
        };
        let roots: Vec<PathBuf> = declared
            .iter()
            .map(|root| {
                let path = Path::new(root);
                let absolute = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.root.join(path)
                };
                absolute.canonicalize().unwrap_or(absolute)
            })
            .collect();
        let allowed = roots.iter().any(|root| target.starts_with(root));
        if !allowed {
            return Err(CapabilityError::Denied(
                if write { "fs.write" } else { "fs.read" },
                target.display().to_string(),
            ));
        }
        Ok((target, roots))
    }
}

fn canonical_for_write(path: &Path) -> Result<PathBuf, CapabilityError> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|error| CapabilityError::Path(path.display().to_string(), error.to_string()));
    }
    let parent = path.parent().ok_or_else(|| {
        CapabilityError::Path(
            path.display().to_string(),
            "no parent directory".to_string(),
        )
    })?;
    let parent = parent
        .canonicalize()
        .map_err(|error| CapabilityError::Path(parent.display().to_string(), error.to_string()))?;
    let name = path.file_name().ok_or_else(|| {
        CapabilityError::Path(path.display().to_string(), "no file name".to_string())
    })?;
    Ok(parent.join(name))
}

fn error_code(error: &CapabilityError) -> &'static str {
    match error {
        CapabilityError::Denied(..) => "CAPABILITY_DENIED",
        CapabilityError::Path(..) => "BAD_PATH",
        CapabilityError::Io(..) => "IO",
    }
}

#[async_trait]
pub trait PluginTransport: Send {
    async fn send(&mut self, message: &Value) -> Result<(), PluginError>;
    async fn recv(&mut self) -> Result<Value, PluginError>;
    async fn close(&mut self);
}

/// Newline-delimited JSON on a child's stdin/stdout. Stderr is dropped: a plugin's debugging
/// output is not a transcript line, and an inherited pipe could block the child when full.
pub struct StdioTransport {
    child: Child,
    stdin: BufWriter<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl StdioTransport {
    pub async fn spawn(
        directory: &Path,
        entry: &PluginEntry,
        capabilities: &Capabilities,
    ) -> Result<Self, PluginError> {
        let mut command = tokio::process::Command::new(&entry.command);
        command
            .args(&entry.args)
            .current_dir(directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for key in BASE_ENV
            .iter()
            .copied()
            .chain(capabilities.env.iter().map(String::as_str))
        {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        crate::process::hide_tokio(&mut command);
        let mut child = command.spawn().map_err(|error| {
            PluginError::Io(std::io::Error::other(format!(
                "could not start {}: {error}",
                entry.command
            )))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::Protocol("no stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::Protocol("no stdout".to_string()))?;
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }
}

#[async_trait]
impl PluginTransport for StdioTransport {
    async fn send(&mut self, message: &Value) -> Result<(), PluginError> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Value, PluginError> {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(10), self.stdout.read_line(&mut line))
            .await
            .map_err(|_| PluginError::Protocol("the plugin stopped answering".to_string()))??;
        if read == 0 {
            return Err(PluginError::Protocol(
                "the plugin closed its output".to_string(),
            ));
        }
        serde_json::from_str(line.trim())
            .map_err(|error| PluginError::Protocol(format!("not JSON: {error}")))
    }

    async fn close(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{Filesystem, PluginManifest};

    fn manifest(json: &str) -> PluginManifest {
        PluginManifest::parse(json).unwrap()
    }

    #[test]
    fn a_read_outside_the_declared_root_is_refused_without_opening_it() {
        let plugin = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "TOP SECRET").unwrap();
        let broker = Broker::new(
            plugin.path().to_path_buf(),
            Capabilities {
                filesystem: Filesystem {
                    read: vec![".".to_string()],
                    write: Vec::new(),
                },
                ..Capabilities::default()
            },
        );
        let error = broker.read(&secret.display().to_string()).unwrap_err();
        assert!(matches!(error, CapabilityError::Denied(..)));
        assert!(!error.to_string().contains("TOP SECRET"));
    }

    #[test]
    fn a_declared_root_permits_its_own_children_and_nothing_else() {
        let plugin = tempfile::tempdir().unwrap();
        fs::create_dir(plugin.path().join("notes")).unwrap();
        fs::write(plugin.path().join("notes/a.txt"), "inside").unwrap();
        let broker = Broker::new(
            plugin.path().to_path_buf(),
            Capabilities {
                filesystem: Filesystem {
                    read: vec!["notes".to_string()],
                    write: vec!["notes".to_string()],
                },
                ..Capabilities::default()
            },
        );
        assert_eq!(broker.read("notes/a.txt").unwrap(), "inside");
        assert!(broker.read("plugin.json").is_err());
        let bytes = broker.write("notes/b.txt", "written").unwrap();
        assert_eq!(bytes, 7);
        assert_eq!(
            fs::read_to_string(plugin.path().join("notes/b.txt")).unwrap(),
            "written"
        );
    }

    #[test]
    fn a_write_without_the_capability_is_denied_and_creates_nothing() {
        let plugin = tempfile::tempdir().unwrap();
        let broker = Broker::new(plugin.path().to_path_buf(), Capabilities::default());
        let error = broker.write("new.txt", "x").unwrap_err();
        assert!(matches!(error, CapabilityError::Denied(..)));
        assert!(!plugin.path().join("new.txt").exists());
    }

    /// A plugin that asks for a file it was not granted gets an error object, and the host never
    /// hands it the contents. This is the roadmap-153 fixture at the broker boundary.
    #[tokio::test]
    async fn a_plugin_without_a_grant_cannot_read_a_file_even_when_it_tries() {
        let plugin_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, "NEVER LEAVE").unwrap();
        let manifest = manifest(
            r#"{"schema":1,"name":"probe","version":"1.0.0",
                "entry":{"command":"unused"},"tools":[{"name":"probe","description":"Probe."}]}"#,
        );
        let mut transport = ScriptedTransport::new(vec![
            json!({ "type": "ready", "protocol": 1 }),
            json!({ "type": "result", "id": 1, "ok": true, "content": "asked" }),
        ]);
        let host = PluginHost::new(plugin_dir.path().to_path_buf(), manifest.clone());
        let tool = manifest.tool("probe").unwrap().clone();
        let reply = host
            .exchange(&mut transport, &tool, json!({}))
            .await
            .unwrap();
        assert_eq!(reply.content, json!("asked"));
        assert_eq!(transport.sent[0]["type"], "hello");
        assert_eq!(transport.sent[1]["type"], "call");
        assert_eq!(transport.sent[2]["type"], "capability_result");
        assert_eq!(transport.sent[2]["ok"], false);
        assert_eq!(transport.sent[2]["error"]["code"], "CAPABILITY_DENIED");
        assert!(!transport.sent[2].to_string().contains("NEVER LEAVE"));
    }

    /// The same fixture with a real child process, when a Python interpreter is available. The
    /// child writes the request and blocks for the host's answer; it never sees the file.
    #[tokio::test]
    async fn a_real_child_process_is_refused_by_the_broker() {
        let Some(python) = python() else {
            return;
        };
        let plugin_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "NEVER LEAVE").unwrap();
        fs::write(
            plugin_dir.path().join("probe.py"),
            r#"import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if message["type"] == "hello":
        print(json.dumps({"type": "ready", "protocol": 1}), flush=True)
    elif message["type"] == "call":
        print(json.dumps({"type": "capability", "id": 2, "capability": "fs.read", "path": "secret.txt"}), flush=True)
        answer = json.loads(sys.stdin.readline())
        print(json.dumps({"type": "result", "id": message["id"], "ok": True, "content": answer}), flush=True)
"#,
        )
        .unwrap();
        let manifest = manifest(
            r#"{"schema":1,"name":"probe","version":"1.0.0",
                "entry":{"command":"python","args":["probe.py"]},
                "tools":[{"name":"probe","description":"Probe."}]}"#,
        );
        let host = PluginHost::new(plugin_dir.path().to_path_buf(), manifest.clone());
        let reply = host
            .call(&manifest.tool("probe").unwrap().clone(), json!({}))
            .await
            .unwrap();
        assert_eq!(reply.content["ok"], false);
        assert_eq!(reply.content["error"]["code"], "CAPABILITY_DENIED");
        assert!(!reply.text().contains("NEVER LEAVE"));
        let _ = python;
    }

    fn python() -> Option<String> {
        let output = std::process::Command::new("python")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output.status.success().then(|| "python".to_string())
    }

    struct ScriptedTransport {
        sent: Vec<Value>,
        incoming: std::collections::VecDeque<Value>,
    }

    impl ScriptedTransport {
        fn new(incoming: Vec<Value>) -> Self {
            Self {
                sent: Vec::new(),
                incoming: incoming.into(),
            }
        }
    }

    #[async_trait]
    impl PluginTransport for ScriptedTransport {
        async fn send(&mut self, message: &Value) -> Result<(), PluginError> {
            self.sent.push(message.clone());
            if message.get("type").and_then(Value::as_str) == Some("call") {
                // Insert the capability request at the head, as the next thing the plugin says.
                self.incoming.push_front(json!({
                    "type": "capability", "id": 2, "capability": "fs.read", "path": "../secret.txt"
                }));
            }
            Ok(())
        }

        async fn recv(&mut self) -> Result<Value, PluginError> {
            self.incoming
                .pop_front()
                .ok_or_else(|| PluginError::Protocol("script ended".to_string()))
        }

        async fn close(&mut self) {}
    }
}
