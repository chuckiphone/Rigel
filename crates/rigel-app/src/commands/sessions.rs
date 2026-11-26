//! Session lifecycle, templates, bookmarks, attention and skill pins.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

#[tauri::command]
pub(crate) fn session_start(
    project: String,
    title: Option<String>,
    budget_cents: Option<i64>,
) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        // An explicit cap still wins; otherwise the saved default is the one place that choice
        // lives, and zero means no cap.
        let budget_cents = budget_cents.or_else(|| {
            let saved = load_settings().default_budget_cents;
            (saved > 0).then_some(saved)
        });
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .start(title.as_deref(), budget_cents)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_list(project: String) -> Result<Vec<Session>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .list()
            .await
            .map_err(message)
    })
}

/// D148: the session list's three edits. Each returns the row as stored, so the window renders
/// the database's answer instead of its own optimism.
#[tauri::command]
pub(crate) fn session_rename(
    project: String,
    id: String,
    title: Option<String>,
) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .rename(&id, title.as_deref())
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_pin(project: String, id: String, pinned: bool) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .set_pinned(&id, pinned)
            .await
            .map_err(message)
    })
}

/// Q270: the window's skill pins. The body hash is resolved from the local index unless a
/// template supplied one, so a template can restore the exact body its session read.
#[tauri::command]
pub(crate) fn session_skill_pins(
    project: String,
    session: String,
) -> Result<Vec<rigel_core::SkillPinView>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::list(&service, &index, &session)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_skill_pin(
    project: String,
    session: String,
    skill_id: String,
    body_sha: Option<String>,
) -> Result<rigel_core::SkillPinView, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        service.get(&session).await.map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        let sha = match body_sha.filter(|sha| !sha.trim().is_empty()) {
            Some(sha) => sha,
            None => index
                .body(&skill_id)
                .await
                .map_err(message)?
                .map(|(_, sha)| sha)
                .ok_or_else(|| format!("{skill_id} has no indexed body to pin"))?,
        };
        service
            .pin_skill(&session, &skill_id, &sha, false)
            .await
            .map_err(message)?;
        rigel_core::pins::one(&service, &index, &session, &skill_id)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_skill_unpin(
    project: String,
    session: String,
    skill_id: String,
) -> Result<bool, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .unpin_skill(&session, &skill_id)
            .await
            .map_err(message)
    })
}

/// D141: the full changed-pin review. Bodies stay in the index's blob store; this read only
/// returns them for the open card, never writes them into the project database.
#[tauri::command]
pub(crate) fn session_skill_review(
    project: String,
    session: String,
    skill_id: String,
) -> Result<rigel_core::SkillPinReview, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::review(&service, &index, &session, &skill_id)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_skill_keep(
    project: String,
    session: String,
    skill_id: String,
    reason: String,
) -> Result<rigel_core::SkillPinView, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::keep(&service, &index, &session, &skill_id, &reason)
            .await
            .map_err(message)
    })
}

/// D141: one reason for keeping every changed pin. The pinned hashes stay put; the core records
/// a single `skill_pin` intervention with each pin's pinned and current hash.
#[tauri::command]
pub(crate) fn session_skill_keep_all(
    project: String,
    session: String,
    reason: String,
) -> Result<Vec<rigel_core::SkillPinView>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::keep_all(&service, &index, &session, &reason)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_skill_update(
    project: String,
    session: String,
    skill_id: String,
    reason: String,
) -> Result<rigel_core::SkillPinView, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::update(&service, &index, &session, &skill_id, &reason)
            .await
            .map_err(message)
    })
}

/// D141: one reason for every changed pin. The core resolves each current hash and records a
/// single `skill_pin` intervention with all old/new pairs.
#[tauri::command]
pub(crate) fn session_skill_update_all(
    project: String,
    session: String,
    reason: String,
) -> Result<Vec<rigel_core::SkillPinView>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        rigel_core::pins::update_all(&service, &index, &session, &reason)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_archive(
    project: String,
    id: String,
    archived: bool,
) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .set_archived(&id, archived)
            .await
            .map_err(message)
    })
}

