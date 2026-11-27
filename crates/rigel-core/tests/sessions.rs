//! The session log is the product's spine: if these break, replay and resume are fiction.

use rigel_core::{ImagePart, SessionError, SessionService, SessionStatus};
use tempfile::TempDir;

#[tokio::test]
async fn a_session_survives_a_reopen_with_its_original_timestamp() {
    let tmp = TempDir::new().unwrap();

    let started = {
        let service = SessionService::open(tmp.path()).await.unwrap();
        service.start(Some("first"), Some(500)).await.unwrap()
    };
    assert_eq!(started.status, SessionStatus::Active);
    assert!(started.created_at > 0);

    // A second service stands in for a restarted app.
    let reopened = SessionService::open(tmp.path()).await.unwrap();
    let listed = reopened.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, started.id);
    assert_eq!(listed[0].created_at, started.created_at);
    assert_eq!(listed[0].title.as_deref(), Some("first"));
    assert_eq!(listed[0].budget_cents, Some(500));
}

#[tokio::test]
async fn sessions_list_newest_first_and_end_marks_them_ended() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();

    let first = service.start(Some("a"), None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = service.start(Some("b"), None).await.unwrap();

    let listed = service.list().await.unwrap();
    assert_eq!(listed[0].id, second.id);
    assert_eq!(listed[1].id, first.id);

    let ended = service.end(&first.id).await.unwrap();
    assert_eq!(ended.status, SessionStatus::Ended);
    assert!(ended.ended_at.is_some());

    assert!(matches!(
        service.end("missing").await,
        Err(SessionError::SessionNotFound(_))
    ));
}

/// D148: the window's list is an edit over rows that already existed. Renaming clears back to
/// the automatic title, pinning reorders without a second sort key, and archiving hides a row
/// from the default list without ending it or making it unreachable.
#[tokio::test]
async fn a_session_can_be_renamed_pinned_and_archived() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();

    let first = service.start(Some("first"), None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let second = service.start(Some("second"), None).await.unwrap();

    let renamed = service
        .rename(&second.id, Some("  release check  "))
        .await
        .unwrap();
    assert_eq!(renamed.title.as_deref(), Some("release check"));

    // One line and 120 characters: a row cannot grow into the pane it is drawn in.
    let multiline = format!("release check\n{}", "x".repeat(200));
    let clipped = service
        .rename(&second.id, Some(multiline.as_str()))
        .await
        .unwrap();
    assert_eq!(clipped.title.as_deref(), Some("release check"));
    let long = "y".repeat(200);
    let clipped = service
        .rename(&second.id, Some(long.as_str()))
        .await
        .unwrap();
    assert_eq!(clipped.title.unwrap().chars().count(), 120);

    let cleared = service.rename(&second.id, Some("   ")).await.unwrap();
    assert_eq!(cleared.title, None, "whitespace clears the name");

    let pinned = service.set_pinned(&first.id, true).await.unwrap();
    assert!(pinned.pinned_at.is_some());
    let listed = service.list().await.unwrap();
    assert_eq!(listed[0].id, first.id, "a pinned session sorts first");
    assert_eq!(listed[1].id, second.id);

    // Both edits are database rows, so a restarted window reads the same list.
    let reopened = SessionService::open(tmp.path()).await.unwrap();
    assert_eq!(
        reopened.get(&first.id).await.unwrap().pinned_at,
        pinned.pinned_at
    );

    service.set_pinned(&first.id, false).await.unwrap();
    let listed = service.list().await.unwrap();
    assert_eq!(listed[0].id, second.id, "unpinned returns to newest first");

    let archived = service.set_archived(&second.id, true).await.unwrap();
    assert!(archived.archived_at.is_some());
    assert_eq!(
        service.list().await.unwrap().len(),
        2,
        "list keeps archived rows"
    );
    assert_eq!(
        service.get(&second.id).await.unwrap().status,
        SessionStatus::Active,
        "archiving is not ending"
    );

    assert!(matches!(
        service.rename("missing", Some("x")).await,
        Err(SessionError::SessionNotFound(_))
    ));
    assert!(matches!(
        service.set_pinned("missing", true).await,
        Err(SessionError::SessionNotFound(_))
    ));
    assert!(matches!(
        service.set_archived("missing", true).await,
        Err(SessionError::SessionNotFound(_))
    ));
}

