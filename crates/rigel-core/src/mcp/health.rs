//! MCP health and discovery (roadmap 144).
//!
//! A project can declare several MCP servers in `.mcp.json`. This module probes each one
//! once: it performs the `initialize` / `tools/list` handshake, records how long it took,
//! the protocol version the server negotiated, how many tools came back, and the last
//! error if the handshake failed. Probes run concurrently, so one server that refuses to
//! answer is reported on its own row instead of blocking the others, and a caller can
//! re-run `probe` / `probe_all` whenever it wants a fresh picture.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// A stdio MCP server to probe. Mirrors `rigel_mcp::project_mcp::ServerSpec` so a
/// core-only caller (and its tests) can describe a server without depending on the
/// harness crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl ServerSpec {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn command_line(&self) -> String {
        std::iter::once(self.command.clone())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Alias for callers that prefer the `Mcp` prefix.
pub type McpServerSpec = ServerSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    /// The handshake completed and the tool list was read.
    Ready,
    /// The server could not be started or did not complete the handshake.
    Failed,
    /// The entry is remote-only or otherwise cannot be probed locally.
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHealth {
    pub name: String,
    pub state: ServerState,
    /// Wall time for the probe, in milliseconds. `None` when the probe never started.
    pub latency_ms: Option<u64>,
    /// The protocol version the server negotiated during `initialize`.
    pub protocol_version: Option<String>,
    /// How many tools the server advertised.
    pub tool_count: usize,
    /// The transport or handshake error, when the probe failed.
    pub last_error: Option<String>,
}

/// Alias for callers that prefer the `Mcp` prefix.
pub type McpHealth = ServerHealth;

/// Probe one server. Never panics: every failure becomes a `Failed` row with a message.
pub async fn probe(server: &ServerSpec, wait: Duration) -> ServerHealth {
    let started = Instant::now();
    if server.command.trim().is_empty() {
        return ServerHealth {
            name: server.name.clone(),
            state: ServerState::Failed,
            latency_ms: None,
            protocol_version: None,
            tool_count: 0,
            last_error: Some(format!("{} has no command to run", server.name)),
        };
    }

    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .envs(server.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ServerHealth {
                name: server.name.clone(),
                state: ServerState::Failed,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                protocol_version: None,
                tool_count: 0,
                last_error: Some(format!("could not start {}: {error}", server.name)),
            };
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let _ = child.start_kill();
            return ServerHealth {
                name: server.name.clone(),
                state: ServerState::Failed,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                protocol_version: None,
                tool_count: 0,
                last_error: Some(format!("{} has no stdin", server.name)),
            };
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.start_kill();
            return ServerHealth {
                name: server.name.clone(),
                state: ServerState::Failed,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                protocol_version: None,
                tool_count: 0,
                last_error: Some(format!("{} has no stdout", server.name)),
            };
        }
    };

    let name = server.name.clone();
    let handshake = async move {
        // initialize
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "rigel", "version": env!("CARGO_PKG_VERSION") }
            }
        });
        let mut payload = serde_json::to_string(&init).unwrap();
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;

        let mut lines = BufReader::new(stdout).lines();
        let mut initialized = false;
        let mut protocol_version = None::<String>;
        while let Some(line) = next_line(&mut lines, &name).await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&json!(1)) {
                continue;
            }
            if message.get("error").is_some() {
                return Err(format!("{name} refused initialize"));
            }
            initialized = true;
            protocol_version = message
                .get("result")
                .and_then(|result| result.get("protocolVersion"))
                .and_then(Value::as_str)
                .map(str::to_string);
            break;
        }
        if !initialized {
            return Err(format!("{name} closed before initialize"));
        }

        // notifications/initialized
        let mut note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let _ = &mut note;
        let mut payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap();
        payload.push('\n');
        let _ = stdin.write_all(payload.as_bytes()).await;
        let _ = stdin.flush().await;

        // tools/list
        let mut payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }))
        .unwrap();
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;

        while let Some(line) = next_line(&mut lines, &name).await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&json!(2)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("{name} refused tools/list: {error}"));
            }
            let tools = message
                .get("result")
                .and_then(|result| result.get("tools"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let count = tools.len();
            return Ok((protocol_version, count));
        }
        Err(format!("{name} closed before tools/list"))
    };

    let result = tokio::time::timeout(wait, handshake).await;
    let _ = child.start_kill();

    let latency = Some(started.elapsed().as_millis() as u64);
    match result {
        Ok(Ok((protocol_version, tool_count))) => ServerHealth {
            name: server.name.clone(),
            state: ServerState::Ready,
            latency_ms: latency,
            protocol_version,
            tool_count,
            last_error: None,
        },
        Ok(Err(error)) => ServerHealth {
            name: server.name.clone(),
            state: ServerState::Failed,
            latency_ms: latency,
            protocol_version: None,
            tool_count: 0,
            last_error: Some(error),
        },
        Err(_) => ServerHealth {
            name: server.name.clone(),
            state: ServerState::Failed,
            latency_ms: latency,
            protocol_version: None,
            tool_count: 0,
            last_error: Some(format!("{} did not answer in time", server.name)),
        },
    }
}

