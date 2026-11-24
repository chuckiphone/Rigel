//! Model doors, API credentials, model lists and the local runtimes.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Item 15: the launch health pass. The picker reads the same cached rows instead of probing
/// every door a second time.
#[tauri::command]
pub(crate) fn provider_health() -> Result<Vec<health::DoorHealth>, String> {
    tauri::async_runtime::block_on(async {
        let store = credentials::KeychainCredentialStore;
        Ok(health::launch(&store).await)
    })
}

/// Roadmap 26: the effective policy for a project, read before a turn and shown by the route
/// inspector. The file is the policy; this command does not cache a second copy.
#[tauri::command]
pub(crate) fn routing_policy(project: String) -> Result<openrouter::RoutingPolicy, String> {
    Ok(openrouter::RoutingPolicy::load(std::path::Path::new(
        &project,
    )))
}

/// Roadmap 28: the saved custom door, with header values masked.
#[tauri::command]
pub(crate) fn custom_provider_get() -> Result<Option<custom::MaskedDoor>, String> {
    Ok(custom::load()?.map(|door| door.masked()))
}

/// Roadmap 28: ask a candidate door for its models without saving anything.
#[tauri::command]
pub(crate) fn custom_provider_probe(door: custom::CustomDoor) -> Result<Vec<String>, String> {
    tauri::async_runtime::block_on(custom::probe(&door.base_url, &door.headers))
}

/// Roadmap 28: validate the probe result and write the whole record to the keychain. No header
/// value is ever written to settings or returned to the renderer.
#[tauri::command]
pub(crate) fn custom_provider_save(door: custom::CustomDoor) -> Result<custom::MaskedDoor, String> {
    credentials::set(custom::ID, &door.encode()?).map_err(message)?;
    Ok(door.masked())
}

/// Item 18: capabilities for the rows the picker is already holding.
#[tauri::command]
pub(crate) fn model_capabilities(
    models: Vec<capabilities::ModelRef>,
) -> Result<Vec<capabilities::CapabilityRow>, String> {
    Ok(capabilities::for_models(models))
}

#[tauri::command]
pub(crate) fn backend_status() -> ApiBackendStatus {
    api_backend_status()
}

/// Save the hosted/self-hosted choice. The core validates the URL before the file is replaced, so
/// a bad address leaves the previous choice and the next turn still has a working door.
#[tauri::command]
pub(crate) fn backend_set(mode: String, api_url: String) -> Result<ApiBackendStatus, String> {
    let config = rigel_core::backend::BackendConfig::from_parts(&mode, &api_url)?;
    rigel_core::backend::save_at(&index_dir(), &config)?;
    clear_model_cache();
    Ok(api_backend_status())
}

/// The relay credential, read from the field once and written to the OS keychain. The command
/// returns only whether one exists; it never reads the value back into the window.
#[tauri::command]
pub(crate) fn backend_set_token(token: String) -> Result<ApiBackendStatus, String> {
    let token = token.trim();
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err("Paste the relay credential as one line, without spaces.".to_string());
    }
    credentials::set(credentials::RELAY, token).map_err(message)?;
    Ok(api_backend_status())
}

#[tauri::command]
pub(crate) fn backend_clear_token() -> Result<ApiBackendStatus, String> {
    credentials::delete(credentials::RELAY).map_err(message)?;
    Ok(api_backend_status())
}

#[tauri::command]
pub(crate) fn providers_list() -> Result<Vec<ProviderRow>, String> {
    let store = credentials::KeychainCredentialStore;
    Ok(providers::all()
        .iter()
        .map(|spec| ProviderRow {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            hint: spec.hint.to_string(),
            keyless: providers::keyless(spec.id),
            env: credentials::provider_environment_variable(spec.id).map(str::to_string),
            access_token: spec.access_token,
            default_model: spec.default_model.map(str::to_string),
            key_url: spec.key_url.map(str::to_string),
            key_present: providers::present(&store, spec),
        })
        .collect())
}

