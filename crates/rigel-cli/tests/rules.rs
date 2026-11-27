//! `rigel rules explain` (roadmap 98).

use std::fs;
use std::process::Command;

#[test]
fn explain_prints_the_winner_and_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("AGENTS.md"), "rule A").unwrap();
    fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
    fs::write(dir.path().join(".cursor/rules/x.md"), "rule B").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args([
            "rules",
            "explain",
            "--project",
            dir.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("winner: AGENTS.md:1"));
    assert!(stdout.contains("conflicts: 1"));
}