/// Roadmap 56: a name is either the user's or the cheap model's, never both. The first automatic
/// title sticks; a manual name refuses every later one; clearing the manual name opens the door
/// again. The source travels with the row so a restarted window agrees.
#[tokio::test]
async fn only_an_untitled_session_accepts_an_automatic_title() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();

    let manual = service.start(Some("mine"), None).await.unwrap();
    assert_eq!(manual.title_source.as_deref(), Some("manual"));
    assert!(
        service
            .apply_auto_title(&manual.id, "theirs")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        service.get(&manual.id).await.unwrap().title.as_deref(),
        Some("mine")
    );

    let open = service.start(None, None).await.unwrap();
    assert_eq!(open.title_source, None);
    let titled = service
        .apply_auto_title(&open.id, "  Speed up the parser.  ")
        .await
        .unwrap()
        .expect("an untitled session accepts the first automatic title");
    assert_eq!(titled.title.as_deref(), Some("Speed up the parser"));
    assert_eq!(titled.title_source.as_deref(), Some("auto"));

    assert!(
        service
            .apply_auto_title(&open.id, "a second name")
            .await
            .unwrap()
            .is_none(),
        "an automatic title does not replace another automatic title"
    );

    let cleared = service.rename(&manual.id, None).await.unwrap();
    assert_eq!(cleared.title, None);
    assert_eq!(cleared.title_source, None);
    let adopted = service
        .apply_auto_title(&manual.id, "now automatic")
        .await
        .unwrap()
        .expect("clearing the manual name lets the automatic title take over");
    assert_eq!(adopted.title_source.as_deref(), Some("auto"));

    let reopened = SessionService::open(tmp.path()).await.unwrap();
    assert_eq!(
        reopened
            .get(&open.id)
            .await
            .unwrap()
            .title_source
            .as_deref(),
        Some("auto")
    );
    assert!(matches!(
        reopened.apply_auto_title("missing", "x").await,
        Err(SessionError::SessionNotFound(_))
    ));
}

/// D150: a star is a toggle over one target per session, and the label is a single line the
/// inspector can draw without joining the item it names.
#[tokio::test]
async fn a_bookmark_toggles_and_stays_per_session() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("bookmarks"), None).await.unwrap();

    let marked = service
        .toggle_bookmark(
            &session.id,
            "item",
            "item-1",
            "turn 1 · read_file\nsecond line",
        )
        .await
        .unwrap()
        .expect("the first toggle is on");
    assert_eq!(marked.kind, "item");
    assert_eq!(marked.label, "turn 1 · read_file", "one line only");
    assert_eq!(service.bookmarks(&session.id).await.unwrap().len(), 1);

    assert!(
        service
            .toggle_bookmark(&session.id, "item", "item-1", "again")
            .await
            .unwrap()
            .is_none(),
        "starring the same target clears it"
    );
    assert!(service.bookmarks(&session.id).await.unwrap().is_empty());

    // The same id in another kind is another target: a hunk and an item share no namespace.
    service
        .toggle_bookmark(&session.id, "hunk", "item-1", "patch · @@ -1,2 +1,2 @@")
        .await
        .unwrap();
    assert_eq!(service.bookmarks(&session.id).await.unwrap().len(), 1);

    let other = service.start(None, None).await.unwrap();
    assert!(service.bookmarks(&other.id).await.unwrap().is_empty());

    assert!(matches!(
        service
            .toggle_bookmark(&session.id, "file", "item-1", "no")
            .await,
        Err(SessionError::InvalidBookmarkKind(_))
    ));
    assert!(matches!(
        service
            .toggle_bookmark("missing", "item", "item-1", "no")
            .await,
        Err(SessionError::SessionNotFound(_))
    ));
}

