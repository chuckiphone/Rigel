use std::{path::Path, process::Command};

use rigel_core::SessionService;
use serde_json::Value;
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
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
async fn import_cli_dry_run_writes_nothing_then_import_persists_transcript() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = tmp.path().join("claude.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"import CLI prompt\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"import CLI answer\"}]}}\n",
            "bad json\n",
            "{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"duplicate\"}]}}"
        ),
    )
    .unwrap();

    let common = [
        "session",
        "import",
        "--project",
        project.to_str().unwrap(),
        "--file",
        transcript.to_str().unwrap(),
        "--source",
        "claude",
        "--json",
    ];
    let mut dry_args = common.to_vec();
    dry_args.push("--dry-run");
    let (status, stdout) = rigel(&data, &dry_args);
    assert_eq!(status, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["rows"].as_array().unwrap().len(), 4);
    assert!(
        report["rows"][2]["reason"]
            .as_str()
            .unwrap()
            .contains("invalid JSON")
    );
    assert!(
        report["rows"][3]["reason"]
            .as_str()
            .unwrap()
            .contains("duplicate")
    );
    let service = SessionService::open(&project).await.unwrap();
    assert!(service.list().await.unwrap().is_empty());
    drop(service);

    let (status, stdout) = rigel(&data, &common);
    assert_eq!(status, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    let session_id = report["session_id"].as_str().unwrap();
    let service = SessionService::open(&project).await.unwrap();
    let transcript = service.transcript(session_id).await.unwrap();
    assert_eq!(transcript.len(), 1);
    assert_eq!(
        transcript[0].turn.prompt.as_deref(),
        Some("import CLI prompt")
    );
    assert_eq!(transcript[0].items[0].body["text"], "import CLI answer");
}
