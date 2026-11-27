//! Roadmap 190: the cold-start budget.
//!
//! The probe runs the same `startup_ready` step the window runs before it shows itself, in the
//! real `rigel-app` binary. A launch is interactive when the control API answers, so the clock
//! starts at spawn and stops at the probe's one JSON line. The median of several launches must be
//! under two seconds, and a median more than ten percent above the checked-in baseline fails with
//! both numbers in the message. `RIGEL_STARTUP_BASELINE_MS` can supply a machine-specific baseline.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The budget from the roadmap: median launch-to-interactive on the reference machine.
pub const BUDGET_MS: u128 = 2_000;
const SAMPLES: usize = 3;
const START_TIMEOUT_MS: u64 = 10_000;

/// Lower median for an even sample count: a slow outlier cannot make a run look faster.
fn median(samples: &mut [u128]) -> u128 {
    assert!(!samples.is_empty(), "a median needs a sample");
    samples.sort_unstable();
    let middle = samples.len() / 2;
    if samples.len() % 2 == 1 {
        samples[middle]
    } else {
        (samples[middle - 1] + samples[middle]) / 2
    }
}

fn report(median_ms: u128, baseline_ms: Option<u128>) -> String {
    let baseline = baseline_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| "not supplied".to_string());
    format!(
        "cold-start baseline {baseline} ms · current median {median_ms} ms · budget {BUDGET_MS} ms"
    )
}

/// Both gates in one place: the absolute budget and a relative regression against the baseline.
fn check(median_ms: u128, baseline_ms: Option<u128>) -> Result<String, String> {
    let line = report(median_ms, baseline_ms);
    if median_ms >= BUDGET_MS {
        return Err(format!("{line} — over the {BUDGET_MS} ms budget"));
    }
    if let Some(baseline) = baseline_ms
        && median_ms > baseline.saturating_mul(110) / 100
    {
        return Err(format!("{line} — more than 10% slower than the baseline"));
    }
    Ok(line)
}

/// Killed on drop: a failed assertion must not leave a probe process behind.
struct Probe(Child);

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The checked-in figure is the reference machine's cold-start median with headroom; a probe
/// that finds the control API already up measures well below it and is not a regression.
fn baseline_ms() -> Option<u128> {
    if let Ok(value) = std::env::var("RIGEL_STARTUP_BASELINE_MS") {
        return value.parse().ok();
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/startup-baseline.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("median_ms")?.as_u64().map(u128::from)
}

/// One launch, timed from spawn to the probe's ready line. Returns the internally measured ms
/// from the child as well, so a clock skew in the test process cannot hide a regression.
fn launch(data_dir: &Path, project: &Path) -> u128 {
    let started = Instant::now();
    let mut child = Probe(
        Command::new(env!("CARGO_BIN_EXE_rigel-app"))
            .args([
                "startup",
                "--data-dir",
                &data_dir.to_string_lossy(),
                "--project",
                &project.to_string_lossy(),
                "--timeout-ms",
                &START_TIMEOUT_MS.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the window binary starts in startup mode"),
    );
    let stdout = child.0.stdout.take().expect("probe stdout is piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_| line)
            .map_err(|error| error.to_string());
        let _ = sender.send(read);
    });
    let line = receiver
        .recv_timeout(Duration::from_secs(START_TIMEOUT_MS + 10))
        .expect("the startup probe answered")
        .expect("the startup probe wrote a line");
    let value: Value = serde_json::from_str(&line).unwrap_or_else(|error| {
        panic!("the startup probe wrote one JSON line: {line:?} ({error})")
    });
    assert_eq!(
        value["api_ready"], true,
        "the control API was not ready: {line}"
    );
    let _ = child.0.wait();
    started.elapsed().as_millis()
}

#[test]
fn the_budget_and_regression_gates_report_both_numbers() {
    assert_eq!(median(&mut [30, 10, 20]), 20);
    assert_eq!(median(&mut [10, 20]), 15);

    let report = check(1_050, Some(1_000)).expect("within budget and within 10%");
    assert!(report.contains("baseline 1000 ms"), "{report}");
    assert!(report.contains("median 1050 ms"), "{report}");
    let regression = check(1_101, Some(1_000)).unwrap_err();
    assert!(regression.contains("more than 10%"), "{regression}");
    assert!(
        regression.contains("baseline 1000") && regression.contains("median 1101"),
        "{regression}"
    );
    let over = check(2_001, Some(1_000)).unwrap_err();
    assert!(over.contains("budget"), "{over}");
    assert!(
        check(1_900, None).is_ok(),
        "no baseline still enforces the budget"
    );
}

#[test]
fn launch_to_interactive_stays_within_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&project).unwrap();

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        samples.push(launch(&data, &project));
    }
    let current = median(&mut samples);
    let line = check(current, baseline_ms()).unwrap_or_else(|error| panic!("{error}"));
    println!("{line}");
}
