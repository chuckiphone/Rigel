//! D92: spend is attributable across sessions, and a task cap is narrower than a session cap.

use rigel_core::{BudgetError, BudgetService, SessionService, Usage, spend};

fn usage(micros: i64) -> Usage {
    Usage {
        input_tokens: 10,
        output_tokens: 5,
        cost_micros: micros,
        ..Usage::default()
    }
}

#[tokio::test]
async fn spend_sums_every_session_and_groups_by_day() {
    let tmp = tempfile::TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let first = service.start(Some("first"), None).await.unwrap();
    let second = service.start(Some("second"), None).await.unwrap();

    let a = service
        .register_agent_with(
            &first.id,
            "native",
            Some("implementer"),
            Some("openrouter/fast"),
        )
        .await
        .unwrap();
    let b = service
        .register_agent_with(
            &second.id,
            "native",
            Some("reviewer"),
            Some("openrouter/deep"),
        )
        .await
        .unwrap();
    let budget = BudgetService::with_null_interrupt(service.pool().clone());

    for (agent, micros) in [(&a.id, 20_000), (&a.id, 30_000), (&b.id, 60_000)] {
        let turn = service.append_turn(agent, Some("go")).await.unwrap();
        budget
            .account(agent, &turn.id, &usage(micros))
            .await
            .unwrap();
    }

    let report = spend::report(service.pool()).await.unwrap();
    assert_eq!(report.total_micros, 110_000);
    assert_eq!(report.turns, 3);
    assert_eq!(report.by_session.len(), 2);
    assert_eq!(report.by_session[0].label, "second", "largest first");
    assert_eq!(
        report.by_day.iter().map(|row| row.micros).sum::<i64>(),
        110_000
    );
    assert_eq!(
        report.by_agent.iter().map(|row| row.micros).sum::<i64>(),
        110_000
    );
    assert_eq!(
        report.by_model.iter().map(|row| row.micros).sum::<i64>(),
        110_000,
        "the model breakdown reconciles with the stored usage rows"
    );
    assert!(
        report
            .by_model
            .iter()
            .any(|row| row.key == "openrouter/fast" && row.micros == 50_000)
    );
}

#[tokio::test]
async fn spend_reconciles_to_the_micro_dollar_against_stored_usage_rows() {
    // Roadmap 136: the cost dashboard must not round, re-derive or double count a charge. The
    // stored usage rows are the authority; every grouping must add back to exactly the same sum.
    let tmp = tempfile::tempdir().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let first = service.start(Some("ledger"), None).await.unwrap();
    let second = service.start(Some("other ledger"), None).await.unwrap();
    let a = service
        .register_agent_with(
            &first.id,
            "native",
            Some("writer"),
            Some("openrouter/writer"),
        )
        .await
        .unwrap();
    let b = service
        .register_agent_with(
            &first.id,
            "native",
            Some("reviewer"),
            Some("openrouter/reviewer"),
        )
        .await
        .unwrap();
    let c = service
        .register_agent_with(
            &second.id,
            "native",
            Some("writer"),
            Some("openrouter/writer"),
        )
        .await
        .unwrap();
    let budget = BudgetService::with_null_interrupt(service.pool().clone());

    let charges: [(&str, i64); 5] = [
        (&a.id, 1),
        (&a.id, 12),
        (&b.id, 1_234),
        (&c.id, 56_789),
        (&c.id, 4),
    ];
    for (agent, micros) in charges {
        let turn = service
            .append_turn(agent, Some("billable step"))
            .await
            .unwrap();
        budget
            .account(agent, &turn.id, &usage(micros))
            .await
            .unwrap();
    }
    // Two calendar days, so a day-grouping mistake cannot hide inside one bucket.
    sqlx::query(
        "UPDATE turns SET started_at = ? WHERE id = (SELECT id FROM turns ORDER BY seq LIMIT 1)",
    )
    .bind(1_735_689_600_000_i64)
    .execute(service.pool())
    .await
    .unwrap();

    let stored: Vec<String> =
        sqlx::query_scalar("SELECT usage_json FROM turns WHERE usage_json IS NOT NULL")
            .fetch_all(service.pool())
            .await
            .unwrap();
    let db_total: i64 = stored
        .iter()
        .filter_map(|text| serde_json::from_str::<Usage>(text).ok())
        .map(|row| row.cost_micros)
        .sum();
    assert_eq!(db_total, 58_040, "the fixture's exact stored sum");

    let report = spend::report(service.pool()).await.unwrap();
    assert_eq!(report.total_micros, db_total);
    assert_eq!(report.turns, stored.len() as i64);
    for (name, rows) in [
        ("session", &report.by_session),
        ("agent", &report.by_agent),
        ("day", &report.by_day),
        ("model", &report.by_model),
    ] {
        assert_eq!(
            rows.iter().map(|row| row.micros).sum::<i64>(),
            db_total,
            "{name} grouping lost or invented micros"
        );
        assert_eq!(
            rows.iter().map(|row| row.turns).sum::<i64>(),
            report.turns,
            "{name} grouping lost or invented turns"
        );
    }
    assert_eq!(report.by_day.len(), 2);
    assert_eq!(
        report
            .by_model
            .iter()
            .find(|row| row.key == "openrouter/writer")
            .unwrap()
            .micros,
        56_806,
        "the shared model groups both agents' charges exactly"
    );
}

#[tokio::test]
async fn a_task_cap_refuses_the_next_turn_but_the_session_can_continue() {
    let tmp = tempfile::TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service
        .start(Some("capped task"), Some(10_000))
        .await
        .unwrap();
    let agent = service
        .register_agent_with(&session.id, "native", Some("implementer"), None)
        .await
        .unwrap();
    let budget = BudgetService::with_null_interrupt(service.pool().clone());

    budget
        .set_task_cap(&session.id, &agent.id, Some(15_000))
        .await
        .unwrap();
    let snapshot = budget.snapshot(&agent.id).await.unwrap();
    assert_eq!(snapshot.task_cap_micros, Some(15_000));

    let turn = service.append_turn(&agent.id, Some("go")).await.unwrap();
    budget
        .account(&agent.id, &turn.id, &usage(14_000))
        .await
        .unwrap();
    // The one-cent reserve would cross the task cap, while the $1.00 session cap is far away.
    match budget.reserve(&agent.id).await {
        Err(BudgetError::TaskExhausted { cap_micros, .. }) => assert_eq!(cap_micros, 15_000),
        other => panic!("expected a task-cap refusal, got {other:?}"),
    }

    budget
        .set_task_cap(&session.id, &agent.id, None)
        .await
        .unwrap();
    budget
        .reserve(&agent.id)
        .await
        .expect("clearing the cap frees the task");
}