#[tauri::command]
pub(crate) fn route_explain(
    provider: Option<String>,
    model: Option<String>,
) -> Result<RouteExplanation, String> {
    explain_route(&load_settings(), provider.as_deref(), model.as_deref())
}

#[tauri::command]
pub(crate) fn e2e_config() -> Option<E2eConfig> {
    std::env::var("RIGEL_E2E_PROJECT")
        .ok()
        .map(|project| project.trim().to_string())
        .filter(|project| !project.is_empty())
        .map(|project| E2eConfig { project })
}

#[tauri::command]
pub(crate) fn key_status() -> Result<KeyStatus, String> {
    Ok(current_key_status())
}

/// Store a key from the settings form. A chat provider's candidate is checked against its
/// roster before the keychain is touched, so a rejected key leaves the previous one in place
/// (Q244). Harness credentials are stored as before and probed by their own program.
#[tauri::command]
pub(crate) fn key_set(provider: String, value: String) -> Result<KeySetResult, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("paste a key first".to_string());
    }
    let check = if providers::find(&provider).is_some() {
        Some(tauri::async_runtime::block_on(catalog::validate_key(
            &provider, value,
        ))?)
    } else {
        None
    };
    credentials::set(&provider, value).map_err(message)?;
    clear_model_cache();
    // A harness can become ready the moment its key lands; do not keep the old verdict cached.
    rigel_core::agent::agentchat::clear_readiness_cache();
    Ok(KeySetResult {
        status: current_key_status(),
        check,
    })
}

#[tauri::command]
pub(crate) fn key_delete(provider: String) -> Result<KeyStatus, String> {
    credentials::delete(&provider).map_err(message)?;
    clear_model_cache();
    rigel_core::agent::agentchat::clear_readiness_cache();
    Ok(current_key_status())
}

#[tauri::command]
pub(crate) fn gravity_settings() -> GravityStatus {
    let settings = load_settings();
    GravityStatus {
        enabled: settings.gravity_ads,
        test_ads: settings.gravity_test_ads,
        placement_id: rigel_core::gravity::PLACEMENT_ID.to_string(),
        endpoint: rigel_core::gravity::shared_endpoint(),
    }
}

/// One ad request for the Home card. Disabled means **no request at all**: the renderer gets an
/// empty list and draws no slot. The request goes to the shared Firebase broker, so no publisher
/// key is stored on this machine.
#[tauri::command]
pub(crate) fn gravity_ad(
    session: String,
    messages: Vec<rigel_core::gravity::Message>,
    device: Option<rigel_core::gravity::Device>,
) -> Result<Vec<rigel_core::gravity::Ad>, String> {
    let settings = load_settings();
    if !settings.gravity_ads {
        return Ok(Vec::new());
    }
    let mut request = rigel_core::gravity::AdRequest::new(
        session,
        messages,
        rigel_core::gravity::install_id(&index_dir()),
    );
    if let Some(device) = device {
        request = request.with_device(device);
    }
    let answer = tauri::async_runtime::block_on(
        rigel_core::gravity::SharedGravity::configured().get_ads(&request),
    )
    .map_err(message)?;
    if answer.test && !settings.gravity_test_ads {
        return Ok(Vec::new());
    }
    eprintln!(
        "gravity: broker returned {} ad(s), test={}",
        answer.ads.len(),
        answer.test
    );
    Ok(answer.ads)
}

/// Load the impression URL once, from Rust. The webview CSP forbids third-party images and
/// connections, and the signed tracking URL should not be in the renderer anyway.
#[tauri::command]
pub(crate) fn gravity_impression(url: String) -> Result<(), String> {
    let result = tauri::async_runtime::block_on(rigel_core::gravity::track(&url));
    match &result {
        Ok(()) => eprintln!("gravity: impression recorded through Rust"),
        Err(error) => eprintln!("gravity: impression failed: {error}"),
    }
    result.map_err(message)
}

