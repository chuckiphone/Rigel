//! `rigel handoff`: package unfinished work for another agent.
//!
//! The command is deliberately read-only. It selects one session, carries the project's rule
//! files verbatim, names the files the session touched and the files the worktree still has
//! changed, prints a bounded `git diff --stat`, and lists the questions that remain open. The
//! output is a prompt, not a completion report: every open item is an unchecked box and the
//! last recorded model answer is labelled unverified.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use rigel_core::plan;
use rigel_core::rules::{self, RuleSource};
use rigel_core::{Session, SessionService, SessionStatus, TurnTranscript};
use serde::Serialize;
use serde_json::Value;

const MAX_SESSION_FILES: usize = 120;
const MAX_WORKTREE_FILES: usize = 200;
const MAX_DIFF_LINES: usize = 24;
const MAX_LAST_OUTPUT_CHARS: usize = 900;
const MAX_TASK_CHARS: usize = 4_000;

/// The flags for `rigel handoff`. Kept in its own `Args` struct so the top-level command
/// enum does not grow another inline argument variant.
#[derive(Debug, clap::Args)]
pub struct HandoffArgs {
    #[arg(long, value_name = "PATH", default_value = ".")]
    pub project: PathBuf,
    /// Session id. Defaults to the newest session that has not ended.
    #[arg(long)]
    pub session: Option<String>,
    /// Replace the task recorded on the session's latest turn.
    #[arg(long)]
    pub task: Option<String>,
    /// Add a constraint; repeat for more than one.
    #[arg(long = "constraint", value_name = "TEXT")]
    pub constraints: Vec<String>,
    /// Add an open question; repeat for more than one.
    #[arg(long = "open-question", value_name = "TEXT")]
    pub open_questions: Vec<String>,
    /// Write the prompt to this file as well as stdout.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Handoff {
    project: String,
    session: SessionInfo,
    task: String,
    constraints: ConstraintSet,
    files: FileSet,
    diff: DiffSummary,
    last_output: Option<String>,
    plan: Option<String>,
    open_questions: Vec<String>,
    prompt: String,
}

