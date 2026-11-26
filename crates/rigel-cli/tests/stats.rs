//! Roadmap 196: `rigel stats` reads local rows only. The dead proxy makes any attempted request
//! fail fast, so a successful command is direct evidence that none was made.

use std::path::Path;
use std::process::Command;

use rigel_core::SessionService;
use serde_json::{Value, json};
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(data)
        // A closed local port: any HTTP attempt returns immediately instead of hanging the test.
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .args(args)
        .output()
        .expect("rigel runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

async fn seed(tmp: &TempDir) -> (String, String) {
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&data).unwrap();

    let service = SessionService::open(&project).await.unwrap();
    let session = service.start(Some("metrics"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("fix the parser"))
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "usage",
            "completed",
            &json!({
                "input_tokens": 100,
                "output_tokens": 40,
                "reasoning_tokens": 10,
                "cached_input_tokens": 20,
                "cache_write_tokens": 5,
                "cost_micros": 200,
                "step": 1,
                "duration_ms": 300,
                "queue_ms": 50,
            }),
            &json!({}),
        )
        .await
        .unwrap();
    service
        .append_item(&turn.id, "retry", "completed", &json!({}), &json!({}))
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "usage",
            "completed",
            &json!({ "input_tokens": 10, "output_tokens": 5, "cost_micros": 30 }),
            &json!({}),
        )
        .await
        .unwrap();
    service.finish_turn(&turn.id, "completed").await.unwrap();
    drop(service);

    (
        project.to_string_lossy().into_owned(),
        data.to_string_lossy().into_owned(),
    )
}

#[tokio::test]
async fn stats_json_has_the_documented_shape_without_a_request() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = seed(&tmp).await;
    let (code, stdout) = rigel(
        Path::new(&data),
        &["stats", "--project", &project, "--json"],
    );
    assert_eq!(code, 0, "{stdout}");
    let stats: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["schema"], 1);
    assert_eq!(stats["sessions"], 1);
    assert_eq!(stats["turns"], 1);
    assert_eq!(stats["completed_turns"], 1);
    assert_eq!(stats["steps"], 2);
    assert_eq!(stats["tokens"]["input"], 110);
    assert_eq!(stats["tokens"]["output"], 45);
    assert_eq!(stats["tokens"]["reasoning"], 10);
    assert_eq!(stats["tokens"]["total"], 165);
    assert_eq!(stats["cache"]["hits"], 1);
    assert_eq!(stats["cache"]["read_tokens"], 20);
    assert_eq!(stats["cache"]["write_tokens"], 5);
    assert_eq!(stats["retries"], 1);
    assert_eq!(stats["cost_micros"], 230);
    assert_eq!(stats["model_ms"], 300);
    assert_eq!(stats["queue_ms"], 50);
    for field in [
        "latency_ms",
        "model_ms",
        "queue_ms",
        "tokens",
        "cache",
        "retries",
        "cost_micros",
    ] {
        assert!(stats.get(field).is_some(), "{field} is documented");
    }
    assert!(stats["latency_ms"].get("mean_ms").is_some());
    assert!(stats["latency_ms"].get("max_ms").is_some());
}

#[tokio::test]
async fn stats_human_output_names_every_group_and_says_it_is_local() {
    let tmp = TempDir::new().unwrap();
    let (project, data) = seed(&tmp).await;
    let (code, stdout) = rigel(Path::new(&data), &["stats", "--project", &project]);
    assert_eq!(code, 0, "{stdout}");
    for line in [
        "schema     1",
        "sessions   1",
        "turns      1 (1 completed)",
        "steps      2",
        "latency    total",
        "model/queue 300 ms / 50 ms",
        "tokens     165 (110 in, 45 out, 10 reasoning)",
        "cache      1 hits · 20 read · 5 write",
        "retries    1",
        "spend      230 micros",
        "local only · no request was made",
    ] {
        assert!(stdout.contains(line), "{line} is missing from {stdout}");
    }
    assert!(
        !stdout.trim_start().starts_with('{'),
        "human output is not JSON"
    );
}

#[tokio::test]
async fn an_empty_project_reports_zeroes_not_an_error() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    drop(SessionService::open(&project).await.unwrap());

    let (code, stdout) = rigel(
        &data,
        &["stats", "--project", project.to_str().unwrap(), "--json"],
    );
    assert_eq!(code, 0, "{stdout}");
    let stats: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(stats["schema"], 1);
    assert_eq!(stats["turns"], 0);
    assert_eq!(stats["steps"], 0);
    assert_eq!(stats["latency_ms"]["mean_ms"], 0);
    assert_eq!(stats["tokens"]["total"], 0);
    assert_eq!(stats["cache"]["hits"], 0);
    assert_eq!(stats["retries"], 0);
    assert_eq!(stats["cost_micros"], 0);
}
