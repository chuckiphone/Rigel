//! Patch review, attributed hunks and line comments.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

use crate::commands::turns::turn_send;

#[tauri::command]
pub(crate) fn patch_list(project: String, session: String) -> Result<Vec<Patch>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        PatchPipeline::new(Path::new(&project), service.pool().clone())
            .list(&session)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn patch_hunks(project: String, session: String) -> Result<Vec<AttributedHunk>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        PatchPipeline::new(Path::new(&project), service.pool().clone())
            .attributed_hunks(&session)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn hunk_comment_add(
    project: String,
    hunk: String,
    body: String,
    author: Option<String>,
) -> Result<HunkComment, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let comments = HunkComments::new(service.pool().clone());
        comments
            .add(&hunk, &body, author.as_deref().unwrap_or("user"))
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn hunk_comments(project: String, session: String) -> Result<Vec<HunkComment>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let comments = HunkComments::new(service.pool().clone());
        comments.for_session(&session).await.map_err(message)
    })
}

/// Send a comment to the agent that wrote the hunk, as a turn. The comment is marked
/// delivered with the turn's id, so a failure to record that leaves it pending and visible
/// rather than silently answered.
#[tauri::command]
pub(crate) fn hunk_comment_deliver(
    app: tauri::AppHandle,
    state: State<'_, AppRuntime>,
    project: String,
    session: String,
    id: String,
) -> Result<TurnReply, String> {
    let service = tauri::async_runtime::block_on(SessionService::open(Path::new(&project)))
        .map_err(message)?;
    let comments = HunkComments::new(service.pool().clone());
    let comment = tauri::async_runtime::block_on(comments.get(&id))
        .map_err(message)?
        .ok_or_else(|| format!("no comment {id}"))?;

    let reply = turn_send(
        app,
        state,
        project,
        session,
        comment.agent_id.clone(),
        comment.prompt(),
        None,
        None,
        None,
        None,
    )?;

    tauri::async_runtime::block_on(comments.deliver(&id, &reply.turn.id)).map_err(message)?;
    Ok(reply)
}

#[tauri::command]
pub(crate) fn patch_revert(project: String, id: String) -> Result<Patch, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        PatchPipeline::new(Path::new(&project), service.pool().clone())
            .revert(&id)
            .await
            .map_err(message)
    })
}
