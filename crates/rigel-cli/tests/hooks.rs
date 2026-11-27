//! Roadmap 156: the hook suite is installed and removed through the real CLI. The byte-for-byte
//! checks are deliberately exact: a user's existing hook must survive uninstall unchanged.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("rigel runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn git(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn workspace(tmp: &TempDir) -> (PathBuf, PathBuf) {
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    git(&project, &["init", "-q"]);
    (project, data)
}

fn hook(project: &Path, name: &str) -> PathBuf {
    project.join(".git").join("hooks").join(name)
}

#[test]
fn uninstall_restores_a_previous_hook_byte_for_byte() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = workspace(&tmp);
    // CRLF, no trailing newline, and a quote: nothing may be normalised on the way back.
    let original = b"#!/bin/sh\r\n# my own hook\r\necho 'hello'";
    std::fs::write(hook(&project, "pre-commit"), original).unwrap();

    let project_arg = project.to_string_lossy().into_owned();
    let (code, stdout, stderr) = rigel(
        &data,
        &["hooks", "install", "--project", &project_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["installed"].as_array().unwrap().len(), 3);
    assert_eq!(report["backed_up"][0], "pre-commit");

    assert!(
        String::from_utf8_lossy(&std::fs::read(hook(&project, "pre-commit")).unwrap())
            .contains("rigel hooks scan")
    );
    assert!(hook(&project, "prepare-commit-msg").exists());
    assert!(hook(&project, "commit-msg").exists());
    assert_eq!(
        std::fs::read(project.join(".rigel/hooks/pre-commit.orig")).unwrap(),
        original,
        "the backup is the exact original bytes"
    );

    let (code, stdout, _) = rigel(
        &data,
        &["hooks", "status", "--project", &project_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout}");
    let status: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(status["installed"], true);
    assert_eq!(status["hooks"][0]["backed_up"], true);
    assert_eq!(status["hooks"][0]["modified"], false);

    let (code, stdout, stderr) = rigel(
        &data,
        &["hooks", "uninstall", "--project", &project_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["restored"][0], "pre-commit");
    assert_eq!(report["removed"].as_array().unwrap().len(), 2);

    assert_eq!(
        std::fs::read(hook(&project, "pre-commit")).unwrap(),
        original,
        "uninstall restored the previous hook byte for byte"
    );
    assert!(!hook(&project, "prepare-commit-msg").exists());
    assert!(!hook(&project, "commit-msg").exists());
    assert!(!project.join(".rigel/hooks.json").exists());
    assert!(!project.join(".rigel/hooks").exists());
}

#[test]
fn hooks_added_by_rigel_are_removed_on_uninstall() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = workspace(&tmp);
    let project_arg = project.to_string_lossy().into_owned();

    assert_eq!(
        rigel(&data, &["hooks", "install", "--project", &project_arg]).0,
        0
    );
    let (code, stdout, _) = rigel(
        &data,
        &["hooks", "uninstall", "--project", &project_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["restored"].as_array().unwrap().len(), 0);
    assert_eq!(report["removed"].as_array().unwrap().len(), 3);
    for name in ["pre-commit", "prepare-commit-msg", "commit-msg"] {
        assert!(!hook(&project, name).exists(), "{name} was removed");
    }
}

#[test]
fn uninstall_refuses_to_overwrite_a_hook_edited_after_install() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = workspace(&tmp);
    let project_arg = project.to_string_lossy().into_owned();
    assert_eq!(
        rigel(&data, &["hooks", "install", "--project", &project_arg]).0,
        0
    );

    std::fs::write(hook(&project, "pre-commit"), "# edited by a human\n").unwrap();
    let (code, stdout, _) = rigel(
        &data,
        &["hooks", "uninstall", "--project", &project_arg, "--json"],
    );
    assert_ne!(code, 0, "the edit is not silently discarded");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["uninstalled"], false);
    assert!(report["error"].as_str().unwrap().contains("pre-commit"));
    assert_eq!(
        std::fs::read_to_string(hook(&project, "pre-commit")).unwrap(),
        "# edited by a human\n"
    );
    assert!(
        project.join(".rigel/hooks.json").exists(),
        "retry is possible"
    );
}

#[test]
fn the_trailer_is_added_once_and_satisfies_the_attribution_check() {
    let tmp = TempDir::new().unwrap();
    let (_, data) = workspace(&tmp);
    let message = tmp.path().join("COMMIT_EDITMSG");
    std::fs::write(&message, "fix the parser\n\ncoach: sous chef\n").unwrap();
    let message_arg = message.to_string_lossy().into_owned();

    let (code, stdout, stderr) = rigel(
        &data,
        &[
            "hooks",
            "trailer",
            "--file",
            &message_arg,
            "--agent",
            "line cook",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["added"], true);
    assert_eq!(report["trailer"], "rigel: line-cook");
    let attributed = std::fs::read_to_string(&message).unwrap();
    assert_eq!(
        attributed,
        "fix the parser\n\ncoach: sous chef\n\nrigel: line-cook\n"
    );

    let (code, stdout, _) = rigel(
        &data,
        &["hooks", "check-msg", "--file", &message_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout}");
    let check: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(check["attributed"], true);

    let (code, stdout, _) = rigel(
        &data,
        &[
            "hooks",
            "trailer",
            "--file",
            &message_arg,
            "--agent",
            "someone else",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["added"], false, "a second trailer is never appended");
    assert_eq!(std::fs::read_to_string(&message).unwrap(), attributed);

    std::fs::write(&message, "unattributed work\n").unwrap();
    let (code, stdout, _) = rigel(
        &data,
        &["hooks", "check-msg", "--file", &message_arg, "--json"],
    );
    assert_ne!(code, 0, "an unattributed commit is refused");
    let check: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(check["attributed"], false);
}

#[test]
fn the_pre_commit_scan_blocks_an_added_secret_and_never_prints_it() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = workspace(&tmp);
    let project_arg = project.to_string_lossy().into_owned();
    std::fs::write(project.join(".env"), "AKIAIOSFODNN7EXAMPLE\n").unwrap();
    git(&project, &["add", ".env"]);

    let (code, stdout, stderr) = rigel(
        &data,
        &["hooks", "scan", "--project", &project_arg, "--json"],
    );
    assert_ne!(code, 0, "an added secret blocks the commit");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["findings"][0]["rule"], "aws-access-key");
    assert!(
        !stdout.contains("AKIA") && !stderr.contains("AKIA"),
        "the finding must not carry the secret"
    );

    git(&project, &["rm", "--cached", "-q", ".env"]);
    std::fs::write(project.join("clean.txt"), "just a note\n").unwrap();
    git(&project, &["add", "clean.txt"]);
    let (code, stdout, stderr) = rigel(
        &data,
        &["hooks", "scan", "--project", &project_arg, "--json"],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["clean"], true);
    assert_eq!(report["findings"].as_array().unwrap().len(), 0);
}
