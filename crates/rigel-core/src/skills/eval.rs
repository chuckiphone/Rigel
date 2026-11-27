//! Golden task evaluation for skills before installation or update.
//!
//! A caller supplies the task executor because running a language-model task belongs to the
//! application, not the offline skills index. The harness owns the stable case format, checks,
//! and failure report so every install/update surface applies the same gate.

use crate::agent::model::{ChatMessage, ChatRequest, Delta, DeltaSink, Model};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

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
#[async_trait]
pub trait GoldenTaskRunner: Send + Sync {
    async fn run(&self, skill_id: &str, skill_body: &str, prompt: &str) -> Result<String, String>;
}

/// A production runner that asks the configured chat model to perform the task with the skill.
/// Tools are intentionally omitted: evals are deterministic, local checks of model behavior.
pub struct ModelTaskRunner {
    model: Arc<dyn Model>,
    model_id: String,
    effort: Option<String>,
}

impl ModelTaskRunner {
    pub fn new(model: Arc<dyn Model>, model_id: impl Into<String>, effort: Option<String>) -> Self {
        Self {
            model,
            model_id: model_id.into(),
            effort,
        }
    }
}

#[async_trait]
impl GoldenTaskRunner for ModelTaskRunner {
    async fn run(&self, _skill_id: &str, skill_body: &str, prompt: &str) -> Result<String, String> {
        let request = ChatRequest {
            model: self.model_id.clone(),
            messages: vec![
                ChatMessage::system(format!("Apply this skill's instructions:\n\n{skill_body}")),
                ChatMessage::user(prompt),
            ],
            tools: Vec::new(),
            max_tokens: Some(1024),
            temperature: 0.0,
            effort: self.effort.clone(),
        };
        let mut output = String::new();
        let mut sink = |delta| {
            if let Delta::Text(text) = delta {
                output.push_str(&text);
            }
        };
        self.model
            .chat(
                request,
                &CancellationToken::new(),
                &mut sink as &mut DeltaSink<'_>,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(output)
    }
}

/// JSON persistence for per-skill golden tasks. The file is independent of the repository being
/// indexed, so refresh and install use the same locally authored suite.
#[derive(Debug, Clone)]
pub struct GoldenSuiteStore {
    path: PathBuf,
}

impl GoldenSuiteStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("skill-evals.json"),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(
        &self,
    ) -> Result<std::collections::HashMap<String, Vec<GoldenTask>>, EvalStoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|error| EvalStoreError::Invalid(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(error) => Err(EvalStoreError::Io(error.to_string())),
        }
    }
    pub fn save(
        &self,
        suites: &std::collections::HashMap<String, Vec<GoldenTask>>,
    ) -> Result<(), EvalStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EvalStoreError::Io(e.to_string()))?;
        }
        let text = serde_json::to_vec_pretty(suites)
            .map_err(|e| EvalStoreError::Invalid(e.to_string()))?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| EvalStoreError::Io(error.to_string()))?;
        temporary
            .write_all(&text)
            .map_err(|error| EvalStoreError::Io(error.to_string()))?;
        temporary
            .persist(&self.path)
            .map(|_| ())
            .map_err(|error| EvalStoreError::Io(error.error.to_string()))
    }
    pub fn set(&self, id: &str, tasks: Vec<GoldenTask>) -> Result<(), EvalStoreError> {
        if tasks.is_empty() {
            return Err(EvalStoreError::Invalid(
                "a suite must contain at least one task".into(),
            ));
        }
        if tasks
            .iter()
            .any(|task| task.name.trim().is_empty() || task.prompt.trim().is_empty())
        {
            return Err(EvalStoreError::Invalid(
                "task names and prompts must not be empty".into(),
            ));
        }
        let mut suites = self.load()?;
        suites.insert(id.to_owned(), tasks);
        self.save(&suites)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvalStoreError {
    #[error("could not read or write golden task suites: {0}")]
    Io(String),
    #[error("invalid golden task suite file: {0}")]
    Invalid(String),
}

/// Run all golden tasks and preserve enough output to explain a failed gate.
pub async fn evaluate<R: GoldenTaskRunner + ?Sized>(
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
            .await
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

    #[async_trait]
    impl GoldenTaskRunner for FixedRunner {
        async fn run(
            &self,
            _skill_id: &str,
            _skill_body: &str,
            _prompt: &str,
        ) -> Result<String, String> {
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

    #[tokio::test]
    async fn successful_golden_task_produces_a_passing_report() {
        let report = evaluate(
            "repo:files",
            "instructions",
            &[task()],
            &FixedRunner("path: README.md"),
        )
        .await
        .unwrap();
        assert!(report.passed);
        assert!(report.tasks[0].diff.is_empty());
        require_pass(&report).unwrap();
    }

    #[tokio::test]
    async fn failed_task_names_the_task_and_includes_the_output_diff() {
        let report = evaluate(
            "repo:files",
            "instructions",
            &[task()],
            &FixedRunner("I could not find it"),
        )
        .await
        .unwrap();
        let error = require_pass(&report).unwrap_err().to_string();
        assert!(error.contains("find a file"));
        assert!(error.contains("- path: README.md"));
        assert!(error.contains("+ I could not find it"));
    }

    #[tokio::test]
    async fn empty_eval_suite_fails_closed() {
        assert_eq!(
            evaluate("repo:files", "instructions", &[], &FixedRunner("ok"))
                .await
                .unwrap_err(),
            EvalError::NoTasks("repo:files".into())
        );
    }

    #[tokio::test]
    async fn production_runner_executes_a_scripted_model_task() {
        let model = Arc::new(
            crate::agent::ScriptedModel::from_json(r#"{"steps":[{"text":"path: README.md"}]}"#)
                .unwrap(),
        );
        let runner = ModelTaskRunner::new(model, "script", None);
        let report = evaluate("repo:files", "locate files", &[task()], &runner)
            .await
            .unwrap();
        assert!(report.passed);
    }

    #[test]
    fn suite_store_persists_and_reloads_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoldenSuiteStore::new(temp.path());
        store.set("owner:skill", vec![task()]).unwrap();
        let mut revised = task();
        revised.name = "updated task".into();
        store.set("owner:skill", vec![revised.clone()]).unwrap();
        assert_eq!(
            store.load().unwrap().get("owner:skill"),
            Some(&vec![revised])
        );
        assert!(store.path().exists());
    }
}
