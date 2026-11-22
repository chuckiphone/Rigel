//! Roadmap 74: the CLI snapshot lifecycle, through the real binary.
//!
//! Capture and prune share the app-data snapshot directory. A readable snapshot whose project
//! still exists survives a prune; a directory with no manifest is the orphan it removes.

use std::path::Path;
use std::process::Command;

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

#[test]
fn prune_removes_only_the_orphan_and_dry_run_leaves_it_alone() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "protected\n").unwrap();

    let (status, stdout) = rigel(
        &data,
        &[
            "snapshot",
            "capture",
            "--project",
            project.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let captured: Value = serde_json::from_str(&stdout).unwrap();
    let snapshot_id = captured["snapshot"]["id"].as_str().unwrap().to_string();

    let orphan = data.join("snapshots").join("orphan-without-manifest");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("partial-copy"), "not a manifest").unwrap();

    let (status, stdout) = rigel(
        &data,
        &[
            "snapshot",
            "prune",
            "--older-than-days",
            "0",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(status, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["gc"]["removed"][0]["id"], "orphan-without-manifest");
    assert!(orphan.exists(), "a dry run must not delete the orphan");

    let (status, stdout) = rigel(
        &data,
        &["snapshot", "prune", "--older-than-days", "0", "--json"],
    );
    assert_eq!(status, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["gc"]["removed"][0]["id"], "orphan-without-manifest");
    assert!(!orphan.exists());

    let (status, stdout) = rigel(&data, &["snapshot", "list", "--json"]);
    assert_eq!(status, 0, "{stdout}");
    let listed: Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        listed["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|snapshot| snapshot["id"] == snapshot_id),
        "the readable snapshot with a live project must survive"
    );
}
