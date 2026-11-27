//! Roadmap 132: a diff-review verdict names its reviewer and is stored with the turn it reviewed.

use rigel_core::SessionService;

#[tokio::test]
async fn a_named_reviewer_verdict_is_stored_with_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let service = SessionService::open(dir.path()).await.unwrap();
    let session = service.start(Some("reviewed"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("please review"))
        .await
        .unwrap();

    let accepted = vec!["hunk-2".to_string()];
    let stored = service
        .record_diff_review_verdict(&turn.id, " reviewer@example.com ", &accepted, 3)
        .await
        .unwrap();
    assert_eq!(stored.kind, "note");
    assert_eq!(stored.status, "verdict");
    assert_eq!(stored.turn_id, turn.id);

    // The transcript is the evidence: a fresh read sees the same reviewer and chosen hunk.
    let transcript = service.transcript(&session.id).await.unwrap();
    let verdict = transcript[0]
        .items
        .iter()
        .find(|item| item.body.get("kind").and_then(|v| v.as_str()) == Some("diff_review_verdict"))
        .expect("the verdict is stored with the turn");
    assert_eq!(verdict.body["reviewer"], "reviewer@example.com");
    assert_eq!(verdict.body["accepted"][0], "hunk-2");
    assert_eq!(verdict.body["hunks"], 3);
    assert!(verdict.body["created_at"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn a_blank_reviewer_has_no_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let service = SessionService::open(dir.path()).await.unwrap();
    let session = service.start(Some("reviewed"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("please review"))
        .await
        .unwrap();
    assert!(
        service
            .record_diff_review_verdict(&turn.id, "  ", &[], 1)
            .await
            .is_err()
    );
    let transcript = service.transcript(&session.id).await.unwrap();
    assert!(transcript[0].items.is_empty());
}