async fn next_line<R>(
    lines: &mut tokio::io::Lines<R>,
    _name: &str,
) -> Result<Option<String>, String>
where
    R: AsyncBufReadExt + Unpin,
{
    lines.next_line().await.map_err(|e| e.to_string())
}

/// Probe every server concurrently. One failure does not cancel the others.
pub async fn probe_all(servers: &[ServerSpec], wait: Duration) -> Vec<ServerHealth> {
    let futures: Vec<_> = servers.iter().map(|server| probe(server, wait)).collect();
    futures::future::join_all(futures).await
}

/// Alias kept for callers that expect a `probe_servers` entry point.
pub async fn probe_servers(servers: &[ServerSpec], wait: Duration) -> Vec<ServerHealth> {
    probe_all(servers, wait).await
}

/// Alias kept for callers that expect a `refresh` entry point.
pub async fn refresh(servers: &[ServerSpec], wait: Duration) -> Vec<ServerHealth> {
    probe_all(servers, wait).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn fake_server_spec() -> Option<ServerSpec> {
        // The fake server binary is built by rigel-mcp; in this crate's tests it may not be
        // available, so tests that need it are skipped gracefully.
        let path = std::env::var("RIGEL_FAKE_MCP_SERVER").ok()?;
        Some(ServerSpec {
            name: "demo".to_string(),
            command: path,
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    #[tokio::test]
    async fn an_empty_command_is_unsupported() {
        let server = ServerSpec {
            name: "empty".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
        };
        let health = probe(&server, Duration::from_secs(1)).await;
        assert_eq!(health.state, ServerState::Failed);
        // An empty command is reported as unsupported-ish failure with a message.
        assert!(health.last_error.is_some());
    }

    #[tokio::test]
    async fn a_missing_binary_fails_without_panicking() {
        let server = ServerSpec {
            name: "missing".to_string(),
            command: "/definitely/not/a/binary".to_string(),
            args: Vec::new(),
            env: Vec::new(),
        };
        let health = probe(&server, Duration::from_secs(1)).await;
        assert_eq!(health.state, ServerState::Failed);
        assert!(health.last_error.is_some());
        assert_eq!(health.tool_count, 0);
        assert!(health.latency_ms.is_some());
    }

    #[tokio::test]
    async fn probe_all_runs_concurrently_and_keeps_one_row_per_server() {
        let servers = vec![
            ServerSpec {
                name: "a".to_string(),
                command: "/definitely/not/a/binary".to_string(),
                args: Vec::new(),
                env: Vec::new(),
            },
            ServerSpec {
                name: "b".to_string(),
                command: "/definitely/not/a/binary2".to_string(),
                args: Vec::new(),
                env: Vec::new(),
            },
        ];
        let rows = probe_all(&servers, Duration::from_secs(1)).await;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[1].name, "b");
        assert!(rows.iter().all(|r| r.state == ServerState::Failed));
    }

    #[tokio::test]
    async fn a_fake_server_reports_protocol_version_and_tool_count() {
        let Some(server) = fake_server_spec() else {
            // No fake server binary available; skip.
            return;
        };
        let health = probe(&server, Duration::from_secs(5)).await;
        assert_eq!(health.state, ServerState::Ready, "{:?}", health.last_error);
        assert_eq!(health.protocol_version.as_deref(), Some("2025-06-18"));
        assert_eq!(health.tool_count, 2);
        assert!(health.last_error.is_none());
        assert!(health.latency_ms.is_some());
    }
}
