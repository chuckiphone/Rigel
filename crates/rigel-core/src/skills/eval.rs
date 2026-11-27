//! Golden task evaluation for skills before installation or update.
//!
//! A caller supplies the task executor because running a language-model task belongs to the
//! application, not the offline skills index. The harness owns the stable case format, checks,
//! and failure report so every install/update surface applies the same gate.

use serde::{Deserialize, Serialize};

/// One repeatable task used to check that a skill still produces its promised behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldenTask {
    pub name: String,
    pub prompt: String,
    /// Each marker must occur in the executor's output for this case to pass.
    #[serde(default)]
    pub expected_contains: Vec<String>,
}

/// The result for one golden task, including a compact expected/actual diff on failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResult {
    pub name: String,
    pub passed: bool,
    pub diff: String,
}

/// A serializable report suitable for CLI or UI display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalReport {
    pub skill_id: String,
    pub passed: bool,
    pub tasks: Vec<TaskResult>,
}

/// Execute one task using the host's chosen agent or test runner.
pub trait GoldenTaskRunner {
    fn run(&self, skill_id: &str, skill_body: &str, prompt: &str) -> Result<String, String>;
}

/// Run all golden tasks and preserve enough output to explain a failed gate.
pub fn evaluate<R: GoldenTaskRunner>(
    skill_id: &str,
    skill_body: &str,
    tasks: &[GoldenTask],
    runner: &R,
) -> Result<EvalReport, EvalError> {
    if tasks.is_empty() {
        return Err(EvalError::NoTasks(skill_id.to_owned()));
    }

    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        let actual = runner
            .run(skill_id, skill_body, &task.prompt)
            .map_err(|detail| EvalError::Runner {
                task: task.name.clone(),
                detail,
            })?;
        let missing: Vec<&str> = task
            .expected_contains
            .iter()
            .map(String::as_str)
            .filter(|marker| !actual.contains(marker))
            .collect();
        let passed = missing.is_empty();
        let diff = if passed {
            String::new()
        } else {
            format_diff(&missing, &actual)
        };
        results.push(TaskResult {
            name: task.name.clone(),
            passed,
            diff,
        });
    }
    let passed = results.iter().all(|result| result.passed);
    Ok(EvalReport {
        skill_id: skill_id.to_owned(),
        passed,
        tasks: results,
    })
}

/// Refuse activation when a task failed, naming both the task and its output diff.
pub fn require_pass(report: &EvalReport) -> Result<(), EvalError> {
    if report.passed {
        return Ok(());
    }
    let failures = report
        .tasks
        .iter()
        .filter(|task| !task.passed)
        .map(|task| format!("task `{}` failed:\n{}", task.name, task.diff))
        .collect::<Vec<_>>()
        .join("\n");
    Err(EvalError::Failed {
        skill_id: report.skill_id.clone(),
        failures,
    })
}

fn format_diff(missing: &[&str], actual: &str) -> String {
    let mut diff = String::from("Expected markers missing from actual output:\n");
    for marker in missing {
        diff.push_str("- ");
        diff.push_str(marker);
        diff.push('\n');
    }
    diff.push_str("Actual output:\n+\n");
    for line in actual.lines().take(20) {
        diff.push_str("+ ");
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("skill {0} has no golden tasks; refusing to enable it without an eval")]
    NoTasks(String),
    #[error("golden task `{task}` could not run: {detail}")]
    Runner { task: String, detail: String },
    #[error("skill {skill_id} failed golden evaluation:\n{failures}")]
    Failed { skill_id: String, failures: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedRunner(&'static str);

    impl GoldenTaskRunner for FixedRunner {
        fn run(&self, _skill_id: &str, _skill_body: &str, _prompt: &str) -> Result<String, String> {
            Ok(self.0.to_owned())
        }
    }

    fn task() -> GoldenTask {
        GoldenTask {
            name: "find a file".into(),
            prompt: "Locate the requested file".into(),
            expected_contains: vec!["path: README.md".into()],
        }
    }

    #[test]
    fn successful_golden_task_produces_a_passing_report() {
        let report = evaluate(
            "repo:files",
            "instructions",
            &[task()],
            &FixedRunner("path: README.md"),
        )
        .unwrap();
        assert!(report.passed);
        assert!(report.tasks[0].diff.is_empty());
        require_pass(&report).unwrap();
    }

    #[test]
    fn failed_task_names_the_task_and_includes_the_output_diff() {
        let report = evaluate(
            "repo:files",
            "instructions",
            &[task()],
            &FixedRunner("I could not find it"),
        )
        .unwrap();
        let error = require_pass(&report).unwrap_err().to_string();
        assert!(error.contains("find a file"));
        assert!(error.contains("- path: README.md"));
        assert!(error.contains("+ I could not find it"));
    }

    #[test]
    fn empty_eval_suite_fails_closed() {
        assert_eq!(
            evaluate("repo:files", "instructions", &[], &FixedRunner("ok")).unwrap_err(),
            EvalError::NoTasks("repo:files".into())
        );
    }
}
