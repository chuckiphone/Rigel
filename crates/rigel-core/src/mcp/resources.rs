//! MCP resources and prompts (roadmap 146).
//!
//! MCP is not only tools: a server can expose read-only resources (files, records) and
//! prompt templates. This module discovers and reads them with the same `initialize` /
//! `tools/list`-style handshake used for tools, so a resource read or a prompt fetch
//! carries the same latency, protocol version and error reporting as a tool call and can
//! be routed through the same approval and audit path.

use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::mcp::health::ServerSpec;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<McpPromptArgument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceReadResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptGetResult {
    pub description: Option<String>,
    pub messages: Vec<PromptMessage>,
}

/// A record that can be stored on the audit path exactly like a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceReadRecord {
    pub server: String,
    pub uri: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Internal handshake helper: send one request and collect the matching response.
async fn rpc(
    server: &ServerSpec,
    method: &str,
    params: Value,
    id: i64,
    wait: Duration,
) -> Result<Value, String> {
    if server.command.trim().is_empty() {
        return Err(format!("{} has no command to run", server.name));
    }
    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .envs(server.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", server.name))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{} has no stdin", server.name))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{} has no stdout", server.name))?;

    let name = server.name.clone();
    let work = async move {
        // initialize
        let mut payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "rigel", "version": env!("CARGO_PKG_VERSION") }
            }
        }))
        .unwrap();
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;

        let mut lines = BufReader::new(stdout).lines();
        let mut initialized = false;
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") == Some(&json!(0)) {
                if message.get("error").is_some() {
                    return Err(format!("{name} refused initialize"));
                }
                initialized = true;
                break;
            }
        }
        if !initialized {
            return Err(format!("{name} closed before initialize"));
        }

        // notifications/initialized
        let mut payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .unwrap();
        payload.push('\n');
        let _ = stdin.write_all(payload.as_bytes()).await;
        let _ = stdin.flush().await;

        // the actual request
        let mut payload = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .unwrap();
        payload.push('\n');
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;

        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!("{name} returned an error: {error}"));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(format!("{name} closed before responding"))
    };

    let result = tokio::time::timeout(wait, work).await;
    let _ = child.start_kill();
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(format!("{} did not answer in time", server.name)),
    }
}

pub async fn list_resources(
    server: &ServerSpec,
    wait: Duration,
) -> Result<Vec<McpResource>, String> {
    let result = rpc(server, "resources/list", json!({}), 1, wait).await?;
    let resources = result
        .get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for r in resources {
        let uri = r
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = r
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if uri.is_empty() || name.is_empty() {
            continue;
        }
        out.push(McpResource {
            uri,
            name,
            description: r
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            mime_type: r
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Ok(out)
}

pub async fn list_prompts(server: &ServerSpec, wait: Duration) -> Result<Vec<McpPrompt>, String> {
    let result = rpc(server, "prompts/list", json!({}), 1, wait).await?;
    let prompts = result
        .get("prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for p in prompts {
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let arguments = p
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| {
                let name = a.get("name").and_then(Value::as_str)?.to_string();
                Some(McpPromptArgument {
                    name,
                    description: a
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    required: a.get("required").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect();
        out.push(McpPrompt {
            name,
            description: p
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            arguments,
        });
    }
    Ok(out)
}

pub async fn read_resource(
    server: &ServerSpec,
    uri: &str,
    wait: Duration,
) -> Result<ResourceReadResult, String> {
    let result = rpc(server, "resources/read", json!({ "uri": uri }), 2, wait).await?;
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for c in contents {
        out.push(ResourceContent {
            uri: c
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or(uri)
                .to_string(),
            mime_type: c
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
            text: c.get("text").and_then(Value::as_str).map(str::to_string),
            blob: c.get("blob").and_then(Value::as_str).map(str::to_string),
        });
    }
    Ok(ResourceReadResult { contents: out })
}

pub async fn get_prompt(
    server: &ServerSpec,
    name: &str,
    arguments: Option<Value>,
    wait: Duration,
) -> Result<PromptGetResult, String> {
    let mut params = json!({ "name": name });
    if let Some(args) = arguments {
        params["arguments"] = args;
    }
    let result = rpc(server, "prompts/get", params, 3, wait).await?;
    let description = result
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            Some(PromptMessage {
                role: m.get("role").and_then(Value::as_str)?.to_string(),
                content: m.get("content").cloned()?,
            })
        })
        .collect();
    Ok(PromptGetResult {
        description,
        messages,
    })
}

/// Build an audit record for a resource read, mirroring a tool call.
pub fn record_read(server: &str, uri: &str, ok: bool, error: Option<String>) -> ResourceReadRecord {
    ResourceReadRecord {
        server: server.to_string(),
        uri: uri.to_string(),
        ok,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_read_mirrors_a_tool_call() {
        let r = record_read("demo", "file:///a.txt", true, None);
        assert!(r.ok);
        assert_eq!(r.uri, "file:///a.txt");
    }

    #[tokio::test]
    async fn an_empty_command_fails_cleanly() {
        let server = ServerSpec {
            name: "empty".to_string(),
            command: String::new(),
            args: vec![],
            env: vec![],
        };
        assert!(
            list_resources(&server, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(list_prompts(&server, Duration::from_secs(1)).await.is_err());
    }
}
