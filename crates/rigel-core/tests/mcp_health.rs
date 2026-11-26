//! MCP health and discovery (roadmap 144) integration tests.

use rigel_core::mcp::health::{ServerSpec, probe, probe_all};
use std::time::Duration;

#[tokio::test]
async fn a_missing_binary_fails_without_blocking_others() {
    let servers = vec![
        ServerSpec {
            name: "missing-a".to_string(),
            command: "/definitely/not/a/binary".to_string(),
            args: vec![],
            env: vec![],
        },
        ServerSpec {
            name: "missing-b".to_string(),
            command: "/definitely/not/a/binary2".to_string(),
            args: vec![],
            env: vec![],
        },
    ];
    let rows = probe_all(&servers, Duration::from_secs(1)).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.last_error.is_some()));
}

#[tokio::test]
async fn a_fake_server_reports_discovery_facts() {
    let Ok(path) = std::env::var("RIGEL_FAKE_MCP_SERVER") else {
        return;
    };
    let server = ServerSpec {
        name: "demo".to_string(),
        command: path,
        args: vec![],
        env: vec![],
    };
    let health = probe(&server, Duration::from_secs(5)).await;
    assert_eq!(health.state, rigel_core::mcp::health::ServerState::Ready);
    assert_eq!(health.protocol_version.as_deref(), Some("2025-06-18"));
    assert_eq!(health.tool_count, 2);
    assert!(health.last_error.is_none());
    assert!(health.latency_ms.is_some());
}