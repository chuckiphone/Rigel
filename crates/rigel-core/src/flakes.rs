//! Roadmap #38: find a test whose outcome changes between identical runs.
//!
//! The hunter runs one explicit plan several times and compares per-test result lines. It is
//! deliberately a repeat detector, not a randomizer: Cargo and most runners have stable order,
//! so a test that passes and fails across repeated runs is a timing, isolation or environment
//! flake. Order dependence needs a runner that can shuffle and is reported as not covered rather
//! than guessed at. A run whose summary disagrees but whose output names no tests is reported as
//! an unattributed aggregate flake.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use thiserror::Error;

use crate::testrun::{self, TestPlan, TestRun, TestRunError};

pub const MIN_RUNS: usize = 2;
pub const DEFAULT_RUNS: usize = 3;
pub const MAX_RUNS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestOutcome {
    pub name: String,
    pub passed: usize,
    pub failed: usize,
    /// Runs in which this name appeared.
    pub runs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlakeRun {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub passed: usize,
    pub failed: usize,
    pub timed_out: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlakeReport {
    pub command: String,
    pub total_runs: usize,
    pub runs: Vec<FlakeRun>,
    pub tests: Vec<TestOutcome>,
    /// Names that both passed and failed across runs.
    pub flaky: Vec<String>,
    /// Names that did not appear in every run; a filtered or conditional test can do this.
    pub unstable_set: Vec<String>,
    /// Runs disagreed but no per-test line could attribute the disagreement.
    pub aggregate_flaky: bool,
    pub all_ok: bool,
    pub notes: Vec<String>,
}

impl FlakeReport {
    /// True when repeated runs produced no inconsistency.
    pub fn stable(&self) -> bool {
        self.flaky.is_empty() && self.unstable_set.is_empty() && !self.aggregate_flaky
    }

    /// The exit-code predicate: a flake or an unattributable disagreement fails the check.
    pub fn failed(&self) -> bool {
        !self.stable()
    }
}

#[derive(Debug, Error)]
pub enum FlakeError {
    #[error("repeat the run between {MIN_RUNS} and {MAX_RUNS} times; {0} is outside that range")]
    Runs(usize),
    #[error(transparent)]
    Test(#[from] TestRunError),
}

/// Run `plan` `total` times, sequentially, and compare the outcomes.
pub async fn hunt(
    project: &std::path::Path,
    plan: &TestPlan,
    total: usize,
) -> Result<FlakeReport, FlakeError> {
    if !(MIN_RUNS..=MAX_RUNS).contains(&total) {
        return Err(FlakeError::Runs(total));
    }
    let mut runs = Vec::with_capacity(total);
    for _ in 0..total {
        runs.push(testrun::run_plan(project, plan).await?);
    }
    Ok(summarize(plan, runs))
}

/// Pure comparison over already-finished runs, so parser and report behavior can be pinned
/// without starting a process.
pub fn summarize(plan: &TestPlan, runs: Vec<TestRun>) -> FlakeReport {
    let total = runs.len();
    let mut counts: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut timed_out = false;
    let mut flake_runs = Vec::new();

    for run in &runs {
        timed_out |= run.timed_out;
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        for (name, passed) in parse_outcomes(&run.output) {
            seen.entry(name)
                .and_modify(|previous| *previous &= passed)
                .or_insert(passed);
        }
        for (name, passed) in seen {
            let entry = counts.entry(name).or_insert((0, 0, 0));
            if passed {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
            entry.2 += 1;
        }
        flake_runs.push(FlakeRun {
            ok: run.ok,
            exit_code: run.exit_code,
            passed: run.passed,
            failed: run.failed,
            timed_out: run.timed_out,
            duration_ms: run.duration_ms,
        });
    }

    let tests: Vec<TestOutcome> = counts
        .into_iter()
        .map(|(name, (passed, failed, runs))| TestOutcome {
            name,
            passed,
            failed,
            runs,
        })
        .collect();
    let flaky: Vec<String> = tests
        .iter()
        .filter(|test| test.passed > 0 && test.failed > 0)
        .map(|test| test.name.clone())
        .collect();
    let unstable_set: Vec<String> = tests
        .iter()
        .filter(|test| test.runs < total)
        .map(|test| test.name.clone())
        .collect();
    let any_ok = flake_runs.iter().any(|run| run.ok);
    let any_failed = flake_runs.iter().any(|run| !run.ok);
    let aggregate_flaky = any_ok && any_failed && flaky.is_empty();

    let mut notes = Vec::new();
    if aggregate_flaky {
        notes.push("run outcomes differed but no per-test result line named the test".to_string());
    }
    if !unstable_set.is_empty() {
        notes.push("some tests did not appear in every run".to_string());
    }
    if timed_out {
        notes.push("at least one run timed out".to_string());
    }
    if flaky.is_empty() && !aggregate_flaky && tests.is_empty() && !any_ok {
        notes.push("the runner produced no per-test results and did not pass".to_string());
    }
    notes.push("order dependence is not tested: the runner would need a shuffle mode".to_string());

    FlakeReport {
        command: plan.label.clone(),
        total_runs: total,
        runs: flake_runs,
        tests,
        flaky,
        unstable_set,
        aggregate_flaky,
        all_ok: runs.iter().all(|run| run.ok),
        notes,
    }
}

/// Read per-test result lines from Rust, Go, Jest/Vitest and Playwright-style output.
/// Summary lines such as `test result: FAILED. 2 passed` deliberately produce nothing.
pub fn parse_outcomes(output: &str) -> Vec<(String, bool)> {
    let mut outcomes = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("test ")
            && let Some((name, status)) = rest.split_once(" ... ")
        {
            let status = status.trim_start();
            if status.starts_with("ok") {
                outcomes.push((name.trim().to_string(), true));
            } else if status.starts_with("FAILED") {
                outcomes.push((name.trim().to_string(), false));
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("--- PASS: ") {
            outcomes.push((clean_go_name(rest), true));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("--- FAIL: ") {
            outcomes.push((clean_go_name(rest), false));
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix('✓')
            .or_else(|| trimmed.strip_prefix('✔'))
        {
            if let Some(name) = clean_symbol_name(rest) {
                outcomes.push((name, true));
            }
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix('✗')
            .or_else(|| trimmed.strip_prefix('✘'))
            .or_else(|| trimmed.strip_prefix('×'))
            .or_else(|| trimmed.strip_prefix('✕'))
        {
            if let Some(name) = clean_symbol_name(rest) {
                outcomes.push((name, false));
            }
            continue;
        }
    }
    outcomes
}

fn clean_go_name(rest: &str) -> String {
    rest.split(" (").next().unwrap_or(rest).trim().to_string()
}

fn clean_symbol_name(rest: &str) -> Option<String> {
    let mut name = rest.trim();
    // Playwright prefixes each row with its index; the index is not part of the test name.
    name = name.trim_start_matches(|character: char| character.is_ascii_digit());
    name = name.trim();
    // Jest and Playwright append a duration, which changes on every run.
    if let Some(open) = name.rfind(" (")
        && name.ends_with(')')
    {
        let inside = &name[open + 2..name.len() - 1];
        if inside.ends_with("ms") || inside.ends_with('s') {
            name = name[..open].trim_end();
        }
    }
    (!name.is_empty()).then(|| name.to_string())
}

/// Unique names from several runs, preserving first-seen order; useful for a quarantine list.
pub fn names(report: &FlakeReport) -> BTreeSet<String> {
    report
        .flaky
        .iter()
        .chain(report.unstable_set.iter())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> TestPlan {
        TestPlan {
            program: "cargo".into(),
            args: vec!["test".into()],
            label: "cargo test".into(),
        }
    }

    fn run(output: &str, ok: bool) -> TestRun {
        TestRun {
            command: "cargo test".into(),
            ok,
            exit_code: Some(if ok { 0 } else { 1 }),
            passed: 0,
            failed: 0,
            output: output.into(),
            truncated: false,
            timed_out: false,
            duration_ms: 1,
        }
    }

    #[test]
    fn parses_rust_go_and_javascript_result_lines() {
        let output = "\
test demo::one ... ok
test demo::two ... FAILED
test result: FAILED. 1 passed; 1 failed
--- PASS: TestGo (0.01s)
--- FAIL: TestGoBad (0.02s)
  ✓ adds numbers (5 ms)
  ✗ rejects a bad value (1.2s)
✓  1 [chromium] › tests/session.spec.ts:12:3 › opens the session (3s)
";
        let outcomes = parse_outcomes(output);
        assert!(outcomes.contains(&("demo::one".into(), true)));
        assert!(outcomes.contains(&("demo::two".into(), false)));
        assert!(outcomes.contains(&("TestGo".into(), true)));
        assert!(outcomes.contains(&("TestGoBad".into(), false)));
        assert!(outcomes.contains(&("adds numbers".into(), true)));
        assert!(outcomes.contains(&("rejects a bad value".into(), false)));
        assert!(
            outcomes
                .iter()
                .any(|(name, passed)| { name.contains("opens the session") && *passed })
        );
        assert_eq!(outcomes.len(), 7, "{outcomes:?}");
    }

    #[test]
    fn a_name_that_passes_and_fails_is_flaky() {
        let report = summarize(
            &plan(),
            vec![
                run("test alpha ... ok\ntest beta ... ok\n", true),
                run("test alpha ... FAILED\ntest beta ... ok\n", false),
            ],
        );
        assert_eq!(report.flaky, vec!["alpha".to_string()]);
        assert!(report.tests.iter().any(|test| {
            test.name == "alpha" && test.passed == 1 && test.failed == 1 && test.runs == 2
        }));
        assert!(!report.aggregate_flaky);
        assert!(!report.stable());
    }

    #[test]
    fn a_disagreement_without_per_test_lines_is_aggregate() {
        let report = summarize(
            &plan(),
            vec![run("2 passed\n", true), run("1 failed\n", false)],
        );
        assert!(report.aggregate_flaky);
        assert!(report.flaky.is_empty());
        assert!(report.notes.iter().any(|note| note.contains("no per-test")));
    }

    #[test]
    fn a_stable_suite_is_not_flagged() {
        let report = summarize(
            &plan(),
            vec![
                run("test alpha ... ok\ntest beta ... ok\n", true),
                run("test alpha ... ok\ntest beta ... ok\n", true),
            ],
        );
        assert!(report.stable());
        assert!(report.all_ok);
        assert!(report.flaky.is_empty());
        assert!(report.unstable_set.is_empty());
    }

    #[test]
    fn a_test_missing_from_a_run_is_an_unstable_set() {
        let report = summarize(
            &plan(),
            vec![
                run("test alpha ... ok\ntest beta ... ok\n", true),
                run("test alpha ... ok\n", true),
            ],
        );
        assert_eq!(report.unstable_set, vec!["beta".to_string()]);
        assert!(report.notes.iter().any(|note| note.contains("every run")));
        assert!(report.failed());
    }

    #[test]
    fn duplicate_result_lines_in_one_run_do_not_inflate_counts() {
        let report = summarize(
            &plan(),
            vec![run(
                "test alpha ... ok\ntest alpha ... ok\ntest alpha ... ok\n",
                true,
            )],
        );
        assert_eq!(report.tests[0].passed, 1);
        assert_eq!(report.tests[0].runs, 1);
    }

    #[test]
    fn repeat_count_is_bounded() {
        let project = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(matches!(
            runtime.block_on(hunt(project.path(), &plan(), 1)),
            Err(FlakeError::Runs(1))
        ));
        assert!(matches!(
            runtime.block_on(hunt(project.path(), &plan(), MAX_RUNS + 1)),
            Err(FlakeError::Runs(_))
        ));
    }
}