/// Record the click, then open the advertiser's landing page in the system browser. Returns the
/// landing URL for the caller's receipt; the window itself never navigates cross-origin.
#[tauri::command]
pub(crate) fn gravity_click(
    url: String,
    landing: Option<String>,
    test: Option<bool>,
) -> Result<String, String> {
    let tracked = tauri::async_runtime::block_on(rigel_core::gravity::click_through(&url));
    // A test creative carries Gravity's test host in `clickUrl`; its advertiser landing page is
    // `url`. Keep the Rust click receipt best-effort for tests, then open the real advertiser.
    let advertiser = landing.filter(|value| value.starts_with("https://"));
    let opening = match (test.unwrap_or(false), advertiser) {
        (true, Some(advertiser)) => {
            match &tracked {
                Ok(_) => eprintln!("gravity: test click recorded through Rust"),
                Err(error) => eprintln!("gravity: test click receipt skipped: {error}"),
            }
            advertiser
        }
        _ => tracked.map_err(message)?,
    };
    let host = opening
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| !host.is_empty())
        .unwrap_or("advertiser");
    eprintln!("gravity: opening {host} in the system browser");
    open_in_browser(&opening)?;
    Ok(opening)
}

/// The four harness probes, cached in `adapters.json` and refreshed when a binary changes.
/// Missing binaries come back as `installed: false` with an install hint, never as an error.
#[tauri::command]
pub(crate) fn harness_probe(force: bool) -> Result<Vec<rigel_adapters::HarnessInfo>, String> {
    let path = index_dir().join(rigel_adapters::registry::CACHE_FILE);
    let cache = tauri::async_runtime::block_on(rigel_adapters::probe_cached(
        &path,
        &credentials::KeychainCredentialStore,
        force,
    ))
    .map_err(message)?;
    Ok(cache.infos())
}

/// Whether the local router answers, and where it lives. This is a short health probe, not a
/// key test: Settings needs to know if the server is up, not whether the key is right.
#[tauri::command]
pub(crate) fn omniroute_status() -> Result<rigel_core::ServerStatus, String> {
    Ok(tauri::async_runtime::block_on(
        rigel_core::agent::omniroute::status(),
    ))
}

/// Start the router when it is down. `omniroute serve --daemon` is launched and health is
/// polled, so a success here means the next turn can actually reach it.
#[tauri::command]
pub(crate) fn omniroute_start() -> Result<rigel_core::ServerStatus, String> {
    tauri::async_runtime::block_on(rigel_core::agent::omniroute::ensure_server())
}

/// The local llama.cpp server behind the `llamacpp` door: its state, the launch mode a start
/// would use, and the one sentence that says what to do when it is down.
#[tauri::command]
pub(crate) fn local_model_status() -> Result<llamaserver::Status, String> {
    Ok(tauri::async_runtime::block_on(llamaserver::status()))
}

/// Start it, or attach to a healthy server that is already there. The core waits for `/health`,
/// so a returned `ready` means the next turn can reach the model.
#[tauri::command]
pub(crate) fn local_model_start() -> Result<llamaserver::Status, String> {
    tauri::async_runtime::block_on(llamaserver::start())
}

/// Stop the process Rigel started, after checking that the recorded PID is still the server.
#[tauri::command]
pub(crate) fn local_model_stop() -> Result<llamaserver::Status, String> {
    llamaserver::stop()?;
    Ok(tauri::async_runtime::block_on(llamaserver::status()))
}

/// The model picker's rows. The catalogue itself lives in the core (`agent::catalog`) so the
/// window and `rigel models` cannot disagree about what a provider offers.
#[tauri::command]
pub(crate) fn models_list() -> Result<Vec<ModelOption>, String> {
    models_cached(false)
}

/// Read every roster again, ignoring the cache. The launch pass and Free mode use it so a model
/// that disappeared (or appeared) upstream is seen the same day rather than after the next restart.
#[tauri::command]
pub(crate) fn models_refresh() -> Result<Vec<ModelOption>, String> {
    models_cached(true)
}