#[tokio::test]
async fn turns_are_sequenced_per_agent_and_close_exactly_once() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(None, None).await.unwrap();

    let codex = service
        .register_agent(&session.id, "codex", Some("implementer"))
        .await
        .unwrap();
    let claude = service
        .register_agent(&session.id, "claude", Some("reviewer"))
        .await
        .unwrap();

    let t1 = service.append_turn(&codex.id, Some("first")).await.unwrap();
    let t2 = service
        .append_turn(&codex.id, Some("second"))
        .await
        .unwrap();
    let other = service
        .append_turn(&claude.id, Some("review"))
        .await
        .unwrap();

    assert_eq!(t1.seq, 1);
    assert_eq!(t2.seq, 2);
    assert_eq!(other.seq, 1, "sequences are per agent, not global");

    let finished = service.finish_turn(&t1.id, "completed").await.unwrap();
    assert_eq!(finished.status, "completed");
    assert!(finished.completed_at.is_some());

    assert!(matches!(
        service.finish_turn(&t2.id, "done-ish").await,
        Err(SessionError::InvalidTurnStatus(_))
    ));
    assert!(matches!(
        service.append_turn("no-such-agent", None).await,
        Err(SessionError::AgentNotFound(_))
    ));
}

/// The turn row is the only place a rebuilt request can learn which image the user attached.
/// Both a remote URL and an already-inlined local image must survive a service reopen, the
/// transcript, and the history query compaction uses.
#[tokio::test]
async fn image_parts_round_trip_through_the_turn_record_and_history() {
    let tmp = TempDir::new().unwrap();
    let images = vec![
        ImagePart {
            url: "https://example.com/sign.png".to_string(),
        },
        ImagePart {
            url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
        },
    ];
    let (session_id, agent_id, turn_id) = {
        let service = SessionService::open(tmp.path()).await.unwrap();
        let session = service.start(Some("images"), Some(25)).await.unwrap();
        let agent = service
            .register_agent(&session.id, "native", Some("worker"))
            .await
            .unwrap();
        let turn = service
            .append_turn_with_images(&agent.id, Some("read the sign"), &images)
            .await
            .unwrap();
        assert_eq!(
            turn.images, images,
            "the append returns the persisted parts"
        );
        let loaded = service.turn(&turn.id).await.unwrap();
        assert_eq!(loaded.images, images, "a direct load keeps the exact parts");
        service.finish_turn(&turn.id, "completed").await.unwrap();
        (session.id, agent.id, turn.id)
    };

    // A second service stands in for an app restart.
    let reopened = SessionService::open(tmp.path()).await.unwrap();
    let current = reopened
        .append_turn(&agent_id, Some("what did the sign say?"))
        .await
        .unwrap();
    let history = reopened
        .history_for_agent(&session_id, &agent_id, &current.id, 20)
        .await
        .unwrap();
    let stored = history
        .iter()
        .find(|transcript| transcript.turn.id == turn_id)
        .expect("the image turn is in the rebuilt history");
    assert_eq!(stored.turn.images, images);

    let transcript = reopened.transcript(&session_id).await.unwrap();
    let rendered = transcript
        .iter()
        .find(|entry| entry.turn.id == turn_id)
        .expect("the image turn is in the transcript");
    assert_eq!(rendered.turn.images, images);
}

/// A text-only turn has no image field on its public shape and a NULL image column, so old
/// rows and new text-only rows carry exactly the pre-image record.
#[tokio::test]
async fn a_text_only_turn_record_is_unchanged() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("text"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service
        .append_turn(&agent.id, Some("explain this"))
        .await
        .unwrap();
    assert!(turn.images.is_empty());
    let json = serde_json::to_string(&turn).unwrap();
    assert!(!json.contains("\"images\""), "{json}");

    let stored = service.turn(&turn.id).await.unwrap();
    assert!(stored.images.is_empty());
    assert!(
        !serde_json::to_string(&stored)
            .unwrap()
            .contains("\"images\"")
    );
    let images_json: Option<String> =
        sqlx::query_scalar("SELECT images_json FROM turns WHERE id = ?")
            .bind(&turn.id)
            .fetch_one(service.pool())
            .await
            .unwrap();
    assert!(
        images_json.is_none(),
        "a text-only row keeps the old NULL shape"
    );
}

