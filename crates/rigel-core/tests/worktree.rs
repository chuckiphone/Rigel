//! Worktrees and merge-back (P5.5), against real git repositories.
//!
//! The interesting cases are the disagreements: a file the main tree moved, a deletion, a
//! binary. Each has to be reported and has to leave the main tree untouched, because the whole
//! point of a worktree is that a subagent cannot surprise the shared tree.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use rigel_core::{SessionService, WorktreeError, WorktreeService};

fn git(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn has_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    assert!(git(tmp.path(), &["init", "-q"]));
    assert!(git(tmp.path(), &["config", "user.email", "rigel@test"]));
    assert!(git(tmp.path(), &["config", "user.name", "Rigel Test"]));
    assert!(git(tmp.path(), &["config", "core.autocrlf", "false"]));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "fn version() -> u8 { 1 }\n").unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "target/\n").unwrap();
    assert!(git(tmp.path(), &["add", "."]));
    assert!(git(tmp.path(), &["commit", "-q", "-m", "base"]));
    tmp
}

async fn setup() -> Option<(
    tempfile::TempDir,
    tempfile::TempDir,
    SessionService,
    String,
    String,
    WorktreeService,
)> {
    if !has_git() {
        return None;
    }
    let repo = repo();
    let data = tempfile::tempdir().unwrap();
    let service = SessionService::open(repo.path()).await.unwrap();
    let session = service.start(Some("worktree"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("worker"))
        .await
        .unwrap();
    let trees = WorktreeService::new(repo.path(), data.path(), service.pool().clone());
    Some((repo, data, service, session.id, agent.id, trees))
}

#[tokio::test]
async fn creating_a_worktree_leaves_the_project_tree_alone() {
    let Some((repo, _data, service, session, agent, trees)) = setup().await else {
        return;
    };

    let worktree = trees.create(&agent).await.unwrap();
    assert!(worktree.path.exists());
    assert!(
        worktree.path.starts_with(trees.base_dir()),
        "a worktree lives under the data directory, not inside the project"
    );
    assert!(git(&worktree.path, &["rev-parse", "--is-inside-work-tree"]));
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("src/lib.rs")).unwrap(),
        "fn version() -> u8 { 1 }\n"
    );

    // The agent row points at it, and the project has no new untracked files.
    let row = service.agent(&agent).await.unwrap();
    assert_eq!(
        row.worktree.as_deref(),
        Some(worktree.path.to_string_lossy().as_ref())
    );
    let status = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    let porcelain = String::from_utf8_lossy(&status.stdout);
    let dirty: Vec<&str> = porcelain
        .lines()
        .filter(|line| !line.contains(".rigel/"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    assert!(
        dirty.is_empty(),
        "cutting a worktree changed the project tree: {dirty:?}"
    );

    // The session id is accepted here only so an unused warning cannot hide a mistake.
    assert_eq!(row.session_id, session);
}

#[tokio::test]
async fn a_change_in_the_worktree_lands_as_one_applied_patch() {
    let Some((repo, _data, _service, session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(
        worktree.path.join("src/lib.rs"),
        "fn version() -> u8 { 2 }\n",
    )
    .unwrap();

    assert_eq!(
        trees.preview(&agent).await.unwrap(),
        vec![("src/lib.rs".to_string(), "modified".to_string())]
    );

    let report = trees.merge_back(&session, &agent).await.unwrap();
    assert!(report.clean(), "{report:?}");
    assert_eq!(report.applied, 1);
    assert_eq!(report.lines[0].path, "src/lib.rs");
    assert!(report.lines[0].patch_id.is_some());
    assert_eq!(
        std::fs::read_to_string(repo.path().join("src/lib.rs")).unwrap(),
        "fn version() -> u8 { 2 }\n"
    );

    // Through the pipeline, so there is a patch row and a revert is possible.
    let pipeline = rigel_core::PatchPipeline::new(repo.path(), _service.pool().clone());
    let patches = pipeline.list(&session).await.unwrap();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].status, "applied");
    pipeline.revert(&patches[0].id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("src/lib.rs")).unwrap(),
        "fn version() -> u8 { 1 }\n"
    );

    // The worktree itself is untouched by the merge.
    assert_eq!(
        std::fs::read_to_string(worktree.path.join("src/lib.rs")).unwrap(),
        "fn version() -> u8 { 2 }\n"
    );
}

