//! `rigel test flakes` as a program: a real script whose result changes between runs.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn rigel(project: &Path, args: &[&str]) -> (i32, Value) {
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
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "not json: {error}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

fn fixture(test_script: &str) -> TempDir {
    let project = TempDir::new().unwrap();
    fs::write(
        project.path().join("package.json"),
        format!(
            r#"{{"name":"flake-fixture","private":true,"scripts":{{"test":"node {test_script}"}}}}"#
        ),
    )
    .unwrap();
    project
}

#[test]
fn an_alternating_test_is_reported_as_flaky() {
    let project = fixture("flaky.js");
    fs::write(
        project.path().join("flaky.js"),
        r#"
const fs = require('fs');
const n = fs.existsSync('count.txt') ? Number(fs.readFileSync('count.txt', 'utf8')) : 0;
fs.writeFileSync('count.txt', String(n + 1));
if (n % 2 === 0) {
  console.log('test alpha ... ok');
} else {
  console.log('test alpha ... FAILED');
  process.exit(1);
}
"#,
    )
    .unwrap();

    let (code, report) = rigel(
        project.path(),
        &[
            "test", "flakes", "--runs", "4", "--json", "--", "node", "flaky.js",
        ],
    );
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["total_runs"], 4);
    assert_eq!(report["runs"].as_array().unwrap().len(), 4);
    assert_eq!(report["flaky"][0], "alpha");
    assert_eq!(report["all_ok"], false);
    assert_eq!(report["aggregate_flaky"], false);
    let alpha = report["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|test| test["name"] == "alpha")
        .unwrap();
    assert_eq!(alpha["passed"], 2);
    assert_eq!(alpha["failed"], 2);
}

#[test]
fn a_stable_test_is_clean_and_the_detected_suite_can_be_used() {
    let project = fixture("stable.js");
    fs::write(
        project.path().join("stable.js"),
        "console.log('test alpha ... ok');\n",
    )
    .unwrap();

    let (code, report) = rigel(project.path(), &["test", "flakes", "--runs", "3", "--json"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["command"], "npm test");
    assert_eq!(report["flaky"].as_array().unwrap().len(), 0);
    assert_eq!(report["all_ok"], true);
    assert_eq!(report["total_runs"], 3);
}

#[test]
fn a_repeat_count_outside_the_range_is_an_error() {
    let project = fixture("stable.js");
    fs::write(
        project.path().join("stable.js"),
        "console.log('test alpha ... ok');\n",
    )
    .unwrap();
    let (code, report) = rigel(project.path(), &["test", "flakes", "--runs", "1", "--json"]);
    assert_eq!(code, 1, "{report}");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("between 2 and 10")
    );
}