#[tokio::test]
async fn items_keep_order_and_the_raw_payload_untouched() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(None, None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "codex", None)
        .await
        .unwrap();
    let turn = service.append_turn(&agent.id, Some("go")).await.unwrap();

    let body = serde_json::json!({ "text": "hello", "unmapped": false });
    let raw = serde_json::json!({ "type": "item.completed", "item": { "type": "agent_message" } });

    service
        .append_item(&turn.id, "message", "completed", &body, &raw)
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "command",
            "completed",
            &serde_json::json!({ "command": "cargo test" }),
            &serde_json::json!({ "type": "item.completed" }),
        )
        .await
        .unwrap();

    let items = service.items(&turn.id).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].seq, 1);
    assert_eq!(items[1].seq, 2);
    assert_eq!(items[0].body, body, "body round-trips through SQLite");
    assert_eq!(items[0].kind, "message");
}

#[tokio::test]
async fn checkpoints_record_the_tree_hash_for_resume() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(None, None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", None)
        .await
        .unwrap();
    let turn = service.append_turn(&agent.id, None).await.unwrap();

    let checkpoint = service
        .checkpoint(
            &session.id,
            Some(&turn.id),
            "9c1f0d",
            &serde_json::json!({ "patches": [] }),
        )
        .await
        .unwrap();

    assert_eq!(checkpoint.tree_hash, "9c1f0d");
    assert_eq!(checkpoint.turn_id.as_deref(), Some(turn.id.as_str()));

    let listed = service.checkpoints(&session.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, checkpoint.id);
    let journal = service
        .checkpoint_journal(&checkpoint.id)
        .await
        .unwrap()
        .expect("journal round-trips");
    assert_eq!(journal["patches"], serde_json::json!([]));

    assert!(matches!(
        service
            .checkpoint("missing", None, "x", &serde_json::json!({}))
            .await,
        Err(SessionError::SessionNotFound(_))
    ));
}

#[tokio::test]
async fn applying_a_role_preset_rewrites_the_roster_row_and_rejects_unknown_names() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("roles"), None).await.unwrap();
    let agent = service
        .register_agent_with(&session.id, "native", None, None)
        .await
        .unwrap();

    let applied = service
        .apply_role_preset(&session.id, &agent.id, "reviewer")
        .await
        .unwrap();
    assert_eq!(applied.role.as_deref(), Some("reviewer"));
    assert_eq!(applied.model.as_deref(), Some("openai/gpt-5.4-mini"));

    assert!(matches!(
        service
            .apply_role_preset(&session.id, &agent.id, "nonexistent")
            .await,
        Err(SessionError::InvalidRole(_))
    ));
}

#[tokio::test]
async fn permanent_deletion_needs_an_archive_and_removes_the_session() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("delete me"), None).await.unwrap();

    let refused = service.delete_permanently(&session.id, &session.id).await;
    assert!(
        matches!(refused, Err(SessionError::NotArchived(_))),
        "a live transcript is never deleted by one call"
    );
    assert_eq!(service.list().await.unwrap().len(), 1);

    service.set_archived(&session.id, true).await.unwrap();
    assert!(matches!(
        service.delete_permanently(&session.id, "wrong id").await,
        Err(SessionError::Delete(
            rigel_core::store::delete::DeleteError::Confirmation(_)
        ))
    ));
    assert_eq!(service.list().await.unwrap().len(), 1);
    sqlx::query(
        "INSERT INTO patches (id, session_id, agent_id, path, base_hash, result_hash, diff, status, created_at) VALUES ('patch-delete', ?, 'unowned-agent', 'a', 'base', 'result', '', 'proposed', 0)",
    )
    .bind(&session.id)
    .execute(service.pool())
    .await
    .unwrap();
    let owned_blobs = tmp
        .path()
        .join(".rigel/sessions")
        .join(&session.id)
        .join("blobs");
    std::fs::create_dir_all(&owned_blobs).unwrap();
    std::fs::write(owned_blobs.join("blob"), b"private session blob").unwrap();
    let tombstones = tmp.path().join("tombstones.json");
    rigel_core::store::delete::delete_session(
        service.pool(),
        tmp.path(),
        &session.id,
        &session.id,
        &tombstones,
    )
    .await
    .unwrap();
    assert!(service.list().await.unwrap().is_empty());
    assert!(matches!(
        service.get(&session.id).await,
        Err(SessionError::SessionNotFound(_))
    ));
    let persisted = rigel_core::store::delete::read_tombstones(&tombstones).unwrap();
    assert!(persisted.sessions.contains_key(&session.id));
    let db_tombstone: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_tombstones WHERE session_id = ?")
            .bind(&session.id)
            .fetch_one(service.pool())
            .await
            .unwrap();
    assert_eq!(db_tombstone, 1);
    let patches: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM patches WHERE session_id = ?")
        .bind(&session.id)
        .fetch_one(service.pool())
        .await
        .unwrap();
    assert_eq!(patches, 0);
    assert!(!owned_blobs.exists());
}

