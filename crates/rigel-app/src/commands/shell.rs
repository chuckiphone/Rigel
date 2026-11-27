//! Tray, usage and launch-time server commands.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Replay-ready usage for the status bar: one place computes the money and tokens.
#[tauri::command]
pub(crate) fn usage_summary(project: String, session: String) -> Result<Usage, String> {
    tauri::async_runtime::block_on(async move {
        let service = SessionService::open(Path::new(&project))
            .await
            .map_err(message)?;
        let turns = service.transcript(&session).await.map_err(message)?;
        let mut total = Usage::default();
        for turn in turns {
            if let Some(usage) = turn.turn.usage {
                total.add(&usage);
            }
        }
        Ok(total)
    })
}

/// Every local service a session can need, started as the window opens and again once the
/// remembered project is restored (the chat checkout sits beside it). The window calls this
/// once; none of the starts blocks the launch, and a service that cannot start is a status the
/// relevant tab explains rather than an error at boot.
#[tauri::command]
pub(crate) fn servers_start(app: tauri::AppHandle, project: Option<String>) {
    warm_router();
    warm_chat(project);
    warm_skills(app);
    warm_models();
}

/// Roadmap 114: the notification routing table the settings page shows. It is computed from the
/// same settings and local clock the emitter uses, so the two cannot disagree.
#[tauri::command]
pub(crate) fn notify_routing_table() -> Vec<notify::NotifyRoute> {
    let now = rigel_core::migrate::now_ms();
    let minutes = ((now / 60_000) % 1_440) as i32;
    notify::routing_table(&load_settings().notification_settings(), minutes)
}

/// Roadmap 115: pause every running turn from the tray. The step boundary is the same one the
/// per-turn Pause uses, so no response is cut in mid-stream.
#[tauri::command]
pub(crate) fn tray_pause_all(state: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    state.cancels.pause_all();
    Ok(())
}

#[tauri::command]
pub(crate) fn tray_snapshot() -> Result<tray_status::TraySnapshot, String> {
    tauri::async_runtime::block_on(tray_status::snapshot())
}

#[tauri::command]
pub(crate) fn tray_open(app: tauri::AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
pub(crate) fn tray_hide(app: tauri::AppHandle) {
    hide_tray_panel(&app);
}

#[tauri::command]
pub(crate) fn tray_quit(app: tauri::AppHandle) {
    quit(&app);
}
