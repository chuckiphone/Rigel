use rigel_core::{
    PROJECT_MIGRATIONS, SessionService, Store, apply,
    search::{SearchQuery, search_history},
};
use tempfile::TempDir;

#[tokio::test]
async fn searches_prompts_answers_commands_and_patches_with_filters_and_stable_order() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("history search"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("builder"))
        .await
        .unwrap();
    let turn = service
        .append_turn(
            &agent.id,
            Some("commonfilter promptsearch distinctive prompt"),
        )
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "message",
            "completed",
            &serde_json::json!({ "text": "commonfilter answersearch distinctive answer" }),
            &serde_json::json!({ "type": "message" }),
        )
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "command",
            "completed",
            &serde_json::json!({ "command": "commonfilter commandsearch cargo test" }),
            &serde_json::json!({ "type": "command" }),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO patches
           (id, session_id, agent_id, path, base_hash, result_hash, diff, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("patch-search-test")
    .bind(&session.id)
    .bind(&agent.id)
    .bind("src/patchsearch.rs")
    .bind("base")
    .bind("result")
    .bind("@@ -1 +1 @@\n-old\n+commonfilter patchsearch replacement")
    .bind("applied")
    .bind(4_i64)
    .execute(service.pool())
    .await
    .unwrap();
    // Filters use current project, agent, and model metadata even for already indexed rows.
    sqlx::query("UPDATE agents SET model = ? WHERE id = ?")
        .bind("model-search-test")
        .bind(&agent.id)
        .execute(service.pool())
        .await
        .unwrap();

    for needle in [
        "promptsearch",
        "answersearch",
        "commandsearch",
        "patchsearch",
    ] {
        let query = SearchQuery {
            text: needle.to_string(),
            ..SearchQuery::default()
        };
        let results = search_history(service.pool(), &query).await.unwrap();
        assert_eq!(results.len(), 1, "query {needle}");
        assert!(results[0].snippet.contains(&format!("⟦{needle}⟧")));
    }

    let filtered = SearchQuery {
        text: "commonfilter".to_string(),
        project: Some(session.project_root.clone()),
        agent: Some(agent.id.clone()),
        model: Some("model-search-test".to_string()),
        after_ms: Some(0),
        before_ms: Some(i64::MAX),
        ..SearchQuery::default()
    };
    let first = search_history(service.pool(), &filtered).await.unwrap();
    let second = search_history(service.pool(), &filtered).await.unwrap();
    assert_eq!(first, second, "identical searches have stable ordering");
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|hit| {
        hit.session_id == session.id
            && hit.agent_id == agent.id
            && hit.model == "model-search-test"
            && hit.project_root == session.project_root
    }));

    let excluded = SearchQuery {
        model: Some("other-model".to_string()),
        ..filtered.clone()
    };
    assert!(
        search_history(service.pool(), &excluded)
            .await
            .unwrap()
            .is_empty()
    );
    for excluded in [
        SearchQuery {
            project: Some("another-project".to_string()),
            ..filtered.clone()
        },
        SearchQuery {
            agent: Some("another-agent".to_string()),
            ..filtered.clone()
        },
        SearchQuery {
            after_ms: Some(i64::MAX),
            ..filtered
        },
    ] {
        assert!(
            search_history(service.pool(), &excluded)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn migration_backfills_existing_rows_and_source_triggers_track_edits_and_deletes() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("history.db"), &PROJECT_MIGRATIONS[..27])
        .await
        .unwrap();
    let service = SessionService::from_store(&store, tmp.path());
    let session = service.start(Some("history backfill"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("builder"))
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("backfillprompt"))
        .await
        .unwrap();
    let item = service
        .append_item(
            &turn.id,
            "message",
            "completed",
            &serde_json::json!({ "text": "originalanswer" }),
            &serde_json::json!({ "type": "message" }),
        )
        .await
        .unwrap();
    apply(store.pool(), PROJECT_MIGRATIONS).await.unwrap();

    let prompt = SearchQuery {
        text: "backfillprompt".to_string(),
        ..SearchQuery::default()
    };
    assert_eq!(
        search_history(store.pool(), &prompt).await.unwrap().len(),
        1
    );
    let original = SearchQuery {
        text: "originalanswer".to_string(),
        ..SearchQuery::default()
    };
    assert_eq!(
        search_history(store.pool(), &original).await.unwrap().len(),
        1
    );

    sqlx::query("UPDATE items SET body_json = ? WHERE id = ?")
        .bind(r#"{"text":"updatedanswer"}"#)
        .bind(&item.id)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        search_history(store.pool(), &original)
            .await
            .unwrap()
            .is_empty()
    );
    let updated = SearchQuery {
        text: "updatedanswer".to_string(),
        ..SearchQuery::default()
    };
    assert_eq!(
        search_history(store.pool(), &updated).await.unwrap().len(),
        1
    );

    sqlx::query("DELETE FROM turns WHERE id = ?")
        .bind(turn.id)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        search_history(store.pool(), &prompt)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        search_history(store.pool(), &updated)
            .await
            .unwrap()
            .is_empty()
    );
}
