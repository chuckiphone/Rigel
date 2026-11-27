use rigel_core::SessionService;
use tempfile::TempDir;

#[tokio::test]
async fn fork_records_parent_session_and_turn_and_lists_both_branches() {
    let project = TempDir::new().unwrap();
    let service = SessionService::open(project.path()).await.unwrap();
    let parent = service.start(Some("original"), None).await.unwrap();
    let agent = service
        .register_agent(&parent.id, "native", Some("implementer"))
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("first prompt"))
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "answer",
            "completed",
            &serde_json::json!({"text": "first answer"}),
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    service.finish_turn(&turn.id, "completed").await.unwrap();
    let later = service
        .append_turn(&agent.id, Some("later prompt"))
        .await
        .unwrap();

    let fork = service.fork(&parent.id, &turn.id).await.unwrap();
    assert_eq!(fork.parent_session_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(fork.parent_turn_id.as_deref(), Some(turn.id.as_str()));
    assert_eq!(service.list().await.unwrap().len(), 2);
    let transcript = service.transcript(&fork.id).await.unwrap();
    assert_eq!(
        transcript.len(),
        1,
        "fork only includes history through the selected turn"
    );
    assert_eq!(transcript[0].turn.prompt.as_deref(), Some("first prompt"));
    assert_ne!(
        transcript[0].turn.id, turn.id,
        "forked history gets independent turn ids"
    );
    assert_eq!(transcript[0].items[0].body["text"], "first answer");
    assert!(service.turn(&later.id).await.is_ok());
    let agents = service.agents(&fork.id).await.unwrap();
    let (markdown, json) =
        rigel_core::diagnostics::session_bundle(&fork, &agents, &transcript, &[], &[], &[]);
    assert!(markdown.contains(&format!("forked from session: `{}`", parent.id)));
    assert!(markdown.contains(&format!("forked at turn: `{}`", turn.id)));
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["session"]["parent_session_id"], parent.id);
    assert_eq!(json["session"]["parent_turn_id"], turn.id);
}

#[tokio::test]
async fn fork_rejects_a_turn_from_another_session() {
    let project = TempDir::new().unwrap();
    let service = SessionService::open(project.path()).await.unwrap();
    let parent = service.start(None, None).await.unwrap();
    let other = service.start(None, None).await.unwrap();
    let agent = service
        .register_agent(&other.id, "native", None)
        .await
        .unwrap();
    let turn = service.append_turn(&agent.id, None).await.unwrap();

    assert!(service.fork(&parent.id, &turn.id).await.is_err());
}
