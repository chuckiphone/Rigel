//! Settings, updates, the code graph and Free mode.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Tighten a composer draft before it is sent, on that fixed cheap, tiered model. It returns questions
/// and a proposed brief and never sends anything: the window shows both as a review card and the
/// send button stays the user's. This call is outside any session budget, so the card shows what
/// it cost on its own.
#[tauri::command]
pub(crate) fn prompt_refine(
    project: Option<String>,
    request: rigel_core::agent::RefineRequest,
) -> Result<rigel_core::agent::Refinement, String> {
    let model_name = REFINE_MODEL.to_string();
    // The composer's effort is the tier a tiered model runs at; it stays on the wire, not in
    // the id.
    let effort = load_settings().effort;
    let model: Arc<dyn Model> = tauri::async_runtime::block_on(providers::build(
        REFINE_PROVIDER,
        &credentials::KeychainCredentialStore,
    ))
    .map_err(|error| {
        format!("the refiner could not start on {REFINE_PROVIDER}/{REFINE_MODEL}: {error}")
    })?;

    let refiner = rigel_core::agent::PromptRefiner::new(model, model_name).with_effort(effort);
    let root = project.map(PathBuf::from);
    let outcome = tauri::async_runtime::block_on(async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            refiner.refine(&request, root.as_deref()),
        )
        .await
        .map_err(|_| "the refiner took too long; send the draft as it is".to_string())?
    })?;
    // A retired Antigravity client answers with an upgrade notice instead of JSON. Treat it as
    // a failed refinement: auto-refine must send the draft the user typed, never that notice.
    let unavailable = outcome.refined.to_ascii_lowercase();
    if unavailable.contains("antigravity") && unavailable.contains("no longer supported") {
        return Err(
            "the installed Antigravity version is no longer supported; update it at \
             https://antigravity.google/download"
                .to_string(),
        );
    }
    Ok(outcome)
}

#[tauri::command]
pub(crate) fn settings_get() -> Result<Settings, String> {
    Ok(load_settings())
}

/// Write settings, creating the app data directory if this is a first run.
#[tauri::command]
pub(crate) fn settings_set(
    app: tauri::AppHandle,
    mut settings: Settings,
) -> Result<Settings, String> {
    if providers::find(&settings.provider).is_none() {
        return Err(providers::unknown_provider(&settings.provider));
    }
    // `Vertex` and ` vertex ` name the same door; the file should say so, because a hand-edited
    // settings file is otherwise reported as an unknown provider on the next note.
    settings.provider = settings.provider.trim().to_ascii_lowercase();
    settings.model = settings.model.trim().to_string();
    if !valid_effort(&settings.effort) {
        return Err(format!(
            "unknown effort `{}`; use low, medium or high",
            settings.effort
        ));
    }
    settings.swarm_lead = settings.swarm_lead.trim().to_ascii_lowercase();
    if !matches!(settings.swarm_lead.as_str(), "muse" | "broadcast") {
        return Err(format!(
            "unknown swarm lead `{}`; use muse or broadcast",
            settings.swarm_lead
        ));
    }
    // A settings file is editable by hand, so the numbers are clamped here rather than trusted:
    // a 3-pixel terminal or a 10 000-step turn is a bug report, not a preference.
    settings.terminal_font_size = settings.terminal_font_size.clamp(10, 20);
    settings.terminal_scrollback = settings.terminal_scrollback.clamp(1_000, 100_000);
    settings.terminal_shell = settings.terminal_shell.trim().to_string();
    settings.max_steps = settings.max_steps.clamp(1, 50);
    // D159: four steps, so a hand-edited 117 is a mistake rather than a new mode.
    if !matches!(settings.ui_scale, 90 | 100 | 110 | 125) {
        settings.ui_scale = default_ui_scale();
    }
    settings.vertex_project = settings.vertex_project.trim().to_string();

    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(message)?;
    }
    let text = toml::to_string_pretty(&settings).map_err(message)?;
    fs::write(&path, text).map_err(message)?;
    // The next turn reads this from the core, not from the file, so a save takes effect at once.
    vertex::set_project_override(Some(&settings.vertex_project));
    // Turning the switch on starts the freshness loop without a restart. It is idempotent: the
    // loop already running from launch is not spawned a second time.
    if settings.skills_auto_update {
        warm_skills(app);
    }
    Ok(settings)
}

/// Free mode: test the best-benchmarked tool-capable free models and save the best one that
/// answers.
///
/// The result carries every attempt, so the window can say what it skipped and why (a 429 from a
/// shared pool reads differently from a 401 on a key). Saving goes through `settings_set`, which
/// is the same path a manual pick uses, so the composer and the next turn cannot disagree.
#[tauri::command]
pub(crate) async fn free_mode(
    app: tauri::AppHandle,
) -> Result<rigel_core::agent::FreeChoice, String> {
    // This command genuinely spends minutes refreshing provider rosters and probing candidates.
    // A synchronous command runs on the window's main thread, so `block_on` here froze the UI for
    // the whole scan; an async command runs on the runtime instead and the button can show its
    // "scanning" state while the window stays responsive.
    let current = load_settings().provider.trim().to_lowercase();
    let choice = tokio::time::timeout(
        std::time::Duration::from_secs(285),
        rigel_core::agent::freemode::pick(&current, &credentials::KeychainCredentialStore),
    )
    .await
    .map_err(|_| "Free mode reached its five-minute limit; no model was changed".to_string())??;
    let mut settings = load_settings();
    settings.provider = choice.provider.clone();
    settings.model = choice.model.clone();
    settings.free_mode = true;
    settings_set(app, settings)?;
    // The picker must show the same fresh free/paid labels the selection pass just used. Clear the
    // old minute cache; its next open performs a live read with the persisted winning provider first.
    clear_model_cache();
    Ok(choice)
}

