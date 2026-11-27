use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

fn run(project: &Path, script: &Path, dry_run: bool, route: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rigel"));
    command
        .args([
            "agent",
            "run",
            "--project",
            project.to_str().unwrap(),
            "--agent",
            "native",
            "--prompt",
            "summarize the project",
            "--provider",
            "omniroute",
            "--model",
            "test-model",
            "--script",
            script.to_str().unwrap(),
            "--approval",
            "deny",
            "--max-steps",
            "5",
            "--json",
        ])
        .env("RIGEL_OMNIROUTE_URL", route);
    if dry_run {
        command.arg("--dry-run");
    }
    command.output().expect("rigel agent run succeeds")
}

fn last_json_with_plan(output: &Output) -> Value {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("plan").is_some())
        .expect("command reports its run plan")
}

#[test]
fn dry_run_reports_the_real_run_plan_without_opening_a_provider_socket() {
    let project = tempdir().unwrap();
    let script_file = project.path().join("script.json");
    std::fs::write(&script_file, r#"{"steps":[{"text":"done"}]}"#).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let route = format!("http://{}/v1", listener.local_addr().unwrap());

    let dry = run(project.path(), &script_file, true, &route);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_report = last_json_with_plan(&dry);
    assert_eq!(dry_report["dry_run"], true);
    assert_eq!(dry_report["plan"]["route"]["provider"], "omniroute");
    assert_eq!(dry_report["plan"]["route"]["model"], "test-model");
    assert_eq!(dry_report["plan"]["route"]["base_url"], route);
    assert_eq!(dry_report["plan"]["agent"], "native");
    assert_eq!(dry_report["plan"]["max_steps"], 5);
    assert!(
        dry_report["plan"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "read_file")
    );
    assert_eq!(dry_report["plan"]["required_approvals"]["commands"], "deny");
    assert!(
        dry_report["plan"]["prompt_budget"]["input_tokens"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );

    let real = run(project.path(), &script_file, false, &route);
    assert!(
        real.status.success(),
        "{}",
        String::from_utf8_lossy(&real.stderr)
    );
    let real_report = last_json_with_plan(&real);
    assert_eq!(real_report["dry_run"], false);
    assert_eq!(dry_report["plan"], real_report["plan"]);
}
