//! Network policy, session and task caps, the daily safety ledger and spend reports.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Roadmap 82: the session's safe mode. Entering is explicit, leaving needs the typed phrase,
/// and both changes are persisted under `.rigel` outside the checkpointed tree.
#[tauri::command]
pub(crate) fn safe_mode_get(project: String, session: String) -> Result<SafeModeState, String> {
    SafeModeState::load(Path::new(&project), &session).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn safe_mode_set(
    project: String,
    session: String,
    enter: bool,
    confirmation: Option<String>,
) -> Result<SafeModeState, String> {
    let mut state = safe_mode_get(project.clone(), session.clone())?;
    if enter {
        state.enter();
    } else {
        state
            .leave(confirmation.as_deref().unwrap_or_default())
            .map_err(|error| error.to_string())?;
    }
    state
        .save(Path::new(&project), &session)
        .map_err(|error| error.to_string())?;
    Ok(state)
}

/// Q240: the project-local kill switch, rules and disabled sessions. The same file is read by
/// `Tools::new` and every CLI turn, so the window and the request path cannot disagree.
#[tauri::command]
pub(crate) fn network_get(project: String) -> Result<NetworkPolicy, String> {
    Ok(NetworkPolicy::load(Path::new(&project)))
}

/// Roadmap 61: validate and normalize before writing, so a rule this build cannot enforce is an
/// error in the editor, never a row that looks saved and is ignored on the next fetch.
#[tauri::command]
pub(crate) fn network_set(project: String, policy: NetworkPolicy) -> Result<NetworkPolicy, String> {
    let root = Path::new(&project);
    if !root.is_dir() {
        return Err("open a project before changing its network policy".to_string());
    }
    policy.save(root)
}

/// Roadmap 61: the same decision `fetch_url`, `browser_open` and Exa consult, without sending.
#[tauri::command]
pub(crate) fn network_check(
    project: String,
    url: String,
    session: Option<String>,
) -> Result<rigel_core::NetworkCheck, String> {
    NetworkPolicy::load(Path::new(&project)).check_url(&url, session.as_deref())
}

#[tauri::command]
pub(crate) fn budget_get(
    project: String,
    agent: String,
) -> Result<rigel_core::BudgetSnapshot, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        BudgetService::with_null_interrupt(service.pool().clone())
            .snapshot(&agent)
            .await
            .map_err(message)
    })
}

/// Set the cap without counting an intervention (settings editing). The transcript's 80%
/// prompt uses `budget_raise`, which records one (§8.8).
#[tauri::command]
pub(crate) fn budget_set(
    project: String,
    session: String,
    cap_cents: Option<i64>,
) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let budget = BudgetService::with_null_interrupt(service.pool().clone());
        budget.set_cap(&session, cap_cents).await.map_err(message)?;
        service.get(&session).await.map_err(message)
    })
}

#[tauri::command]
pub(crate) fn budget_raise(
    project: String,
    session: String,
    cap_cents: i64,
) -> Result<Session, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let budget = BudgetService::with_null_interrupt(service.pool().clone());
        budget
            .raise_cap(&session, cap_cents)
            .await
            .map_err(message)?;
        service.get(&session).await.map_err(message)
    })
}

#[tauri::command]
pub(crate) fn safety_get(project: String) -> Result<SafetyView, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let budget = BudgetService::with_null_interrupt(service.pool().clone());
        let settings = budget.safety_settings().await.map_err(message)?;
        let daily = budget.daily_status().await.map_err(message)?;
        Ok(SafetyView {
            writable_agents: settings.writable_agents,
            daily_cap_micros: settings.daily_cap_micros,
            day: daily.day,
            today_micros: daily.spend_micros,
        })
    })
}

/// Change one knob. The daily cap takes dollars, which is what the user types; the ledger
/// stores micro-dollars, which is what every other amount uses. `clear_daily_cap` turns the
/// cap off without pretending it was set to zero.
#[tauri::command]
pub(crate) fn safety_set(
    project: String,
    session: String,
    writable_agents: Option<i64>,
    daily_cap_cents: Option<i64>,
    clear_daily_cap: Option<bool>,
) -> Result<SafetyView, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let budget = BudgetService::with_null_interrupt(service.pool().clone());
        if let Some(count) = writable_agents {
            budget.set_writable_agents(count).await.map_err(message)?;
        }
        if clear_daily_cap.unwrap_or(false) {
            budget
                .set_daily_cap(&session, None)
                .await
                .map_err(message)?;
        } else if let Some(cents) = daily_cap_cents {
            budget
                .set_daily_cap(&session, Some(cents * 10_000))
                .await
                .map_err(message)?;
        }
        let settings = budget.safety_settings().await.map_err(message)?;
        let daily = budget.daily_status().await.map_err(message)?;
        Ok(SafetyView {
            writable_agents: settings.writable_agents,
            daily_cap_micros: settings.daily_cap_micros,
            day: daily.day,
            today_micros: daily.spend_micros,
        })
    })
}

/// D92: spend across every session in this project.
#[tauri::command]
pub(crate) fn spend_report(project: String) -> Result<rigel_core::spend::SpendReport, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        rigel_core::spend::report(service.pool())
            .await
            .map_err(message)
    })
}

/// D92: cap one task (agent) below the session cap. `None` removes it.
#[tauri::command]
pub(crate) fn task_cap_set(
    project: String,
    session: String,
    agent: String,
    cap_cents: Option<i64>,
) -> Result<rigel_core::BudgetSnapshot, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let budget = BudgetService::with_null_interrupt(service.pool().clone());
        let cap = cap_cents.map(|cents| cents * 10_000);
        budget
            .set_task_cap(&session, &agent, cap)
            .await
            .map_err(message)?;
        budget.snapshot(&agent).await.map_err(message)
    })
}