/// The project's code graph: files, symbols and the edges between them. Read-only, bounded and
/// local; the window renders exactly what the scanner saw.
#[tauri::command]
pub(crate) fn code_graph(
    project: String,
    focus: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
) -> Result<rigel_core::codegraph::CodeGraph, String> {
    let root = PathBuf::from(&project);
    if project.trim().is_empty() || !root.is_dir() {
        return Err("Open a project before opening its graph.".to_string());
    }
    let mut graph = rigel_core::codegraph::snapshot(
        &root,
        focus.as_deref(),
        depth.unwrap_or(2),
        limit.unwrap_or(700),
    )
    .map_err(message)?;
    // The background indexer keeps the concept layer; the on-demand scan is still the authority
    // for files and symbols. Merge only concepts that still point at a live file.
    if focus.as_deref().unwrap_or("").trim().is_empty()
        && let Some(cached) = rigel_core::codegraph::watch::read_graph(&root)
    {
        rigel_core::codegraph::watch::merge_concepts(&mut graph, &cached);
    }
    Ok(graph)
}

#[tauri::command]
pub(crate) fn code_graph_watch_status(
    project: String,
) -> Option<rigel_core::codegraph::watch::WatchStatus> {
    let root = PathBuf::from(project);
    if let Ok(guard) = graph_status_slot().lock()
        && let Some(status) = guard.as_ref()
        && Path::new(&status.project) == root.as_path()
    {
        return Some(status.clone());
    }
    rigel_core::codegraph::watch::read_status(&root)
}

/// The live zero-priced chat rows, ranked, without probing them. The picker and Free mode use
/// the same roster; the background warmer refreshes it every three minutes so this list is never
/// a stale snapshot from launch.
#[tauri::command]
pub(crate) fn free_agents() -> Result<Vec<ModelOption>, String> {
    tauri::async_runtime::block_on(async {
        let settings = load_settings();
        let provider = settings.provider.trim().to_lowercase();
        let rows = catalog::roster(&provider, &credentials::KeychainCredentialStore).await?;
        Ok(rigel_core::agent::freemode::candidates(&provider, rows))
    })
}

#[tauri::command]
pub(crate) fn update_status() -> Result<UpdateStatus, String> {
    update_status_now()
}

/// The two switches and an optional manifest override, saved beside the app data. The manifest
/// URL is https-only; the default is the latest GitHub release's signed `latest.json`.
#[tauri::command]
pub(crate) fn update_set(
    enabled: bool,
    auto_install: bool,
    manifest_url: Option<String>,
) -> Result<UpdateStatus, String> {
    if let Some(url) = manifest_url.as_deref()
        && !url.trim().is_empty()
        && !url.starts_with("https://")
    {
        return Err("the update manifest must be an https URL".to_string());
    }
    // The window has no URL field, so `null` means "keep whatever the CLI configured". An
    // explicit empty string clears it and returns to the GitHub default.
    let manifest_url = match manifest_url {
        Some(url) if url.trim().is_empty() => None,
        Some(url) => Some(url),
        None => rigel_core::update::load_settings(&index_dir()).manifest_url,
    };
    let settings = rigel_core::update::UpdateSettings {
        enabled,
        auto_install,
        manifest_url,
    };
    rigel_core::update::save_settings(&index_dir(), &settings).map_err(message)?;
    update_status_now()
}

/// One check, made only when the window asks. The core verifies the Ed25519 signature before it
/// reads the version, then compares the signed version with this build.
#[tauri::command]
pub(crate) async fn update_check() -> Result<rigel_core::UpdateCheck, String> {
    rigel_core::update::check_latest(&index_dir(), env!("CARGO_PKG_VERSION"))
        .await
        .map_err(message)
}

/// Download the signed installer, verify its hash, start it and close the window so the installer
/// can replace the executable. A per-machine MSI install uses the MSI twin when the manifest has
/// one; a per-user NSIS install uses the setup executable.
#[tauri::command]
pub(crate) async fn update_install(app: tauri::AppHandle) -> Result<String, String> {
    let dir = index_dir();
    let check = rigel_core::update::check_latest(&dir, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(message)?;
    let Some((url, sha256)) =
        rigel_core::update::available_artifact(&check, installed_per_machine())
    else {
        return Err("this build is already current".to_string());
    };
    let path = rigel_core::update::download_artifact(url, sha256, &dir)
        .await
        .map_err(message)?;
    rigel_core::update::install_artifact(&path).map_err(message)?;

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        quit(&handle);
    });
    Ok(format!(
        "Installing {} — Rigel will close while the installer runs.",
        check.offered
    ))
}