#[derive(Debug, Serialize)]
struct SessionInfo {
    id: String,
    title: Option<String>,
    status: String,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct ConstraintSet {
    sources: Vec<RuleSource>,
    text: String,
    extra: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FileSet {
    session: Vec<String>,
    worktree: Vec<WorktreeFile>,
}

#[derive(Debug, Clone, Serialize)]
struct WorktreeFile {
    status: String,
    path: String,
}

#[derive(Debug, Default, Serialize)]
struct DiffSummary {
    is_repo: bool,
    branch: Option<String>,
    head: Option<String>,
    changed_paths: usize,
    untracked_paths: usize,
    summary: String,
    stat: String,
    note: Option<String>,
    #[serde(skip)]
    files: Vec<WorktreeFile>,
}

/// Run `rigel handoff`. `--out` writes the prompt as well as printing it; `--json` replaces the
/// prompt on stdout with the structured object that contains it.
pub async fn run(args: HandoffArgs) -> ExitCode {
    let HandoffArgs {
        project,
        session,
        task,
        constraints,
        open_questions,
        out,
        json: as_json,
    } = args;
    let root = match std::fs::canonicalize(&project) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("rigel: {}: {error}", project.display());
            return ExitCode::from(2);
        }
    };
    let service = match SessionService::open(&root).await {
        Ok(service) => service,
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };
    let session = match select_session(&service, session.as_deref()).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            eprintln!(
                "rigel: no sessions in {}; run `rigel session start` first",
                root.display()
            );
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };
    let transcript = match service.transcript(&session.id).await {
        Ok(transcript) => transcript,
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };

    let latest = transcript
        .iter()
        .rfind(|entry| entry.turn.parent_turn_id.is_none());
    let task = task
        .map(|text| truncate_chars(text.trim(), MAX_TASK_CHARS))
        .filter(|text| !text.is_empty())
        .or_else(|| {
            latest
                .and_then(|entry| entry.turn.prompt.as_deref())
                .map(|text| truncate_chars(text.trim(), MAX_TASK_CHARS))
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            "No task prompt was recorded. Inspect the files, the diff and the open questions below, then continue the unfinished work.".to_string()
        });

    let loaded_rules = rules::load(&root);
    let extra = constraints
        .into_iter()
        .map(|text| truncate_chars(text.trim(), MAX_TASK_CHARS))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let constraints = ConstraintSet {
        sources: loaded_rules.sources,
        text: loaded_rules.text,
        extra,
    };

    let session_files = session_files(&transcript);
    let diff = diff_summary(&root);
    let files = FileSet {
        session: session_files,
        worktree: diff.files.clone(),
    };

    let plan_text = match plan::state(service.pool(), &session.id).await {
        Ok(state) => state
            .plan
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        Err(_) => None,
    };

    let mut questions = open_questions
        .into_iter()
        .map(|text| clean_line(&text, MAX_TASK_CHARS))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    if let Some(entry) = latest {
        // An error item is unresolved evidence, not a completion. Keep only a few so a noisy
        // turn cannot turn the handoff into a second transcript.
        for item in entry
            .items
            .iter()
            .filter(|item| item.kind == "error")
            .take(3)
        {
            let text = item
                .body
                .get("message")
                .or_else(|| item.body.get("error"))
                .map(value_text)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| {
                    "The previous turn recorded an error without a message.".to_string()
                });
            questions.push(format!(
                "Resolve the recorded error: {}",
                clean_line(&text, 500)
            ));
        }
    }
    questions = dedupe(questions);

    let last_output = last_assistant_text(&transcript)
        .map(|text| truncate_chars(text.trim(), MAX_LAST_OUTPUT_CHARS));

    let mut handoff = Handoff {
        project: root.display().to_string(),
        session: SessionInfo {
            id: session.id.clone(),
            title: session.title.clone(),
            status: session.status.as_str().to_string(),
            created_at: session.created_at,
        },
        task,
        constraints,
        files,
        diff,
        last_output,
        plan: plan_text,
        open_questions: questions,
        prompt: String::new(),
    };
    handoff.prompt = render(&handoff);

    if let Some(path) = out
        && let Err(error) = std::fs::write(&path, format!("{}\n", handoff.prompt))
    {
        eprintln!("rigel: could not write {}: {error}", path.display());
        return ExitCode::from(2);
    }

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&handoff).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!("{}", handoff.prompt);
    }
    ExitCode::SUCCESS
}

async fn select_session(
    service: &SessionService,
    requested: Option<&str>,
) -> Result<Option<Session>, String> {
    if let Some(id) = requested {
        return service
            .get(id)
            .await
            .map(Some)
            .map_err(|error| error.to_string());
    }
    let sessions = service.list().await.map_err(|error| error.to_string())?;
    let chosen = sessions
        .iter()
        .filter(|session| session.status != SessionStatus::Ended)
        .max_by_key(|session| (session.created_at, session.id.clone()))
        .or_else(|| {
            sessions
                .iter()
                .max_by_key(|session| (session.created_at, session.id.clone()))
        });
    Ok(chosen.cloned())
}

