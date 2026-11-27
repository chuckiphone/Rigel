//! Per-project retention policy, preview and preview-bound cleanup (roadmap from PR #74).
//!
//! The core owns the policy and the destructive work. These commands only read it, ask for a
//! preview, and pass the exact totals back when the user confirms; a wrong confirmation is an
//! error from the core, never a partial delete.

use crate::*;
use rigel_core::retention::{self, RetentionPolicy, RetentionPreview};

#[tauri::command]
pub(crate) fn retention_get_policy(project: String) -> Result<RetentionPolicy, String> {
    retention::load_policy(Path::new(&project)).map_err(message)
}

#[tauri::command]
pub(crate) fn retention_set_policy(
    project: String,
    policy: RetentionPolicy,
) -> Result<RetentionPolicy, String> {
    retention::save_policy(Path::new(&project), policy).map_err(message)?;
    retention_get_policy(project)
}

#[tauri::command]
pub(crate) fn retention_preview(project: String) -> Result<RetentionPreview, String> {
    tauri::async_runtime::block_on(async move {
        let root = PathBuf::from(&project);
        let store = Store::open_project(&root).await.map_err(message)?;
        retention::preview(&store, &root, &index_dir(), rigel_core::migrate::now_ms())
            .await
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn retention_apply(
    project: String,
    confirm_count: u64,
    confirm_bytes: u64,
) -> Result<RetentionPreview, String> {
    tauri::async_runtime::block_on(async move {
        let root = PathBuf::from(&project);
        let store = Store::open_project(&root).await.map_err(message)?;
        retention::apply(
            &store,
            &root,
            &index_dir(),
            rigel_core::migrate::now_ms(),
            confirm_count,
            confirm_bytes,
        )
        .await
        .map_err(message)
    })
}
