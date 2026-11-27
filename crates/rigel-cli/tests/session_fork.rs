use std::{path::Path, process::Command};

use rigel_core::SessionService;
use tempfile::TempDir;

#[tokio::test]
async fn session_fork_cli_creates_a_lineaged_branch_with_prefix_history() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let service = SessionService::open(&project).await.unwrap();
    let parent = service.start(Some("Parent session"), None).await.unwrap();
    let agent = service
        .register_agent(&parent.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("prompt at fork point"))
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "answer",
            "completed",
            &serde_json::json!({"text": "answer"}),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    service.finish_turn(&turn.id, "completed").await.unwrap();
    drop(service);

    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["session", "fork", "--project"])
        .arg(&project)
        .args(["--id", &parent.id, "--turn", &turn.id])
        .output()
        .expect("rigel runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fork_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let listing = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["session", "list", "--project"])
        .arg(&project)
        .output()
        .expect("rigel lists sessions");
    assert!(listing.status.success());
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains(&format!("fork of {} at {}", &parent.id[..8], &turn.id[..8])));

    let service = SessionService::open(Path::new(&project)).await.unwrap();
    let fork = service.get(&fork_id).await.unwrap();
    assert_eq!(fork.parent_session_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(fork.parent_turn_id.as_deref(), Some(turn.id.as_str()));
    let transcript = service.transcript(&fork.id).await.unwrap();
    assert_eq!(transcript.len(), 1);
    assert_eq!(
        transcript[0].turn.prompt.as_deref(),
        Some("prompt at fork point")
    );
    assert_eq!(transcript[0].items[0].body["text"], "answer");
}
