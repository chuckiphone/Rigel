//! Q242: a project MCP server declared in `.mcp.json` is refused before its first tool call
//! until the user sees the real tool list and approves both the server and that tool.
//!
//! The fixture is a local stdio server; no provider key or network request is involved.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rigel_core::agent::approval::ApprovalBus;
use rigel_core::mcp::pin::{Catalog, PinStore};
use rigel_core::{ApprovalScope, SessionService};
use rigel_mcp::{McpContext, handle};
use serde_json::{Value, json};

async fn context() -> (tempfile::TempDir, McpContext, String) {
    let tmp = tempfile::TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("mcp"), Some(100)).await.unwrap();
    let agent = service
        .register_agent(&session.id, "claude", Some("worker"))
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("use a project tool"))
        .await
        .unwrap();
    std::fs::write(
        tmp.path().join(".mcp.json"),
        serde_json::to_string(&json!({
            "mcpServers": {
                "demo": {
                    "command": env!("CARGO_BIN_EXE_fake-mcp-server"),
                    "args": []
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let ctx = McpContext {
        project_root: tmp.path().to_path_buf(),
        session_id: session.id,
        agent_id: agent.id,
        index_dir: tmp.path().join("index"),
        approval_wait: Duration::from_secs(5),
        turn_id: Some(turn.id.clone()),
        tool_lists: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        pins: rigel_core::mcp::pin::PinStore::new(tmp.path()),
    };
    (tmp, ctx, turn.id)
}

fn request() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "rigel_approve",
            "arguments": {
                "tool_name": "mcp__demo__echo",
                "input": { "text": "hello" }
            }
        }
    })
}

fn decision(response: Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no tool text in {response}"));
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn a_project_mcp_server_is_refused_until_the_card_approves_it() {
    let (_tmp, ctx, turn) = context().await;
    let response = handle(&ctx, request()).await.unwrap();
    let decision = decision(response);

    assert_eq!(decision["behavior"], "deny");
    assert_eq!(decision["code"], "MCP_SERVER_DENIED");
    assert_eq!(decision["server"], "demo");
    assert!(
        decision["message"]
            .as_str()
            .unwrap()
            .contains("did not approve")
    );

    let service = SessionService::open(&ctx.project_root).await.unwrap();
    let items = service.items(&turn).await.unwrap();
    assert!(
        items.iter().any(|item| {
            item.kind == "approval"
                && item.body["server"] == "demo"
                && item.body["decision"] == "deny"
                && item.body["gate"] == "Q242"
        }),
        "the refusal is recorded in the transcript: {items:?}"
    );
}

#[tokio::test]
async fn an_approved_project_server_shows_its_tools_and_records_the_run() {
    let (_tmp, ctx, turn) = context().await;
    let bus = ApprovalBus::new(&ctx.index_dir);
    let responder = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut server_answered = false;
        while Instant::now() < deadline {
            for pending in bus.pending() {
                if pending.kind == "mcp_server" {
                    let tools = pending.detail.as_ref().unwrap()["tools"]
                        .as_array()
                        .unwrap();
                    let names: Vec<&str> = tools
                        .iter()
                        .filter_map(|tool| tool["name"].as_str())
                        .collect();
                    assert!(names.contains(&"echo"), "real tool list: {names:?}");
                    assert!(
                        pending.detail.as_ref().unwrap()["command"]
                            .as_str()
                            .unwrap()
                            .contains("fake-mcp-server")
                    );
                    bus.answer(&pending.id, Some(ApprovalScope::Project))
                        .unwrap();
                    server_answered = true;
                } else if pending.kind == "mcp_tool" {
                    assert!(server_answered, "the server is asked first");
                    assert_eq!(pending.detail.as_ref().unwrap()["tool"], "echo");
                    assert_eq!(
                        pending.detail.as_ref().unwrap()["tools"]
                            .as_array()
                            .unwrap()
                            .len(),
                        2
                    );
                    bus.answer(&pending.id, Some(ApprovalScope::Once)).unwrap();
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the two approval questions did not arrive");
    });

    let response = handle(&ctx, request()).await.unwrap();
    responder.await.unwrap();
    let decision = decision(response);

    assert_eq!(decision["behavior"], "allow");
    assert_eq!(decision["scope"], "once");
    assert_eq!(decision["server"], "demo");
    assert_eq!(decision["tool"], "echo");
    assert!(
        decision["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "echo")
    );

    let service = SessionService::open(&ctx.project_root).await.unwrap();
    let items = service.items(&turn).await.unwrap();
    assert!(
        items.iter().any(|item| {
            item.kind == "approval"
                && item.body["server"] == "demo"
                && item.body["mcp_tool"] == "echo"
                && item.body["decision"] == "allow"
        }),
        "the permitted run is recorded in the transcript: {items:?}"
    );
}

#[tokio::test]
async fn a_changed_tool_is_blocked_until_the_schema_diff_is_accepted() {
    let (tmp, ctx, turn) = context().await;
    let old = Catalog::new(
        "1",
        vec![
            json!({
                "name": "echo",
                "description": "Echo before the update",
                "inputSchema": {
                    "type": "object",
                    "properties": { "text": { "type": "string" } }
                }
            }),
            json!({
                "name": "dangerous_write",
                "description": "Write a file in the project",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } }
                }
            }),
        ],
    )
    .unwrap();
    PinStore::new(tmp.path()).accept("demo", &old).unwrap();

    let bus = ApprovalBus::new(&ctx.index_dir);
    let deny = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            for pending in bus.pending() {
                if pending.kind == "mcp_schema" {
                    let detail = pending.detail.as_ref().unwrap();
                    let diff = detail["schema_diff"].as_object().unwrap();
                    assert_eq!(diff["changed"][0]["name"], "echo");
                    assert!(
                        diff["blocked_tools"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|tool| tool == "echo")
                    );
                    assert!(
                        detail["note"]
                            .as_str()
                            .unwrap()
                            .contains("blocked until this card is accepted")
                    );
                    bus.answer(&pending.id, None).unwrap();
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the schema diff card did not arrive");
    });

    let refused = decision(handle(&ctx, request()).await.unwrap());
    deny.await.unwrap();
    assert_eq!(refused["behavior"], "deny");
    assert_eq!(refused["code"], "MCP_SCHEMA_DENIED");
    assert_eq!(refused["server"], "demo");
    assert_eq!(refused["tool"], "echo");

    let service = SessionService::open(&ctx.project_root).await.unwrap();
    let items = service.items(&turn).await.unwrap();
    assert!(
        items.iter().any(|item| {
            item.kind == "approval"
                && item.body["gate"] == "R68"
                && item.body["decision"] == "deny"
                && item.body["mcp_tool"] == "echo"
        }),
        "the schema refusal is recorded: {items:?}"
    );

    // The second call sees the same diff. Accepting it at project scope updates the pin and
    // only then does the ordinary server/tool approval continue.
    let bus = ApprovalBus::new(&ctx.index_dir);
    let accept = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut schema_answered = false;
        let mut server_answered = false;
        while Instant::now() < deadline {
            for pending in bus.pending() {
                match pending.kind.as_str() {
                    "mcp_schema" => {
                        bus.answer(&pending.id, Some(ApprovalScope::Project))
                            .unwrap();
                        schema_answered = true;
                    }
                    "mcp_server" => {
                        assert!(schema_answered, "the schema is accepted first");
                        bus.answer(&pending.id, Some(ApprovalScope::Project))
                            .unwrap();
                        server_answered = true;
                    }
                    "mcp_tool" => {
                        assert!(schema_answered);
                        assert!(server_answered);
                        bus.answer(&pending.id, Some(ApprovalScope::Once)).unwrap();
                        return;
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the schema, server and tool questions did not arrive in order");
    });

    let allowed = decision(handle(&ctx, request()).await.unwrap());
    accept.await.unwrap();
    assert_eq!(allowed["behavior"], "allow");
    assert_eq!(allowed["tool"], "echo");

    let pin = PinStore::new(tmp.path()).server("demo").unwrap().unwrap();
    assert_eq!(pin.server_version, "1");
    assert_eq!(pin.tools["echo"].schema["description"], "Echo one line");
}
