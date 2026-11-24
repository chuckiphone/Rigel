//! The agent roster, permissions and roles.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

#[tauri::command]
pub(crate) fn agent_list(project: String, session: String) -> Result<Vec<Agent>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .agents(&session)
            .await
            .map_err(message)
    })
}

/// Roadmap 66: the project ceiling, every role and every open session's agents in one table.
/// The renderer edits the project grant only; the role and agent columns are the effective
/// intersection the dispatcher will use on the next turn.
#[tauri::command]
pub(crate) fn permissions_get(
    project: String,
    session: Option<String>,
) -> Result<rigel_core::PermissionMatrix, String> {
    tauri::async_runtime::block_on(project_permissions(project, session))
}

/// Roadmap 66: validate and save the project grant. The response is the reloaded matrix, so the
/// window shows exactly what the next turn reads rather than echoing the request.
#[tauri::command]
pub(crate) fn permissions_set(
    project: String,
    session: Option<String>,
    tools: Vec<String>,
) -> Result<rigel_core::PermissionMatrix, String> {
    tauri::async_runtime::block_on(async move {
        rigel_core::ProjectPermissions::save(std::path::Path::new(&project), &tools)
            .map_err(message)?;
        project_permissions(project, session).await
    })
}

/// Roadmap 66: narrow one agent below its role. The project grant is re-read for the response,
/// and the dispatcher still intersects this override with the role and the project on every
/// call. Passing `null` removes the override; an empty array is a real denial for this agent.
#[tauri::command]
pub(crate) fn permissions_set_agent(
    project: String,
    session: String,
    agent: String,
    tools: Option<Vec<String>>,
) -> Result<rigel_core::PermissionMatrix, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(std::path::Path::new(&project))
            .await
            .map_err(message)?;
        service
            .set_agent_tool_overrides(&session, &agent, tools.as_deref())
            .await
            .map_err(message)?;
        project_permissions(project, Some(session)).await
    })
}

/// The session work board: which agent holds which files, so the roster can show it.
#[tauri::command]
pub(crate) fn agent_team(project: String, session: String) -> Result<Vec<TeamAgent>, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .team(&session)
            .await
            .map_err(message)
    })
}

/// Register an agent. Other harnesses arrive in Phase 4; saying so is better than a button
/// that quietly does nothing.
#[tauri::command]
pub(crate) fn agent_spawn(
    project: String,
    session: String,
    harness: Option<String>,
    role: Option<String>,
    model: Option<String>,
) -> Result<Agent, String> {
    let harness = harness.unwrap_or_else(|| "native".to_string());
    if !matches!(harness.as_str(), "native" | "claude") {
        return Err(format!(
            "{harness} is not wired in the window yet; native and claude are"
        ));
    }
    let model = model.unwrap_or_else(|| load_settings().model);
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .register_agent_with(&session, &harness, role.as_deref(), Some(&model))
            .await
            .map_err(message)
    })
}

/// D94: the four role presets, for the roster picker.
#[tauri::command]
pub(crate) fn role_presets() -> Vec<rigel_core::agent::RolePreset> {
    rigel_core::agent::ROLE_PRESETS.to_vec()
}

/// D94: apply a preset from the roster. The next turn reads the new role.
#[tauri::command]
pub(crate) fn role_apply(
    project: String,
    session: String,
    agent: String,
    preset: String,
) -> Result<Agent, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .apply_role_preset(&session, &agent, &preset)
            .await
            .map_err(message)
    })
}

/// Roadmap 58: the roster picker. The model and provider travel together; an empty string clears
/// the field so the row falls back to the composer.
#[tauri::command]
pub(crate) fn agent_configure(
    project: String,
    session: String,
    agent: String,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
) -> Result<Agent, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .set_agent_config(
                &session,
                &agent,
                model.as_deref(),
                provider.as_deref(),
                effort.as_deref(),
            )
            .await
            .map_err(message)
    })
}

/// Roadmap 58: leave a preset without losing the pair it resolved to.
#[tauri::command]
pub(crate) fn role_clear(project: String, session: String, agent: String) -> Result<Agent, String> {
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .clear_role_preset(&session, &agent)
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn agent_stop(
    state: State<'_, AppRuntime>,
    project: String,
    agent: String,
) -> Result<(), String> {
    state.cancels.cancel_agent(&agent);
    tauri::async_runtime::block_on(async move {
        SessionService::open(Path::new(&project))
            .await
            .map_err(message)?
            .set_agent_status(&agent, "exited")
            .await
            .map_err(message)
    })
}
