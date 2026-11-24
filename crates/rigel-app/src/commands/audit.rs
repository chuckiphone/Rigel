//! The Audit log page's read side (roadmap 83).
//!
//! Both commands resolve the project the same way as every other project command, then hand the
//! filter to `rigel-core::audit::log`. The core owns matching and CSV formatting, so the page and
//! its export always carry the same rows.

use crate::*;

#[tauri::command]
pub(crate) fn audit_query(
    project: String,
    filter: rigel_core::audit::AuditFilter,
) -> Result<rigel_core::audit::AuditView, String> {
    let root = std::fs::canonicalize(&project).map_err(message)?;
    rigel_core::audit::query(&root, &filter).map_err(message)
}

#[tauri::command]
pub(crate) fn audit_export(
    project: String,
    filter: rigel_core::audit::AuditFilter,
) -> Result<rigel_core::audit::FilteredExport, String> {
    let root = std::fs::canonicalize(&project).map_err(message)?;
    rigel_core::audit::export_csv(&root, &filter).map_err(message)
}
