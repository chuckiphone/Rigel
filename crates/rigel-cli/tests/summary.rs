//! `rigel session summary` reads the row `finish_turn` stored, not a fresh derivation.

use std::path::Path;
use std::process::Command;

use rigel_core::SessionService;
use serde_json::Value;
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_rigel");
    let output = Command::new(exe)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("rigel runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[tokio::test]
async fn session_summary_prints_the_stored_summary_and_provenance() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();

    let service = SessionService::open(&project).await.unwrap();
    let session = service.start(Some("PDF importer"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("fix the lexer crash"))
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "message",
            "completed",
            &serde_json::json!({ "role": "assistant", "text": "the crash is in the lexer" }),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    service.finish_turn(&turn.id, "completed").await.unwrap();
    drop(service);

    let (code, stdout) = rigel(
        &data,
        &[
            "session",
            "summary",
            "--project",
            project.to_str().unwrap(),
            "--id",
            &session.id,
        ],
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("the crash is in the lexer"), "{stdout}");
    assert!(stdout.contains("1 items"), "{stdout}");

    let (code, stdout) = rigel(
        &data,
        &[
            "session",
            "summary",
            "--project",
            project.to_str().unwrap(),
            "--id",
            &session.id,
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let versioned: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(versioned["schema_version"], 1);
    let rows = &versioned["data"];
    assert_eq!(rows[0]["turn_id"], turn.id);
    assert_eq!(rows[0]["item_ids"].as_array().unwrap().len(), 1);
    assert!(
        rows[0]["summary"]
            .as_str()
            .unwrap()
            .contains("the crash is in the lexer")
    );

    let (code, _) = rigel(
        &data,
        &[
            "session",
            "summary",
            "--project",
            project.to_str().unwrap(),
            "--id",
            "missing",
        ],
    );
    assert_ne!(code, 0, "an unknown session is an error, not an empty list");
}
