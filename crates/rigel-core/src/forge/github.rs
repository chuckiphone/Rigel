use super::{Forge, ForgeError, PullRequestPlan, build_plan};
use crate::session::{Session, TurnTranscript};
use crate::storage::Store;
use serde::Deserialize;
use std::{path::Path, process::Command};

pub trait GhRunner: Send + Sync {
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError>;
}
impl<F> GhRunner for F
where
    F: Fn(Option<&Path>, &[&str]) -> Result<String, ForgeError> + Send + Sync,
{
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError> {
        self(project, args)
    }
}
struct SystemGh;
impl GhRunner for SystemGh {
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError> {
        let mut command = Command::new("gh");
        if let Some(project) = project {
            command.current_dir(project);
        }
        command.args(args);
        crate::process::hide(&mut command);
        let output = command.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ForgeError::GhMissing
            } else {
                ForgeError::Failed(e.to_string())
            }
        })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if args == ["auth", "status"] {
                Err(ForgeError::NotSignedIn(if msg.is_empty() {
                    "run gh auth login".into()
                } else {
                    msg
                }))
            } else {
                Err(ForgeError::Failed(msg))
            }
        }
    }
}
pub struct GitHubForge {
    runner: Box<dyn GhRunner>,
}
impl Default for GitHubForge {
    fn default() -> Self {
        Self {
            runner: Box::new(SystemGh),
        }
    }
}
impl GitHubForge {
    pub fn with_runner(runner: Box<dyn GhRunner>) -> Self {
        Self { runner }
    }

