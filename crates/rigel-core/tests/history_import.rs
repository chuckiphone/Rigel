use rigel_core::{
    SessionService,
    import::{HistorySource, RowDisposition, dry_run, import_history},
};
use tempfile::TempDir;

#[test]
fn dry_run_lists_every_importable_duplicate_and_invalid_row() {
    let input = concat!(
        "{\"type\":\"user\",\"uuid\":\"u1\",\"timestamp\":\"2024-01-02T03:04:05Z\",\"message\":{\"role\":\"user\",\"content\":\"prompt\"}}\n",
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]}}\n",
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"duplicate\"}]}}\n",
        "not-json\n",
        "{\"type\":\"progress\",\"data\":{}}"
    );
    let report = dry_run(input, HistorySource::Claude);
    assert!(report.dry_run);
    assert_eq!(report.session_id, None);
    assert_eq!(report.rows.len(), 5);
    assert_eq!(report.rows[0].disposition, RowDisposition::Import);
    assert!(report.rows[2].reason.contains("duplicate"));
    assert!(report.rows[3].reason.contains("invalid JSON"));
    assert!(report.rows[4].reason.contains("unsupported"));
}

#[tokio::test]
async fn imports_claude_turn_and_tool_rows_with_source_timestamps() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let input = concat!(
        "{\"type\":\"user\",\"uuid\":\"u1\",\"timestamp\":\"2024-01-02T03:04:05Z\",\"message\":{\"role\":\"user\",\"content\":\"imported prompt\"}}\n",
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"timestamp\":\"2024-01-02T03:04:06Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"imported answer\"},{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"Read\",\"input\":{\"file\":\"src/lib.rs\"}}]}}"
    );
    let report = import_history(
        &service,
        input,
        HistorySource::Claude,
        Some("Imported Claude run"),
    )
    .await
    .unwrap();
    let session_id = report.session_id.unwrap();
    assert!(!report.dry_run);
    assert_eq!(report.rows.len(), 2);
    let imported_session = service.get(&session_id).await.unwrap();
    assert_eq!(
        imported_session.title.as_deref(),
        Some("Imported Claude run")
    );
    assert_eq!(imported_session.created_at, 1_704_164_645_000);
    assert_eq!(imported_session.ended_at, Some(1_704_164_646_000));
    let agents = service.agents(&session_id).await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].harness, "claude");
    let transcript = service.transcript(&session_id).await.unwrap();
    assert_eq!(transcript.len(), 1);
    assert_eq!(
        transcript[0].turn.prompt.as_deref(),
        Some("imported prompt")
    );
    assert_eq!(transcript[0].turn.started_at, 1_704_164_645_000);
    assert_eq!(transcript[0].turn.completed_at, Some(1_704_164_646_000));
    assert_eq!(transcript[0].items.len(), 2);
    assert_eq!(transcript[0].items[0].kind, "answer");
    assert_eq!(transcript[0].items[1].kind, "tool");
    assert_eq!(transcript[0].items[1].created_at, 1_704_164_646_000);
}

#[test]
fn imports_codex_function_calls_as_tool_rows() {
    let input = r#"{"type":"response_item","id":"c1","timestamp":1704164645000,"payload":{"type":"function_call","name":"shell","arguments":"cargo test"}}"#;
    let report = dry_run(input, HistorySource::Codex);
    assert_eq!(report.rows[0].disposition, RowDisposition::Import);
}