/// The id becomes one directory name under `.rigel/sessions`; a crafted value must be rejected
/// before any row or file is touched, so a sibling's blobs cannot be removed as "session-owned".
#[tokio::test]
async fn an_unsafe_session_id_cannot_reach_a_sibling_blob_directory() {
    use rigel_core::store::delete::{DeleteError, delete_session};

    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("safe"), None).await.unwrap();
    let sibling = tmp.path().join(".rigel/sessions/neighbour");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("keep"), b"another session's blob").unwrap();
    let tombstones = tmp.path().join("unsafe-tombstones.json");

    for bad_id in [
        "../neighbour",
        "..\\neighbour",
        "/absolute",
        "nested/child",
        ".",
        "..",
    ] {
        let error = delete_session(service.pool(), tmp.path(), bad_id, bad_id, &tombstones)
            .await
            .expect_err("an unsafe id is refused");
        assert!(matches!(error, DeleteError::UnsafeSessionId(_)), "{error}");
        assert!(
            sibling.join("keep").exists(),
            "{bad_id} reached a sibling directory"
        );
    }

    assert!(
        !tombstones.exists(),
        "no tombstone was written for an unsafe id"
    );
    assert_eq!(service.get(&session.id).await.unwrap().id, session.id);
    assert_eq!(service.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn skill_pins_are_per_session_and_unpin_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("pins"), None).await.unwrap();

    service
        .pin_skill(
            &session.id,
            "github:acme/skills:pdf/SKILL.md",
            "sha-a",
            false,
        )
        .await
        .unwrap();
    service
        .pin_skill(
            &session.id,
            "github:acme/skills:review/SKILL.md",
            "sha-b",
            false,
        )
        .await
        .unwrap();

    let pins = service.skill_pins(&session.id).await.unwrap();
    assert_eq!(pins.len(), 2);
    assert!(
        pins.iter()
            .any(|pin| pin.skill_id.ends_with("pdf/SKILL.md") && pin.body_sha == "sha-a")
    );
    assert!(
        pins.iter()
            .any(|pin| pin.skill_id.ends_with("review/SKILL.md") && pin.body_sha == "sha-b")
    );

    // A load does not silently move a pin; only a deliberate repin does.
    assert_eq!(
        service
            .pin_skill(
                &session.id,
                "github:acme/skills:pdf/SKILL.md",
                "sha-new",
                false
            )
            .await
            .unwrap(),
        "sha-a"
    );
    assert_eq!(
        service
            .pin_skill(
                &session.id,
                "github:acme/skills:pdf/SKILL.md",
                "sha-new",
                true
            )
            .await
            .unwrap(),
        "sha-new"
    );

    let other = service.start(Some("other"), None).await.unwrap();
    assert!(service.skill_pins(&other.id).await.unwrap().is_empty());
    assert!(matches!(
        service.skill_pins("missing").await,
        Err(SessionError::SessionNotFound(_))
    ));

    assert!(
        !service
            .unpin_skill(&session.id, "github:acme/skills:missing/SKILL.md")
            .await
            .unwrap()
    );
    assert!(
        service
            .unpin_skill(&session.id, "github:acme/skills:pdf/SKILL.md")
            .await
            .unwrap()
    );
    assert!(
        !service
            .unpin_skill(&session.id, "github:acme/skills:pdf/SKILL.md")
            .await
            .unwrap()
    );
    assert_eq!(service.skill_pins(&session.id).await.unwrap().len(), 1);
}
