//! Claims, conflicts, merges and writable agent trees.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

#[tauri::command]
pub(crate) fn claim_list(project: String) -> Result<CoordinationView, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let claims = ClaimService::new(service.pool().clone(), &root);
        Ok(CoordinationView {
            claims: claims.held(None).await.map_err(message)?,
            unverified_changes: claims.unverified().await.map_err(message)?,
            conflicts: claims.conflicts().await.map_err(message)?,
        })
    })
}

/// Roadmap 95: release a claim by explicit user action only. A stale heartbeat is reported by
/// `claim_list` and never released here on a timer; the reason travels with the release into the
/// ledger. The core refuses an outside path and keeps the released row as a tombstone.
#[tauri::command]
pub(crate) fn claim_release(
    project: String,
    agent: String,
    paths: Vec<String>,
    reason: String,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        ClaimService::new(service.pool().clone(), &root)
            .release(&agent, &paths, &reason)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn conflict_resolve(
    project: String,
    id: String,
    resolution: String,
) -> Result<(), String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        ClaimService::new(service.pool().clone(), &root)
            .resolve_conflict(&id, &resolution)
            .await
            .map_err(message)
    })
}

/// The three rendered versions of a conflict (D79). Read once, when the row is expanded; the
/// two-second coordination poll must not carry whole files.
#[tauri::command]
pub(crate) fn conflict_sides(
    project: String,
    id: String,
) -> Result<rigel_core::ConflictSides, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        ClaimService::new(service.pool().clone(), &root)
            .sides(&id)
            .await
            .map_err(message)
    })
}

/// Merge the three sides and return the text, writing nothing. The Diff view edits this, then
/// hands it back through `conflict_resolve_merged`.
#[tauri::command]
pub(crate) fn conflict_merge(
    project: String,
    id: String,
) -> Result<rigel_core::MergedConflict, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        ClaimService::new(service.pool().clone(), &root)
            .merge(&id)
            .await
            .map_err(message)
    })
}

/// Write the reviewed merged text through the CAS patch pipeline and resolve the conflict (D79).
/// The patch is attributed to `user`; the verdict names the reviewer and carries one decision for
/// every conflicted hunk. A marker block is refused here even if a caller bypasses the UI.
#[tauri::command]
pub(crate) fn conflict_resolve_merged(
    project: String,
    session: String,
    id: String,
    merged: String,
    verdict: rigel_core::ConflictVerdictInput,
) -> Result<rigel_core::MergeResolution, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let pipeline = PatchPipeline::new(&root, service.pool().clone());
        ClaimService::new(service.pool().clone(), &root)
            .resolve_with_merge(&pipeline, &session, &id, &merged, &verdict)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn merge_queue(project: String) -> Result<rigel_core::MergeQueueView, String> {
    let root = std::fs::canonicalize(&project).map_err(message)?;
    Ok(rigel_core::agent::merge_queue::snapshot(&root))
}

#[tauri::command]
pub(crate) fn writable_list(project: String, session: String) -> Result<Vec<WritableView>, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let reviews = MergeReviews::new(service.pool().clone());
        let mut out = Vec::new();
        for row in service.writable_agents(&session).await.map_err(message)? {
            let review = reviews.latest(&row.agent.id).await.map_err(message)?;
            out.push(WritableView {
                agent_id: row.agent.id,
                role: row.agent.role,
                model: row.agent.model,
                status: row.agent.status,
                worktree: row.agent.worktree,
                plan: row.plan,
                plan_note: row.plan_note,
                review,
            });
        }
        Ok(out)
    })
}

#[tauri::command]
pub(crate) fn writable_diff(project: String, agent: String) -> Result<WritableDiff, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        if !service.is_writable(&agent).await.map_err(message)? {
            return Err(format!("{agent} is not a writable subagent"));
        }
        let data = index_dir();
        let worktrees = WorktreeService::new(&root, &data, service.pool().clone());
        let (diff, files) = worktrees.diff(&agent).await.map_err(message)?;
        Ok(WritableDiff {
            agent_id: agent,
            files,
            diff,
        })
    })
}

/// The Diff view's decision about a recorded review: merge an approved one that a conflict
/// interrupted, or cross a block with a typed reason (D115). No model is called — the verdict
/// stands as written, the reason is stored beside it, and the worktree must still hold exactly
/// the diff the verdict was about.
#[tauri::command]
pub(crate) fn writable_merge(
    project: String,
    session: String,
    agent: String,
    reason: Option<String>,
) -> Result<AppliedReview, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let data = index_dir();
        let worktrees = WorktreeService::new(&root, &data, service.pool().clone());
        let reviews = MergeReviews::new(service.pool().clone());
        apply_review(
            &service,
            &worktrees,
            &reviews,
            &session,
            &agent,
            reason.as_deref(),
        )
        .await
        .map_err(message)
    })
}
