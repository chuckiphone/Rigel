//! Roadmap 108: the real probe commands behind first-run setup.

use crate::*;

/// OS/architecture/family, straight from the compiled target.
#[tauri::command]
pub(crate) fn setup_probe_os() -> setup::SetupFinding {
    setup::probe_os()
}

/// The configured or environment shell. Settings own the value; the probe never guesses readiness.
#[tauri::command]
pub(crate) fn setup_probe_shell() -> setup::SetupFinding {
    setup::probe_shell(&load_settings().terminal_shell)
}

/// `git --version`, or the reason it failed.
#[tauri::command]
pub(crate) fn setup_probe_git() -> setup::SetupFinding {
    setup::probe_git()
}

/// The provider rows with their real key-presence flags, from the same command Settings uses.
#[tauri::command]
pub(crate) fn setup_probe_providers() -> Result<Vec<ProviderRow>, String> {
    commands::providers::providers_list()
}

/// The harness rows from the cached real probe. `force` is the explicit re-check button.
#[tauri::command]
pub(crate) fn setup_probe_harnesses(
    force: Option<bool>,
) -> Result<Vec<rigel_adapters::HarnessInfo>, String> {
    commands::providers::harness_probe(force.unwrap_or(false))
}
