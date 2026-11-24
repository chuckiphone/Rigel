//! `rigel test` as a program: changed-file selection, skipped reasons and a real runner.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn run(project: &Path, args: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(project.join(".rigel-data"))
        .args(args)
        .arg("--project")
        .arg(project)
        .output()
        .expect("rigel runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "not json: {error}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

fn git(project: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    command.arg("-C").arg(project).args(args);
    assert!(
        command.status().expect("git runs").success(),
        "git {args:?}"
    );
}

fn fixture() -> TempDir {
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("src")).unwrap();
    fs::write(
        project.path().join("package.json"),
        r#"{"name":"impact-fixture","private":true,"scripts":{"test":"node test.js"}}"#,
    )
    .unwrap();
    fs::write(project.path().join("test.js"), "console.log('3 passed');\n").unwrap();
    fs::write(
        project.path().join("src/index.js"),
        "export const one = 1;\n",
    )
    .unwrap();
    fs::write(project.path().join("README.md"), "fixture\n").unwrap();
    git(project.path(), &["init", "-q"]);
    git(
        project.path(),
        &["config", "user.email", "test@example.com"],
    );
    git(project.path(), &["config", "user.name", "Test"]);
    git(project.path(), &["add", "."]);
    git(project.path(), &["commit", "-qm", "base"]);
    project
}

#[test]
fn a_source_change_selects_the_package_runner_and_docs_are_explained() {
    let project = fixture();
    fs::write(
        project.path().join("src/index.js"),
        "export const two = 2;\n",
    )
    .unwrap();
    fs::write(project.path().join("README.md"), "fixture changed\n").unwrap();

    let (code, plan) = run(
        project.path(),
        &["test", "plan", "--since", "HEAD", "--json"],
    );
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["targets"][0]["label"], "npm run test");
    assert_eq!(plan["targets"][0]["reason"], "package test script");
    assert!(plan["full"].as_array().unwrap().is_empty());
    assert!(
        plan["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["path"] == "README.md")
    );
}

#[test]
fn a_manifest_change_asks_for_the_full_suite() {
    let project = fixture();
    fs::write(
        project.path().join("package.json"),
        r#"{"name":"impact-fixture","private":true,"scripts":{"test":"node test.js"},"description":"changed"}"#,
    )
    .unwrap();

    let (code, plan) = run(
        project.path(),
        &["test", "plan", "--since", "HEAD", "--json"],
    );
    assert_eq!(code, 0, "{plan}");
    assert!(
        plan["full"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["label"] == "npm run test")
    );
    assert!(
        plan["risk"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap().contains("package manifest"))
    );
}

#[test]
fn the_run_command_executes_the_selected_target() {
    let project = fixture();
    fs::write(
        project.path().join("src/index.js"),
        "export const two = 2;\n",
    )
    .unwrap();

    let (code, report) = run(
        project.path(),
        &["test", "run", "--since", "HEAD", "--json"],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["ok"], true);
    assert_eq!(report["runs"][0]["command"], "npm run test");
    assert_eq!(report["runs"][0]["passed"], 3);
    assert_eq!(report["runs"][0]["failed"], 0);
}

#[test]
fn the_run_command_executes_a_required_full_suite() {
    let project = fixture();
    fs::write(
        project.path().join("package.json"),
        r#"{"name":"impact-fixture","private":true,"scripts":{"test":"node test.js"},"description":"changed"}"#,
    )
    .unwrap();

    let (code, report) = run(
        project.path(),
        &["test", "run", "--since", "HEAD", "--json"],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["ok"], true);
    assert_eq!(report["full_runs"][0]["command"], "npm run test");
    assert_eq!(report["full_runs"][0]["passed"], 3);
}

#[test]
fn a_failed_selected_target_is_nonzero_and_stops() {
    let project = fixture();
    fs::write(
        project.path().join("test.js"),
        "console.error('1 failed'); process.exit(1);\n",
    )
    .unwrap();
    fs::write(
        project.path().join("src/index.js"),
        "export const two = 2;\n",
    )
    .unwrap();

    let (code, report) = run(
        project.path(),
        &["test", "run", "--since", "HEAD", "--json"],
    );
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["ok"], false);
    assert_eq!(report["stopped"], true);
    assert_eq!(report["runs"].as_array().unwrap().len(), 1);
    assert!(report["full_runs"].as_array().unwrap().is_empty());
}

#[test]
fn an_explicit_changed_file_works_without_consulting_git() {
    let project = fixture();
    let (code, plan) = run(
        project.path(),
        &["test", "plan", "--changed-file", "src/index.js", "--json"],
    );
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["changed"][0]["path"], "src/index.js");
    assert_eq!(plan["targets"][0]["label"], "npm run test");
}