#[tokio::test]
async fn a_new_file_is_a_patch_and_a_deletion_is_reported_not_written() {
    let Some((repo, _data, _service, session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(worktree.path.join("notes.md"), "from the agent\n").unwrap();
    std::fs::remove_file(worktree.path.join("src/lib.rs")).unwrap();

    let report = trees.merge_back(&session, &agent).await.unwrap();
    assert_eq!(report.applied, 1, "{report:?}");
    assert_eq!(report.skipped, 1);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("notes.md")).unwrap(),
        "from the agent\n"
    );
    assert!(
        repo.path().join("src/lib.rs").exists(),
        "a deletion the pipeline cannot express is not guessed at"
    );
    let deleted = report
        .lines
        .iter()
        .find(|line| line.path == "src/lib.rs")
        .unwrap();
    assert_eq!(deleted.status, rigel_core::MergeStatus::Deleted);
    assert!(deleted.detail.as_deref().unwrap().contains("not a patch"));
}

#[tokio::test]
async fn a_main_tree_that_moved_is_a_conflict_and_nothing_is_written() {
    let Some((repo, _data, _service, session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(
        worktree.path.join("src/lib.rs"),
        "fn version() -> u8 { 2 }\n",
    )
    .unwrap();
    // Someone else edits the same file in the shared tree after the worktree was cut.
    std::fs::write(
        repo.path().join("src/lib.rs"),
        "fn version() -> u8 { 99 }\n",
    )
    .unwrap();

    let report = trees.merge_back(&session, &agent).await.unwrap();
    assert!(!report.clean());
    assert_eq!(report.applied, 0);
    assert_eq!(report.conflicted, 1);
    assert_eq!(report.lines[0].status, rigel_core::MergeStatus::Conflicted);
    assert!(
        report.lines[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("main tree moved")
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("src/lib.rs")).unwrap(),
        "fn version() -> u8 { 99 }\n",
        "the worktree did not overwrite the newer edit"
    );
}

#[tokio::test]
async fn identical_bytes_need_no_patch() {
    let Some((_repo, _data, _service, session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    // Touch the file without changing it: git reports a modification only if the content
    // differs, so this is about the merge deciding the same bytes need nothing.
    std::fs::write(
        worktree.path.join("src/lib.rs"),
        "fn version() -> u8 { 1 }\n",
    )
    .unwrap();
    let report = trees.merge_back(&session, &agent).await.unwrap();
    assert!(report.clean());
    assert_eq!(report.applied, 0);
}

#[tokio::test]
async fn removing_a_dirty_worktree_needs_force_and_clears_the_agent() {
    let Some((_repo, _data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(worktree.path.join("src/lib.rs"), "changed\n").unwrap();

    let error = trees.remove(&agent, false).await.expect_err("dirty tree");
    assert!(matches!(error, WorktreeError::Git { .. }), "{error}");
    assert!(worktree.path.exists());

    trees.remove(&agent, true).await.unwrap();
    assert!(!worktree.path.exists());
    assert!(service.agent(&agent).await.unwrap().worktree.is_none());
}

#[tokio::test]
async fn pruning_keeps_an_unmerged_worktree_with_a_reason() {
    let Some((_repo, _data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(worktree.path.join("src/lib.rs"), "unmerged change\n").unwrap();
    assert!(git(&worktree.path, &["add", "."]));
    assert!(git(&worktree.path, &["commit", "-q", "-m", "child change"]));
    service.set_agent_status(&agent, "exited").await.unwrap();

    let report = trees.prune(Duration::ZERO).await.unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(report.kept, 1);
    assert!(report.entries[0].reason.contains("not merged"));
    assert!(worktree.path.exists());
}

#[tokio::test]
async fn pruning_keeps_a_dirty_worktree_with_a_reason() {
    let Some((_repo, _data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(worktree.path.join("src/lib.rs"), "uncommitted change\n").unwrap();
    service.set_agent_status(&agent, "exited").await.unwrap();

    let report = trees.prune(Duration::ZERO).await.unwrap();
    assert_eq!(report.removed, 0);
    assert!(report.entries[0].reason.contains("uncommitted changes"));
    assert!(worktree.path.exists());
}

#[tokio::test]
async fn pruning_obeys_the_retention_window() {
    let Some((_repo, _data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    service.set_agent_status(&agent, "exited").await.unwrap();

    let report = trees.prune(Duration::from_secs(86_400)).await.unwrap();
    assert_eq!(report.removed, 0);
    assert!(report.entries[0].reason.contains("retention window"));
    assert!(worktree.path.exists());
}

#[tokio::test]
async fn pruning_removes_a_clean_merged_worktree_after_retention() {
    let Some((repo, _data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    let worktree = trees.create(&agent).await.unwrap();
    std::fs::write(worktree.path.join("src/lib.rs"), "merged change\n").unwrap();
    assert!(git(&worktree.path, &["add", "."]));
    assert!(git(&worktree.path, &["commit", "-q", "-m", "merged child"]));
    let output = Command::new("git")
        .arg("-C")
        .arg(&worktree.path)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let child_head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(git(repo.path(), &["merge", "--ff-only", &child_head]));
    service.set_agent_status(&agent, "exited").await.unwrap();
    sqlx::query("UPDATE agents SET started_at = ? WHERE id = ?")
        .bind(1_i64)
        .bind(&agent)
        .execute(service.pool())
        .await
        .unwrap();

    let report = trees.prune(Duration::from_millis(1)).await.unwrap();
    assert_eq!(report.removed, 1);
    assert!(report.entries[0].removed);
    assert!(!worktree.path.exists());
    assert!(service.agent(&agent).await.unwrap().worktree.is_none());
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_is_refused() {
    if !has_git() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let service = SessionService::open(dir.path()).await.unwrap();
    let session = service.start(Some("plain"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let trees = WorktreeService::new(dir.path(), data.path(), service.pool().clone());
    let error = trees.create(&agent.id).await.expect_err("not a repo");
    assert!(matches!(error, WorktreeError::NotARepository(_)), "{error}");
}

/// A stored pointer is editable data. `remove` must refuse a path outside the managed directory —
/// including a symlink placed inside it — without running git or clearing the pointer.
#[tokio::test]
async fn remove_refuses_an_out_of_base_or_escaped_pointer() {
    let Some((_repo, data, service, _session, agent, trees)) = setup().await else {
        return;
    };
    trees.create(&agent).await.unwrap();

    // 1. A path that never starts with the base directory.
    let outside = data.path().join("worktrees-elsewhere").join("checkout");
    std::fs::create_dir_all(&outside).unwrap();
    let raw = outside.to_string_lossy().into_owned();
    sqlx::query("UPDATE agents SET worktree = ? WHERE id = ?")
        .bind(&raw)
        .bind(&agent)
        .execute(service.pool())
        .await
        .unwrap();
    let error = trees
        .remove(&agent, true)
        .await
        .expect_err("out-of-base pointer");
    assert!(matches!(error, WorktreeError::OutsideBase(_)), "{error}");
    assert_eq!(
        service.agent(&agent).await.unwrap().worktree.as_deref(),
        Some(raw.as_str())
    );
    assert!(
        outside.exists(),
        "the directory outside the base was not touched"
    );

    // 2. A path that lexically starts with the base but canonically escapes it. `..` needs no
    //    symlink privilege and exercises the same canonical-containment check on every platform.
    let outside_target = data.path().join("elsewhere");
    std::fs::create_dir_all(&outside_target).unwrap();
    let link = data.path().join("worktrees").join("..").join("elsewhere");
    let raw = link.to_string_lossy().into_owned();
    sqlx::query("UPDATE agents SET worktree = ? WHERE id = ?")
        .bind(&raw)
        .bind(&agent)
        .execute(service.pool())
        .await
        .unwrap();
    let error = trees
        .remove(&agent, true)
        .await
        .expect_err("escaped pointer");
    assert!(matches!(error, WorktreeError::OutsideBase(_)), "{error}");
    assert_eq!(
        service.agent(&agent).await.unwrap().worktree.as_deref(),
        Some(raw.as_str())
    );
    assert!(
        outside_target.exists(),
        "the linked directory was not removed"
    );
}
