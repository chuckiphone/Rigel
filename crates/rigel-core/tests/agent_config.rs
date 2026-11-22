//! Roadmap 58: a roster row owns a model/provided/effort pair; a role preset overrides it, and
//! clearing the preset keeps the pair the row resolved to.

use rigel_core::SessionService;
use tempfile::TempDir;

#[tokio::test]
async fn an_agent_row_stores_its_pair_and_a_preset_overrides_it() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("roster models"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("worker"))
        .await
        .unwrap();
    assert_eq!(agent.model, None);
    assert_eq!(agent.provider, None);
    assert_eq!(agent.effort, None);

    let configured = service
        .set_agent_config(
            &session.id,
            &agent.id,
            Some("anthropic/claude-sonnet-4.5"),
            Some("openrouter"),
            Some("low"),
        )
        .await
        .unwrap();
    assert_eq!(
        configured.model.as_deref(),
        Some("anthropic/claude-sonnet-4.5")
    );
    assert_eq!(configured.provider.as_deref(), Some("openrouter"));
    assert_eq!(configured.effort.as_deref(), Some("low"));

    // A preset owns the row: the stored picker pair is replaced, and the preset carries no
    // provider so its model resolves through the saved door.
    let preset = service
        .apply_role_preset(&session.id, &agent.id, "reviewer")
        .await
        .unwrap();
    assert_eq!(preset.role.as_deref(), Some("reviewer"));
    assert_eq!(preset.model.as_deref(), Some("openai/gpt-5.4-mini"));
    assert_eq!(preset.provider, None);
    assert_eq!(preset.effort.as_deref(), Some("high"));

    // Leaving the preset keeps the pair, so the next pick starts from what the row showed.
    let custom = service
        .clear_role_preset(&session.id, &agent.id)
        .await
        .unwrap();
    assert_eq!(custom.role, None);
    assert_eq!(custom.model.as_deref(), Some("openai/gpt-5.4-mini"));
    assert_eq!(custom.effort.as_deref(), Some("high"));

    // Clearing the model clears the pair rather than leaving a provider with nothing to send.
    let cleared = service
        .set_agent_config(
            &session.id,
            &agent.id,
            Some(""),
            Some("openrouter"),
            Some("high"),
        )
        .await
        .unwrap();
    assert_eq!(cleared.model, None);
    assert_eq!(cleared.provider, None);
    assert_eq!(cleared.effort, None);
}

#[tokio::test]
async fn an_unknown_effort_is_refused_and_leaves_the_row_alone() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("roster effort"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("worker"))
        .await
        .unwrap();
    service
        .set_agent_config(
            &session.id,
            &agent.id,
            Some("openai/gpt-5.4-mini"),
            Some("openrouter"),
            Some("medium"),
        )
        .await
        .unwrap();

    let error = service
        .set_agent_config(
            &session.id,
            &agent.id,
            Some("openai/gpt-5.4-mini"),
            Some("openrouter"),
            Some("maximum"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, rigel_core::SessionError::InvalidEffort(_)));

    let unchanged = service.agent(&agent.id).await.unwrap();
    assert_eq!(unchanged.effort.as_deref(), Some("medium"));
}
