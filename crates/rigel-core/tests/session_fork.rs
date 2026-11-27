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

    let fork = service.fork(&parent.id, &turn.id).await.unwrap();
    assert_eq!(fork.parent_session_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(fork.parent_turn_id.as_deref(), Some(turn.id.as_str()));
    assert_eq!(service.list().await.unwrap().len(), 2);
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