fn session_files(transcript: &[TurnTranscript]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for entry in transcript {
        for item in &entry.items {
            if item.kind != "file_change" {
                continue;
            }
            let path = item
                .body
                .get("result")
                .and_then(|result| result.get("path"))
                .or_else(|| item.body.get("path"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty());
            if let Some(path) = path {
                paths.insert(path.to_string());
            }
        }
    }
    paths.into_iter().take(MAX_SESSION_FILES).collect()
}

fn last_assistant_text(transcript: &[TurnTranscript]) -> Option<String> {
    for entry in transcript.iter().rev() {
        if entry.turn.parent_turn_id.is_some() {
            continue;
        }
        for item in entry.items.iter().rev() {
            if item.kind != "message" {
                continue;
            }
            if item.body.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            if let Some(text) = item.body.get("text").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn diff_summary(root: &Path) -> DiffSummary {
    let is_repo = git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Some("true");
    if !is_repo {
        return DiffSummary {
            note: Some("Not a git worktree; no worktree status or diff is available.".to_string()),
            ..DiffSummary::default()
        };
    }
    let (files, changed_paths, untracked_paths) = worktree_files(root);
    let branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .filter(|branch| !branch.is_empty());
    let head = git(root, &["rev-parse", "--short", "HEAD"]);
    let (summary, stat) = git(root, &["diff", "HEAD", "--stat", "--no-color"])
        .map(|raw| split_stat(&raw))
        .or_else(|| git(root, &["diff", "--stat", "--no-color"]).map(|raw| split_stat(&raw)))
        .unwrap_or_default();
    let mut note = None;
    if head.is_none() {
        note = Some(
            "No commit exists yet; only unstaged tracked changes are in this diff; staged and untracked files are in the status list."
                .to_string(),
        );
    } else if untracked_paths > 0 {
        note = Some(
            "Untracked files are listed by status and do not appear in git diff --stat."
                .to_string(),
        );
    }
    DiffSummary {
        is_repo,
        branch,
        head,
        changed_paths,
        untracked_paths,
        summary,
        stat,
        note,
        files,
    }
}

fn split_stat(raw: &str) -> (String, String) {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let Some((summary, files)) = lines.split_last() else {
        return (String::new(), String::new());
    };
    let mut stat = files
        .iter()
        .take(MAX_DIFF_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if files.len() > MAX_DIFF_LINES {
        stat.push_str("\n…");
    }
    (summary.trim().to_string(), stat)
}

fn worktree_files(root: &Path) -> (Vec<WorktreeFile>, usize, usize) {
    let Some(status) = git(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    ) else {
        return (Vec::new(), 0, 0);
    };
    let files: Vec<WorktreeFile> = status
        .lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let code = line.get(..2)?.to_string();
            let mut path = line.get(3..)?.trim().to_string();
            if let Some((_, destination)) = path.rsplit_once(" -> ") {
                path = destination.to_string();
            }
            if path.is_empty() {
                return None;
            }
            Some(WorktreeFile { status: code, path })
        })
        .collect();
    let total = files.len();
    let untracked = files.iter().filter(|file| file.status == "??").count();
    (
        files.into_iter().take(MAX_WORKTREE_FILES).collect(),
        total,
        untracked,
    )
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        // Keep leading spaces: a porcelain status line starts with the index/worktree code,
        // and ` M path` must not become `M path` or the path loses its first character.
        .then(|| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string()
        })
}

fn render(handoff: &Handoff) -> String {
    let mut out = String::new();
    let title = handoff
        .session
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .map(|title| clean_line(title, 100))
        .unwrap_or_else(|| format!("session {}", short_id(&handoff.session.id)));
    let _ = writeln!(out, "# Handoff — {title}");
    let _ = writeln!(
        out,
        "\nUse this as the prompt for a new session on the same checkout. It describes unfinished work; \
         **nothing here is marked done.**\n"
    );
    let _ = writeln!(out, "## Task\n\n{}\n", handoff.task);
    let _ = writeln!(out, "## Constraints");

    if handoff.constraints.text.trim().is_empty() {
        let _ = writeln!(
            out,
            "No project rule files were found (`AGENTS.md`, `CLAUDE.md` or `.cursor/rules`)."
        );
    } else {
        out.push_str(handoff.constraints.text.trim());
        out.push('\n');
        if handoff
            .constraints
            .sources
            .iter()
            .any(|source| source.truncated)
        {
            let _ = writeln!(
                out,
                "\nOne or more rule files were truncated at the 96 KB cap; the source sizes above are exact."
            );
        }
    }
    if !handoff.constraints.extra.is_empty() {
        let _ = writeln!(out, "\n### Added constraints\n");
        for constraint in &handoff.constraints.extra {
            let _ = writeln!(out, "- {constraint}");
        }
    }

    let _ = writeln!(out, "\n## State");
    let _ = writeln!(out, "- Project: `{}`", handoff.project);
    let _ = writeln!(
        out,
        "- Session: `{}` — {} ({})",
        handoff.session.id,
        handoff
            .session
            .title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or("untitled"),
        handoff.session.status
    );
    if handoff.diff.is_repo {
        let place = match (&handoff.diff.branch, &handoff.diff.head) {
            (Some(branch), Some(head)) => format!("branch `{branch}` at `{head}`"),
            (None, Some(head)) => format!("detached HEAD at `{head}`"),
            (Some(branch), None) => format!("branch `{branch}` with no commit yet"),
            (None, None) => "a git worktree with no commit yet".to_string(),
        };
        let _ = writeln!(
            out,
            "- Worktree: {place}; {} changed path{} ({} untracked)",
            handoff.diff.changed_paths,
            plural(handoff.diff.changed_paths),
            handoff.diff.untracked_paths
        );
    } else {
        let _ = writeln!(out, "- Worktree: not a git worktree");
    }
    if handoff.last_output.is_some() {
        let _ = writeln!(
            out,
            "- Last recorded output below is evidence from the previous session, not proof that the task is complete.\n"
        );
    }

    let _ = writeln!(out, "## Files");
    let _ = writeln!(out, "### Changed in this session\n");
    if handoff.files.session.is_empty() {
        let _ = writeln!(out, "- None recorded by this session.");
    } else {
        for path in &handoff.files.session {
            let _ = writeln!(out, "- `{path}`");
        }
    }
    let _ = writeln!(out, "\n### Changed in the worktree\n");
    if !handoff.diff.is_repo {
        let _ = writeln!(out, "- Not a git worktree.");
    } else if handoff.files.worktree.is_empty() {
        let _ = writeln!(out, "- The git worktree is clean.");
    } else {
        let _ = writeln!(out, "```text");
        for file in &handoff.files.worktree {
            let _ = writeln!(out, "{} {}", file.status, file.path);
        }
        if handoff.diff.changed_paths > handoff.files.worktree.len() {
            let _ = writeln!(
                out,
                "… and {} more (run `git status` for the rest)",
                handoff.diff.changed_paths - handoff.files.worktree.len()
            );
        }
        let _ = writeln!(out, "```");
    }

    if let Some(text) = &handoff.last_output {
        let _ = writeln!(out, "\n### Last recorded output (unverified)\n");
        for line in text.lines() {
            let _ = writeln!(out, "> {line}");
        }
    }

    let _ = writeln!(out, "\n## Diff summary\n");
    if !handoff.diff.is_repo {
        let _ = writeln!(out, "Not a git worktree; no diff summary is available.");
    } else if handoff.diff.summary.is_empty() {
        let _ = writeln!(out, "No tracked diff is recorded.");
    } else {
        let _ = writeln!(out, "{}", handoff.diff.summary);
        if !handoff.diff.stat.is_empty() {
            let _ = writeln!(out, "\n```text\n{}\n```", handoff.diff.stat);
        }
    }
    if let Some(note) = &handoff.diff.note {
        let _ = writeln!(out, "\n{note}");
    }

    if let Some(text) = &handoff.plan {
        let _ = writeln!(
            out,
            "\n## Open plan (unverified)\n\nThe previous agent proposed this plan. No step is accepted as done.\n"
        );
        out.push_str(text.trim());
        out.push('\n');
    }

    let _ = writeln!(out, "\n## Open questions\n");
    if handoff.open_questions.is_empty() {
        let _ = writeln!(
            out,
            "None recorded. Review the diff and the last output, then decide the next step before editing."
        );
    } else {
        for question in &handoff.open_questions {
            let _ = writeln!(out, "- [ ] {question}");
        }
    }

    let _ = writeln!(
        out,
        "\n## Start here\n\n1. Read the constraints above before editing; they survive this handoff.\n2. Inspect the session files and the worktree diff. Do not assume any of it is correct.\n3. Work the open questions. Mark an item done only after a named check passes.\n4. If a constraint and this handoff disagree, the constraint wins.\n"
    );
    out.trim_end().to_string()
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn clean_line(text: &str, max: usize) -> String {
    truncate_chars(text.lines().next().unwrap_or("").trim(), max)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::{clean_line, split_stat, truncate_chars};

    #[test]
    fn a_stat_summary_stays_separate_from_the_file_rows() {
        let raw = " src/a.rs | 2 +-\n src/b.rs | 4 ++++\n 2 files changed, 4 insertions(+), 2 deletions(-)\n";
        let (summary, stat) = split_stat(raw);
        assert_eq!(summary, "2 files changed, 4 insertions(+), 2 deletions(-)");
        assert_eq!(stat, " src/a.rs | 2 +-\n src/b.rs | 4 ++++");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("aé日", 2), "aé…");
        assert_eq!(clean_line(" first line\nsecond", 40), "first line");
    }
}
