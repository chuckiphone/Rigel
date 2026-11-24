//! The agent-chat room and its harness processes.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Chat tab status. `project` is the open project root (or path input); the vendor
/// checkout is discovered next to it, or via RIGEL_AGENTCHAT_DIR in the core.
#[tauri::command]
pub(crate) fn chat_status(
    project: Option<String>,
) -> Result<rigel_core::agent::agentchat::ChatStatus, String> {
    Ok(tauri::async_runtime::block_on(
        rigel_core::agent::agentchat::status(project.as_deref()),
    ))
}

/// Boot the vendor chat server when it is down. A room already answering is taken over through
/// its landing-page token, so one left behind by an earlier window stays usable.
#[tauri::command]
pub(crate) fn chat_start(
    project: Option<String>,
) -> Result<rigel_core::agent::agentchat::ChatStatus, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::ensure_server(
        project.as_deref(),
    ))
}

#[tauri::command]
pub(crate) fn chat_channels() -> Result<Vec<String>, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::channels())
}

/// Presence per instance; the vendor shape is passed through untouched.
#[tauri::command]
pub(crate) fn chat_agents() -> Result<serde_json::Value, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::agents())
}

#[tauri::command]
pub(crate) fn chat_messages(
    channel: Option<String>,
    since_id: Option<u64>,
    limit: Option<u64>,
) -> Result<Vec<rigel_core::agent::agentchat::ChatMessage>, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::messages(
        channel.as_deref(),
        since_id.unwrap_or(0),
        limit.unwrap_or(100),
    ))
}

/// One room upload as a `data:` URL. The window draws the bytes it is handed and never learns
/// the chat server's origin or token; only images under `/uploads/` are served.
#[tauri::command]
pub(crate) fn chat_attachment(url: String) -> Result<String, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::attachment(&url))
}

/// A user message over the vendor websocket, exactly like the browser does it.
#[tauri::command]
pub(crate) fn chat_send(channel: String, text: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::block_on(async move {
        rigel_core::agent::agentchat::send(&text, &channel).await?;
        let Some(project) = current_project_root() else {
            return Ok(serde_json::json!({
                "sent": true,
                "kicked": false,
                "reason": "no project is open",
            }));
        };
        let kickoff = rigel_core::swarm::kick_off_muse(&index_dir(), &project, &text).await?;
        Ok(serde_json::json!({
            "sent": true,
            "kicked": kickoff.job_id.is_some(),
            "job_id": kickoff.job_id,
            "targets": kickoff.targets,
            "missing": kickoff.missing,
            "duplicates": kickoff.duplicates,
        }))
    })
}

/// Start the room's agents on a hidden console — they work, nothing opens on screen.
/// Returns the labels launched; an empty list means every agent was already in the room.
#[tauri::command]
pub(crate) fn chat_start_agents(project: Option<String>) -> Result<Vec<String>, String> {
    let free = free_swarm_agent(&load_settings());
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::start_agents_with_free(
        project.as_deref(),
        free.as_ref(),
    ))
}

/// Swarm, Free mode: put the model Free mode chose in the room as `@rigel-free`. Returns `None`
/// when Free mode is off or the chosen door cannot be reached over an OpenAI-compatible API.
#[tauri::command]
pub(crate) fn chat_start_free(project: Option<String>) -> Result<Option<String>, String> {
    let Some(free) = free_swarm_agent(&load_settings()) else {
        return Ok(None);
    };
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::start_free_agent(
        project.as_deref(),
        &free,
    ))
    .map(Some)
}

/// The harness processes Rigel started and still holds. The roster offers Stop only for these;
/// anything else in the room was launched by hand and is the user's to close.
#[tauri::command]
pub(crate) fn chat_launched() -> Vec<rigel_core::agent::agentchat::ChatProcess> {
    rigel_core::agent::agentchat::launched()
}

/// The harnesses the checkout configures, ready or not, so the roster can offer a per-agent start
/// and say why one cannot start.
#[tauri::command]
pub(crate) fn chat_harnesses(
    project: Option<String>,
    force: Option<bool>,
) -> Vec<rigel_core::agent::agentchat::ChatHarness> {
    if force.unwrap_or(false) {
        rigel_core::agent::agentchat::configured_fresh(project.as_deref())
    } else {
        rigel_core::agent::agentchat::configured(project.as_deref())
    }
}

#[tauri::command]
pub(crate) fn chat_start_agent(project: Option<String>, name: String) -> Result<String, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::start_agent(
        project.as_deref(),
        &name,
    ))
}

/// Stop one agent Rigel started: the wrapper, its CLI and that CLI's children.
#[tauri::command]
pub(crate) fn chat_stop_agent(name: String) -> Result<String, String> {
    tauri::async_runtime::block_on(rigel_core::agent::agentchat::stop_agent(&name))
}
