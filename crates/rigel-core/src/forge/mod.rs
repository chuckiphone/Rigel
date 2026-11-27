use std::{path::Path, process::Command};

use crate::session::{Session, TurnTranscript};
use serde::Serialize;

pub mod github;
pub mod linear;
pub mod mock;
pub use github::GitHubForge;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PullRequestPlan {
    pub title: String,
    pub body: String,
    pub branch: String,
    pub base: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("gh is not installed or not on PATH")]
    GhMissing,
    #[error("gh is not signed in: {0}")]
    NotSignedIn(String),
    #[error("{0} is not a git repository")]
    NotARepository(String),
    #[error("{0}")]
    Failed(String),
}

/// All operations a forge adapter must support. Implementations cannot omit behavior.
pub trait Forge: Send + Sync {
    fn check(&self) -> Result<String, ForgeError>;
    fn plan(
        &self,
        project: &Path,
        session: &Session,
        turns: &[TurnTranscript],
    ) -> Result<PullRequestPlan, ForgeError>;
    fn create(&self, project: &Path, plan: &PullRequestPlan) -> Result<String, ForgeError>;
}

fn git_output(project: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(project).args(args);
    crate::process::hide(&mut command);
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn build_plan(
    project: &Path,
    session: &Session,
    turns: &[TurnTranscript],
) -> Result<PullRequestPlan, ForgeError> {
    let branch = git_output(project, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok_or_else(|| ForgeError::NotARepository(project.display().to_string()))?;
    if branch == "HEAD" {
        return Err(ForgeError::Failed(
            "the repository is on a detached HEAD; create a branch first".into(),
        ));
    }
    let base = git_output(project, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .map(|name| name.rsplit('/').next().unwrap_or("main").to_string())
        .unwrap_or_else(|| "main".into());
    let latest = turns
        .iter()
        .rev()
        .find_map(|turn| turn.turn.prompt.as_deref())
        .unwrap_or("Rigel session");
    let title = latest
        .lines()
        .next()
        .unwrap_or("Rigel session")
        .trim()
        .chars()
        .take(72)
        .collect::<String>();
    let mut body = format!(
        "Session `{}` — {} turn(s), ${:.4} spent.\n\n### What was asked\n\n",
        session.id,
        turns.len(),
        session.spend_micros as f64 / 1_000_000.0
    );
    for turn in turns {
        if let Some(prompt) = &turn.turn.prompt {
            let first = prompt.lines().next().unwrap_or("").trim();
            if !first.is_empty() {
                body.push_str(&format!("- {first}\n"));
            }
        }
    }
    if let Some(turn_id) = turns.last().map(|turn| turn.turn.id.as_str()) {
        body.push_str(&format!(
            "\n<!-- rigel:session={};turn={} -->\n",
            session.id, turn_id
        ));
    }
    body.push_str("\n### Review\n\nGenerated from Rigel's session transcript. Review the diff; Rigel did not push or open this pull request by itself.\n");
    Ok(PullRequestPlan {
        title,
        body,
        branch,
        base,
    })
}

#[cfg(test)]
mod conformance {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn fixtures() -> (tempfile::TempDir, Session, Vec<TurnTranscript>) {
        let dir = tempfile::TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=T",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["checkout", "-b", "feature"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let session = Session {
            id: "s-1".into(),
            project_root: "demo".into(),
            title: None,
            title_source: None,
            status: crate::session::SessionStatus::Active,
            budget_cents: Some(500),
            spend_cents: 1,
            spend_micros: 1000,
            interventions: 0,
            tasks_done: 1,
            created_at: 1,
            ended_at: None,
            pinned_at: None,
            archived_at: None,
            parent_session_id: None,
            parent_turn_id: None,
        };
        let turn = crate::session::Turn {
            id: "t-1".into(),
            agent_id: "a-1".into(),
            parent_turn_id: None,
            seq: 1,
            status: "completed".into(),
            prompt: Some("fix parser\nadd tests".into()),
            images: Vec::new(),
            started_at: 1,
            completed_at: Some(2),
            usage: None,
            stop_reason: Some("completed".into()),
        };
        (
            dir,
            session,
            vec![TurnTranscript {
                turn,
                items: Vec::new(),
            }],
        )
    }

    fn assert_conforms<F: Forge>(forge: &F, create_result: &str) {
        let (dir, session, turns) = fixtures();
        assert_eq!(forge.check().unwrap(), "forge ready");
        let plan = forge.plan(dir.path(), &session, &turns).unwrap();
        assert_eq!(plan.branch, "feature");
        assert_eq!(plan.title, "fix parser");
        assert!(plan.body.contains("fix parser"));
        assert!(plan.body.contains("<!-- rigel:session=s-1;turn=t-1 -->"));
        assert_eq!(forge.create(dir.path(), &plan).unwrap(), create_result);
    }

    #[test]
    fn shared_conformance_suite_covers_github_and_mock() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let github =
            GitHubForge::with_runner(Box::new(move |_project: Option<&Path>, args: &[&str]| {
                let mut calls = calls.lock().unwrap();
                calls.push(args.join(" "));
                Ok(if args == ["auth", "status"] {
                    "".into()
                } else {
                    "https://example.test/pr/1".into()
                })
            }));
        assert_conforms(&github, "https://example.test/pr/1");
        let mock = mock::MockForge::default();
        assert_conforms(&mock, "https://example.test/pr/1");
    }
}
