use rigel_core::{forge::github::GitHubForge, storage::Store};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[tokio::test]
async fn sync_imports_linked_pr_checks_comments_and_updates_closed_state_idempotently() {
    let project = tempfile::tempdir().unwrap();
    let db_root = tempfile::tempdir().unwrap();
    let store = Store::open_project_with_key(db_root.path(), None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions(id, project_root, status, created_at) VALUES ('session-1', 'demo', 'active', 1)")
        .execute(store.pool()).await.unwrap();
    sqlx::query("INSERT INTO agents(id, session_id, harness, status, started_at) VALUES ('agent-1', 'session-1', 'native', 'completed', 1)")
        .execute(store.pool()).await.unwrap();
    sqlx::query("INSERT INTO turns(id, agent_id, seq, status, prompt, started_at) VALUES ('turn-1', 'agent-1', 1, 'completed', 'implement feature', 1)")
        .execute(store.pool()).await.unwrap();

    let closed = Arc::new(AtomicBool::new(false));
    let closed_for_runner = closed.clone();
    let forge = GitHubForge::with_runner(Box::new(
        move |_project: Option<&Path>, args: &[&str]| {
            if args == ["auth", "status"] {
                return Ok(String::new());
            }
            if args.starts_with(&["repo", "view"]) {
                return Ok("owner/repo".into());
            }
            if args.starts_with(&["pr", "list"]) {
                let state = if closed_for_runner.load(Ordering::SeqCst) {
                    "CLOSED"
                } else {
                    "OPEN"
                };
                return Ok(format!(
                    r#"[{{"number":7,"title":"Feature","url":"https://github.test/pull/7","state":"{state}","body":"<!-- rigel:session=session-1;turn=turn-1 -->","headRefName":"feature","headRefOid":"abc123","updatedAt":"2026-01-02T00:00:00Z","mergedAt":null}}]"#
                ));
            }
            if args.get(1).is_some_and(|arg| arg.contains("check-runs")) {
                return Ok(r#"{"check_runs":[{"id":11,"name":"CI","status":"completed","conclusion":"success","details_url":"https://github.test/check/11","started_at":"2026-01-02T00:00:00Z","completed_at":"2026-01-02T00:01:00Z"}]}"#.into());
            }
            if args.get(1).is_some_and(|arg| arg.ends_with("/comments")) {
                return Ok(r#"[{"id":22,"body":"Looks good","path":"src/lib.rs","line":9,"user":{"login":"reviewer"},"created_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"}]"#.into());
            }
            Err(rigel_core::forge::ForgeError::Failed(format!(
                "unexpected gh call: {}",
                args.join(" ")
            )))
        },
    ));

    let first = forge
        .sync_session(project.path(), &store, "session-1")
        .await
        .unwrap();
    assert_eq!(first.pull_requests, 1);
    assert_eq!(first.checks, 1);
    assert_eq!(first.review_comments, 1);
    let (title, url): (String, String) = sqlx::query_as(
        "SELECT title, url FROM forge_pull_requests WHERE session_id='session-1' AND number=7",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(title, "Feature");
    assert_eq!(url, "https://github.test/pull/7");
    let linked_turn: String = sqlx::query_scalar(
        "SELECT source_turn_id FROM forge_pull_requests WHERE session_id='session-1' AND number=7",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(linked_turn, "turn-1");
    let (check_name, conclusion): (String, String) = sqlx::query_as(
        "SELECT name, conclusion FROM forge_checks WHERE session_id='session-1' AND pr_number=7",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        (check_name.as_str(), conclusion.as_str()),
        ("CI", "success")
    );
    let (comment, author): (String, String) = sqlx::query_as(
        "SELECT body, author FROM forge_review_comments WHERE session_id='session-1' AND pr_number=7",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        (comment.as_str(), author.as_str()),
        ("Looks good", "reviewer")
    );

    let second = forge
        .sync_session(project.path(), &store, "session-1")
        .await
        .unwrap();
    assert_eq!(second, first);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM forge_pull_requests WHERE session_id='session-1'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM forge_checks WHERE session_id='session-1'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forge_review_comments WHERE session_id='session-1'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);

    closed.store(true, Ordering::SeqCst);
    forge
        .sync_session(project.path(), &store, "session-1")
        .await
        .unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM forge_pull_requests WHERE session_id='session-1' AND number=7",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(state, "closed");
}
