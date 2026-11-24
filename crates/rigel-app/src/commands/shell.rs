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