/// Q274: the window's only destructive session action. The core refuses a session that has not
/// been archived; the renderer asks for the id again before it gets this far.
#[tauri::command]
pub(crate) fn session_delete(project: String, id: String) -> Result<(), String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .delete_permanently(&id)
            .await
            .map_err(message)
    })
}

/// D154: the project's unresolved work. Open approvals lead the list as one block; their rows
/// carry the session, agent and turn the asking surface recorded, so the view can open the
/// transcript row. Everything below it is grouped by session.
#[tauri::command]
pub(crate) fn attention_list(project: String) -> Result<Attention, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let view = rigel_core::attention::attention(service.pool(), rigel_core::migrate::now_ms())
            .await
            .map_err(message)?;
        let open = ApprovalBus::new(index_dir()).pending_for(Path::new(&project));
        Ok(view.with_approvals(&open, Path::new(&project)))
    })
}

/// D152: project-local session templates. The file lives beside the session database and is
/// never committed; the three commands are the only way the window touches it.
#[tauri::command]
pub(crate) fn template_list(project: String) -> Result<Vec<SessionTemplate>, String> {
    SessionTemplates::open(Path::new(&project))
        .list()
        .map_err(message)
}

#[tauri::command]
pub(crate) fn template_save(
    project: String,
    template: SessionTemplate,
) -> Result<SessionTemplate, String> {
    SessionTemplates::open(Path::new(&project))
        .save(template)
        .map_err(message)
}

#[tauri::command]
pub(crate) fn template_delete(project: String, id: String) -> Result<(), String> {
    SessionTemplates::open(Path::new(&project))
        .delete(&id)
        .map_err(message)
}

/// D156: the recorded chain behind one patch, read-only. A missing link comes back empty; the
/// window prints "unknown" for it rather than a guess.
#[tauri::command]
pub(crate) fn patch_provenance(project: String, patch_id: String) -> Result<Provenance, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        rigel_core::provenance::patch(service.pool(), &patch_id)
            .await
            .map_err(message)
    })
}

/// D150: one list per session, and a toggle so the star is the same command on and off.
#[tauri::command]
pub(crate) fn bookmark_list(project: String, session: String) -> Result<Vec<Bookmark>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .bookmarks(&session)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn bookmark_toggle(
    project: String,
    session: String,
    kind: String,
    target_id: String,
    label: String,
) -> Result<Option<Bookmark>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .toggle_bookmark(&session, &kind, &target_id, &label)
            .await
            .map_err(message)
    })
}

/// Roadmap 103: change one bookmark's note. `None` clears it; the answer says whether a row was
/// actually updated so a stale id cannot look like a successful edit.
#[tauri::command]
pub(crate) fn bookmark_note(
    project: String,
    session: String,
    id: String,
    note: Option<String>,
) -> Result<bool, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .set_bookmark_note(&session, &id, note.as_deref())
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_get(project: String, id: String) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .get(&id)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_transcript(
    project: String,
    id: String,
) -> Result<Vec<TurnTranscript>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .transcript(&id)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn session_resume(
    project: String,
    id: String,
    force: bool,
) -> Result<ResumeOutcome, String> {
    tauri::async_runtime::block_on(async move {
        let root = PathBuf::from(&project);
        let service = SessionService::open(&root).await.map_err(message)?;
        rigel_core::resume::resume(&service, &root, &id, force)
            .await
            .map_err(message)
    })
}

/// Roadmap 37: the exact turns a dead process left running.
#[tauri::command]
pub(crate) fn session_recovery(
    project: String,
    id: String,
) -> Result<Option<RecoveryOffer>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .recovery_offer(&id)
            .await
            .map_err(message)
    })
}

/// Roadmap 37: write the tombstone and close the discarded turn in one transaction.
#[tauri::command]
pub(crate) fn turn_discard(project: String, turn: String) -> Result<Turn, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .discard_turn(&turn, "discarded after crash")
            .await
            .map_err(message)
    })
}
