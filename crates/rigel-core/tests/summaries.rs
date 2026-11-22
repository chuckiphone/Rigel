//! Roadmap 57: a turn's summary is stored once, names the items it came from, and is what
//! palette search reads instead of deriving the turn again.

use rigel_core::agent::summary;
use rigel_core::{SessionService, search::workspace};
use tempfile::TempDir;

#[tokio::test]
async fn a_finished_turn_stores_its_summary_and_search_reads_it_without_the_items() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("PDF importer"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("implementer"))
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("Fix the parser crash"))
        .await
        .unwrap();
    let message = service
        .append_item(
            &turn.id,
            "message",
            "completed",
            &serde_json::json!({ "role": "assistant", "text": "the crash is in the lexer" }),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    let command = service
        .append_item(
            &turn.id,
            "command",
            "completed",
            &serde_json::json!({ "command": "cargo test -p parser" }),
            &serde_json::json!({}),
        )
        .await
        .unwrap();

    service.finish_turn(&turn.id, "completed").await.unwrap();

    let stored = summary::for_turn(service.pool(), &turn.id)
        .await
        .unwrap()
        .expect("closing a turn stores its summary");
    assert!(stored.summary.contains("the crash is in the lexer"));
    assert!(stored.summary.contains("cargo test -p parser"));
    assert_eq!(stored.item_ids, vec![message.id.clone(), command.id]);
    assert_eq!(
        summary::for_session(service.pool(), &session.id)
            .await
            .unwrap()
            .len(),
        1,
        "one summary per turn"
    );

    // Remove the bodies. The prompt does not contain `lexer`, so a hit can only come from the
    // stored summary; the palette still finds the turn and labels where it came from.
    sqlx::query("DELETE FROM items WHERE turn_id = ?")
        .bind(&turn.id)
        .execute(service.pool())
        .await
        .unwrap();
    let hits = workspace(service.pool(), "lexer", None, 10).await.unwrap();
    let hit = hits
        .turns
        .iter()
        .find(|hit| hit.turn_id == turn.id)
        .expect("the summary keeps the turn searchable");
    assert!(hit.from_summary);
    assert!(!hit.from_item);
    assert!(hit.snippet.contains("lexer"));
}
