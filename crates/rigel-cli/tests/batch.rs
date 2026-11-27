use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn repository(parent: &TempDir, name: &str) -> PathBuf {
    let path = parent.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&path)
        .output()
        .expect("git init starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

fn scripted_model(parent: &TempDir, cost_usd: f64) -> PathBuf {
    let path = parent.path().join("script.json");
    std::fs::write(
        &path,
        format!(r#"{{"steps":[{{"text":"done","usage":{{"cost_usd":{cost_usd}}}}}]}}"#),
    )
    .unwrap();
    path
}

fn run_batch(repositories: &[PathBuf], script: &Path, cap_cents: i64, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rigel"));
    command
        .arg("batch")
        .arg("--prompt")
        .arg("report on the repository")
        .arg("--cap-cents")
        .arg(cap_cents.to_string())
        .arg("--script")
        .arg(script);
    if json {
        command.arg("--json");
    }
    command.args(repositories);
    command.output().expect("rigel batch starts")
}

fn reports(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value.get("batch_report").cloned())
        .collect()
}

fn stop_report(output: &Output) -> Option<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|value| value.get("batch_stop").cloned())
}

#[test]
fn emits_a_report_for_each_repository() {
    let fixture = tempfile::tempdir().unwrap();
    let repos = [
        repository(&fixture, "one"),
        repository(&fixture, "two"),
        repository(&fixture, "three"),
    ];
    let script = scripted_model(&fixture, 0.005);

    let output = run_batch(&repos, &script, 3, true);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let reports = reports(&output);
    assert_eq!(reports.len(), repos.len());
    for (report, repo) in reports.iter().zip(repos) {
        assert_eq!(
            report["repository"],
            repo.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(report["status"], "completed");
        assert_eq!(report["spend_micros"], 5_000);
    }
    assert!(stop_report(&output).is_none());
}

#[test]
fn one_spend_cap_stops_before_starting_the_remaining_repositories() {
    let fixture = tempfile::tempdir().unwrap();
    let repos = [
        repository(&fixture, "first"),
        repository(&fixture, "second"),
        repository(&fixture, "third"),
    ];
    let script = scripted_model(&fixture, 0.01);

    let output = run_batch(&repos, &script, 1, true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(reports(&output).len(), 1);
    let stop = stop_report(&output).expect("cap stop report");
    let remaining: Vec<String> = repos[1..]
        .iter()
        .map(|repo| repo.canonicalize().unwrap().display().to_string())
        .collect();
    assert_eq!(stop["reason"], "spend cap reached");
    assert_eq!(stop["spent_cents"], 1);
    assert_eq!(stop["remaining_repositories"], serde_json::json!(remaining));
}

#[test]
fn cap_stop_prints_remaining_repositories_in_input_order() {
    let fixture = tempfile::tempdir().unwrap();
    let repos = [
        repository(&fixture, "z-last-alphabetically"),
        repository(&fixture, "a-first-alphabetically"),
        repository(&fixture, "middle"),
    ];
    let script = scripted_model(&fixture, 0.01);

    let output = run_batch(&repos, &script, 1, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = repos[1].canonicalize().unwrap().display().to_string();
    let second = repos[2].canonicalize().unwrap().display().to_string();
    let first_position = stdout.find(&first).expect("first pending repo is printed");
    let second_position = stdout
        .find(&second)
        .expect("second pending repo is printed");
    assert!(
        first_position < second_position,
        "remaining list changed order: {stdout}"
    );
    assert!(stdout.contains("remaining repositories (input order):"));
}