    /// Import pull requests associated with this session, along with their current checks
    /// and review comments. Repeated syncs update the same rows instead of duplicating them.
    pub async fn sync_session(
        &self,
        project: &Path,
        store: &Store,
        session_id: &str,
    ) -> Result<SyncReport, ForgeError> {
        let pool = store.pool();
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await
            .map_err(db_error)?;
        if exists == 0 {
            return Err(ForgeError::Failed(format!(
                "session `{session_id}` was not found"
            )));
        }

        self.check()?;
        let repo = self.runner.run(
            Some(project),
            &[
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "--jq",
                ".nameWithOwner",
            ],
        )?;
        let prs: Vec<RemotePullRequest> = parse_json(&self.runner.run(
            Some(project),
            &[
                "pr",
                "list",
                "--state",
                "all",
                "--json",
                "number,title,url,state,body,headRefName,headRefOid,updatedAt,mergedAt",
            ],
        )?)?;

        let mut report = SyncReport::default();
        for pr in prs {
            let body = pr.body.as_deref().unwrap_or_default();
            let Some(turn_id) = parse_session_turn_marker(body, session_id) else {
                continue;
            };
            let turn_exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM turns t JOIN agents a ON a.id = t.agent_id WHERE t.id = ? AND a.session_id = ?",
            )
            .bind(&turn_id)
            .bind(session_id)
            .fetch_one(pool)
            .await
            .map_err(db_error)?;
            if turn_exists == 0 {
                continue;
            }

            sqlx::query(
                "INSERT INTO forge_pull_requests (session_id, number, title, url, state, body, head_ref, head_sha, updated_at, merged_at, source_turn_id, synced_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(session_id, number) DO UPDATE SET title=excluded.title, url=excluded.url, state=excluded.state, body=excluded.body, head_ref=excluded.head_ref, head_sha=excluded.head_sha, updated_at=excluded.updated_at, merged_at=excluded.merged_at, source_turn_id=COALESCE(forge_pull_requests.source_turn_id, excluded.source_turn_id), synced_at=excluded.synced_at",
            )
            .bind(session_id)
            .bind(pr.number)
            .bind(&pr.title)
            .bind(&pr.url)
            .bind(pr.state.to_ascii_lowercase())
            .bind(body)
            .bind(&pr.head_ref_name)
            .bind(&pr.head_ref_oid)
            .bind(&pr.updated_at)
            .bind(&pr.merged_at)
            .bind(&turn_id)
            .bind(crate::migrate::now_ms())
            .execute(pool)
            .await
            .map_err(db_error)?;
            report.pull_requests += 1;

            let check_runs: CheckRunsResponse = parse_json(&self.runner.run(
                Some(project),
                &[
                    "api",
                    &format!("repos/{repo}/commits/{}/check-runs", pr.head_ref_oid),
                ],
            )?)?;
            for check in check_runs.check_runs {
                sqlx::query(
                    "INSERT INTO forge_checks (session_id, pr_number, external_id, name, status, conclusion, details_url, started_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(session_id, pr_number, external_id) DO UPDATE SET name=excluded.name, status=excluded.status, conclusion=excluded.conclusion, details_url=excluded.details_url, started_at=excluded.started_at, completed_at=excluded.completed_at",
                )
                .bind(session_id)
                .bind(pr.number)
                .bind(check.id.to_string())
                .bind(check.name)
                .bind(check.status)
                .bind(check.conclusion)
                .bind(check.details_url)
                .bind(check.started_at)
                .bind(check.completed_at)
                .execute(pool)
                .await
                .map_err(db_error)?;
                report.checks += 1;
            }

            let comments: Vec<RemoteReviewComment> = parse_json(&self.runner.run(
                Some(project),
                &["api", &format!("repos/{repo}/pulls/{}/comments", pr.number)],
            )?)?;
            for comment in comments {
                sqlx::query(
                    "INSERT INTO forge_review_comments (session_id, pr_number, external_id, body, path, line, author, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(session_id, pr_number, external_id) DO UPDATE SET body=excluded.body, path=excluded.path, line=excluded.line, author=excluded.author, created_at=excluded.created_at, updated_at=excluded.updated_at",
                )
                .bind(session_id)
                .bind(pr.number)
                .bind(comment.id.to_string())
                .bind(comment.body)
                .bind(comment.path)
                .bind(comment.line)
                .bind(comment.user.login)
                .bind(comment.created_at)
                .bind(comment.updated_at)
                .execute(pool)
                .await
                .map_err(db_error)?;
                report.review_comments += 1;
            }
        }
        Ok(report)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    pub pull_requests: usize,
    pub checks: usize,
    pub review_comments: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemotePullRequest {
    number: i64,
    title: String,
    url: String,
    state: String,
    body: Option<String>,
    head_ref_name: Option<String>,
    head_ref_oid: String,
    updated_at: Option<String>,
    merged_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<RemoteCheck>,
}

#[derive(Debug, Deserialize)]
struct RemoteCheck {
    id: i64,
    name: String,
    status: String,
    conclusion: Option<String>,
    details_url: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoteReviewComment {
    id: i64,
    body: String,
    path: Option<String>,
    line: Option<i64>,
    user: RemoteUser,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoteUser {
    login: String,
}

fn parse_json<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, ForgeError> {
    serde_json::from_str(input)
        .map_err(|error| ForgeError::Failed(format!("invalid GitHub response: {error}")))
}

fn db_error(error: sqlx::Error) -> ForgeError {
    ForgeError::Failed(format!("could not store GitHub sync data: {error}"))
}

fn parse_session_turn_marker(body: &str, session_id: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let marker = line
            .trim()
            .strip_prefix("<!-- rigel:session=")?
            .strip_suffix(" -->")?;
        let (session, turn) = marker.split_once(";turn=")?;
        (session == session_id && !turn.is_empty()).then(|| turn.to_string())
    })
}
impl Forge for GitHubForge {
    fn check(&self) -> Result<String, ForgeError> {
        self.runner.run(None, &["auth", "status"])?;
        Ok("forge ready".into())
    }
    fn plan(
        &self,
        project: &Path,
        session: &Session,
        turns: &[TurnTranscript],
    ) -> Result<PullRequestPlan, ForgeError> {
        build_plan(project, session, turns)
    }
    fn create(&self, project: &Path, plan: &PullRequestPlan) -> Result<String, ForgeError> {
        self.check()?;
        let output = self.runner.run(
            Some(project),
            &[
                "pr",
                "create",
                "--title",
                &plan.title,
                "--body",
                &plan.body,
                "--base",
                &plan.base,
            ],
        )?;
        Ok(output.lines().last().unwrap_or(&output).to_string())
    }
}
