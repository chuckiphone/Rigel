//! `rigel doctor --fix` (roadmap 105).

use std::fs;
use std::process::Command;

#[test]
fn fix_lists_repairs_then_reports_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let rigel = dir.path().join(".rigel");
    fs::create_dir_all(&rigel).unwrap();
    fs::write(rigel.join("rigel.db-wal"), "x").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["doctor", "--fix", "--project", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("repairs planned"));
    assert!(stdout.contains("missing-gitignore"));
    assert!(stdout.contains("uncheckpointed-wal"));

    let out2 = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["doctor", "--fix", "--project", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("nothing to do"));
}

#[test]
fn fix_preserves_referenced_blobs_and_removes_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let rigel = dir.path().join(".rigel");
    let blobs = rigel.join("blobs");
    fs::create_dir_all(&blobs).unwrap();
    let referenced = "a".repeat(64);
    let orphan = "orphan.tmp";
    fs::write(blobs.join(&referenced), b"data").unwrap();
    fs::write(blobs.join(orphan), b"junk").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["doctor", "--fix", "--project", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("orphan-blob"));
    assert!(blobs.join(&referenced).exists());
    assert!(!blobs.join(orphan).exists());

    // second run: nothing to do
    let out2 = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["doctor", "--fix", "--project", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("nothing to do"));
}
