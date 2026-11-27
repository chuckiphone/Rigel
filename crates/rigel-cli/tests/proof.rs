//! `rigel proof` as a program: real command evidence, freshness and the test-run handoff.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn rigel(project: &Path, args: &[&str]) -> (i32, Value) {
    // `proof record` uses `last = true`; keep `--project` before the command separator so the
    // command after `--` stays exactly what the caller asked for.
    let split = args
        .iter()
        .position(|arg| *arg == "--")
        .unwrap_or(args.len());
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(&args[..split])
        .arg("--project")
        .arg(project)
        .args(&args[split..])
        .output()
        .expect("rigel runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let versioned: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "not json: {error}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(versioned["schema_version"], 1, "{versioned}");
    let json = versioned.get("data").unwrap_or(&versioned).clone();
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
        r#"{"name":"proof-fixture","private":true,"scripts":{"test":"node test.js"}}"#,
    )
    .unwrap();
    fs::write(project.path().join("test.js"), "console.log('3 passed');\n").unwrap();
    fs::write(
        project.path().join("src/index.js"),
        "export const one = 1;\n",
    )
    .unwrap();
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
fn a_recorded_command_verifies_and_goes_stale_with_the_tree() {
    let project = fixture();
    fs::write(
        project.path().join("src/index.js"),
        "export const two = 2;\n",
    )
    .unwrap();

    let (code, bundle) = rigel(
        project.path(),
        &[
            "proof",
            "record",
            "--since",
            "HEAD",
            "--json",
            "--",
            "node",
            "-e",
            "console.log('3 passed')",
        ],
    );
    assert_eq!(code, 0, "{bundle}");
    assert_eq!(bundle["ok"], true);
    assert_eq!(bundle["files"].as_array().unwrap().len(), 1);
    assert_eq!(bundle["evidence"][0]["passed"], 3);
    assert_eq!(bundle["evidence"][0]["ok"], true);

    let (code, verified) = rigel(project.path(), &["proof", "verify", "--json"]);
    assert_eq!(code, 0, "{verified}");
    assert_eq!(verified[0]["valid"], true);
    assert_eq!(verified[0]["fresh"], true);
    assert_eq!(verified[0]["freshness"], "fresh");
    assert_eq!(verified[0]["ok"], true);

    fs::write(
        project.path().join("src/index.js"),
        "export const three = 3;\n",
    )
    .unwrap();
    let (code, stale) = rigel(project.path(), &["proof", "verify", "--json"]);
    assert_eq!(code, 2, "{stale}");
    assert_eq!(stale[0]["fresh"], false);
    assert_eq!(stale[0]["freshness"], "stale");
    assert_eq!(stale[0]["changed"][0], "src/index.js");
}

#[test]
fn a_docs_only_change_records_a_verified_bundle_without_evidence() {
    let project = fixture();
    fs::write(project.path().join("README.md"), "new words\n").unwrap();
    let (code, report) = rigel(
        project.path(),
        &["test", "run", "--since", "HEAD", "--proof", "--json"],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["ok"], true);
    assert_eq!(report["proof"]["ok"], true);
    assert!(report["proof"]["evidence"].as_array().unwrap().is_empty());

    let (code, verified) = rigel(project.path(), &["proof", "verify", "--json"]);
    assert_eq!(code, 0, "{verified}");
    assert_eq!(verified[0]["fresh"], true);
}

#[test]
fn test_run_with_proof_writes_evidence_and_lists_it() {
    let project = fixture();
    fs::write(
        project.path().join("src/index.js"),
        "export const two = 2;\n",
    )
    .unwrap();

    let (code, report) = rigel(
        project.path(),
        &["test", "run", "--since", "HEAD", "--proof", "--json"],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["ok"], true);
    assert_eq!(report["proof"]["ok"], true);
    assert_eq!(report["proof"]["evidence"].as_array().unwrap().len(), 1);
    assert_eq!(report["proof"]["evidence"][0]["command"], "npm run test");

    let (code, verified) = rigel(project.path(), &["proof", "verify", "--json"]);
    assert_eq!(code, 0, "{verified}");
    assert_eq!(verified[0]["fresh"], true);
    assert_eq!(verified[0]["ok"], true);

    let (code, listed) = rigel(project.path(), &["proof", "list", "--json"]);
    assert_eq!(code, 0, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["evidence"], 1);
}
