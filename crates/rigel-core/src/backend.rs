//! Which Rigel API this installation talks to.
//!
//! The `rigel` model door and the desktop relay bridge must agree: a machine cannot read its
//! turns from one API and post its room events to another. This module is the one saved choice
//! both read. It is deliberately small and has no secret in it — the relay credential stays in
//! the OS keychain under `rigel/relay`.
//!
//! Two placements are possible:
//!
//!   * [`HOSTED`] — the API operated at `https://api.askrigel.com`; no URL is stored.
//!   * [`SELF_HOSTED`] — the operator's own deployment, with the base URL stored next to the
//!     mode. The deployment runbook is `deploy/README.md`.
//!
//! `RIGEL_API_URL` (and the older `RIGEL_RELAY_BASE_URL`) still override the saved choice for a
//! headless run or a test. An override is reported by [`status_at`] so Settings can say that the
//! saved value is not the one requests are using.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The API Rigel uses unless the user chooses otherwise.
pub const HOSTED_API_URL: &str = "https://api.askrigel.com";
pub const HOSTED: &str = "hosted";
pub const SELF_HOSTED: &str = "self_hosted";
/// The current override name. The legacy name is still read: a machine configured before this
/// setting existed keeps working without a migration step.
pub const API_URL_ENV: &str = "RIGEL_API_URL";
pub const LEGACY_API_URL_ENV: &str = "RIGEL_RELAY_BASE_URL";
const MAX_URL_BYTES: usize = 2048;

fn default_mode() -> String {
    HOSTED.to_string()
}

/// The saved backend placement. `api_url` is empty in hosted mode, so a later switch back to
/// self-hosted starts from a blank field rather than a stale URL the user forgot about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendConfig {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub api_url: String,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            api_url: String::new(),
        }
    }
}

impl BackendConfig {
    pub fn hosted() -> Self {
        Self::default()
    }

    /// A self-hosted deployment. The URL is normalized and validated here, not at request time:
    /// an invalid URL should fail on the click that saved it.
    pub fn self_hosted(api_url: impl AsRef<str>) -> Result<Self, String> {
        Ok(Self {
            mode: SELF_HOSTED.to_string(),
            api_url: normalize_api_url(api_url.as_ref())?,
        })
    }

    /// Validate and normalize the pair the Settings page sends.
    pub fn from_parts(mode: &str, api_url: &str) -> Result<Self, String> {
        match mode.trim().to_ascii_lowercase().as_str() {
            HOSTED => Ok(Self::hosted()),
            SELF_HOSTED => Self::self_hosted(api_url),
            other => Err(format!(
                "unknown backend mode `{other}`; use {HOSTED} or {SELF_HOSTED}"
            )),
        }
    }

    pub fn is_self_hosted(&self) -> bool {
        self.mode.eq_ignore_ascii_case(SELF_HOSTED)
    }

    /// The base every route is appended to. Hosted always uses the compiled-in URL, so a hand
    /// edited file cannot redirect hosted users somewhere else without also changing the mode.
    pub fn effective_url(&self) -> String {
        if self.is_self_hosted() && !self.api_url.trim().is_empty() {
            self.api_url.trim().trim_end_matches('/').to_string()
        } else {
            HOSTED_API_URL.to_string()
        }
    }
}

/// A validated `http(s)` origin plus optional path prefix, with no trailing slash, query or
/// fragment. Credentials in the URL are refused: they would end up in logs and `backend.json`.
pub fn normalize_api_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Enter the base URL of your Rigel API deployment.".to_string());
    }
    if raw.len() > MAX_URL_BYTES {
        return Err("The API URL is too long.".to_string());
    }
    let parsed =
        reqwest::Url::parse(raw).map_err(|_| "The API URL is not a valid URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("The API URL must start with http:// or https://.".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("The API URL must name a host.".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("The API URL must not contain a username or password.".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("The API URL must not contain a query or fragment.".to_string());
    }
    let mut base = parsed.origin().ascii_serialization();
    let path = parsed.path().trim_end_matches('/');
    if !path.is_empty() && path != "/" {
        base.push_str(path);
    }
    Ok(base)
}

pub fn path_at(data_dir: &Path) -> PathBuf {
    data_dir.join("backend.json")
}

/// Read the saved choice. A missing or unreadable file is hosted mode, not an error: a first run
/// has no file, and a corrupt one must not stop the app from starting.
pub fn load_at(data_dir: &Path) -> BackendConfig {
    let Ok(text) = std::fs::read_to_string(path_at(data_dir)) else {
        return BackendConfig::default();
    };
    let Ok(mut config) = serde_json::from_str::<BackendConfig>(&text) else {
        return BackendConfig::default();
    };
    config.mode = config.mode.trim().to_ascii_lowercase();
    if !matches!(config.mode.as_str(), HOSTED | SELF_HOSTED) {
        return BackendConfig::default();
    }
    if config.is_self_hosted() {
        match normalize_api_url(&config.api_url) {
            Ok(url) => config.api_url = url,
            Err(_) => return BackendConfig::default(),
        }
    } else {
        config.api_url.clear();
    }
    config
}

/// Write the choice atomically, so a crash cannot leave half a JSON document behind.
pub fn save_at(data_dir: &Path, config: &BackendConfig) -> Result<BackendConfig, String> {
    let config = BackendConfig::from_parts(&config.mode, &config.api_url)?;
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let path = path_at(data_dir);
    let temp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?;
    std::fs::write(&temp, body).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, &path).map_err(|error| error.to_string())?;
    Ok(config)
}

fn valid_override(value: &str) -> Option<String> {
    normalize_api_url(value).ok()
}

/// The environment override, if one is set and valid. The newer name wins.
pub fn env_override() -> Option<(&'static str, String)> {
    for name in [API_URL_ENV, LEGACY_API_URL_ENV] {
        if let Ok(value) = std::env::var(name)
            && let Some(url) = valid_override(&value)
        {
            return Some((name, url));
        }
    }
    None
}

/// The base URL requests use, in precedence order: environment override, saved choice, hosted.
pub fn api_base_at(data_dir: &Path) -> String {
    if let Some((_, url)) = env_override() {
        return url;
    }
    load_at(data_dir).effective_url()
}

/// The folder the app and CLI use for the index and settings. `RIGEL_INDEX_DIR` wins, matching
/// the window's `index_dir()`; without it this is the platform app-data folder.
pub fn data_dir() -> PathBuf {
    std::env::var_os("RIGEL_INDEX_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(crate::storage::default_index_dir)
}

/// [`api_base_at`] for the shared data directory. The model door has no data directory in its
/// signature, and this is the same folder the window saves the choice in.
pub fn api_base() -> String {
    api_base_at(&data_dir())
}

/// What Settings shows. Token presence is added by the command layer, because this module stays
/// free of keychain reads and remains testable without an OS credential store.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackendStatus {
    pub mode: String,
    pub api_url: String,
    pub effective_url: String,
    pub hosted_url: String,
    /// The environment variable that is overriding the saved choice, if any.
    pub override_env: Option<String>,
}

pub fn status_at(data_dir: &Path) -> BackendStatus {
    let config = load_at(data_dir);
    let override_env = env_override();
    BackendStatus {
        mode: config.mode.clone(),
        api_url: config.api_url.clone(),
        effective_url: override_env
            .as_ref()
            .map(|(_, url)| url.clone())
            .unwrap_or_else(|| config.effective_url()),
        hosted_url: HOSTED_API_URL.to_string(),
        override_env: override_env.map(|(name, _)| name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_hosted_api() {
        let config = BackendConfig::default();
        assert_eq!(config.mode, HOSTED);
        assert_eq!(config.effective_url(), HOSTED_API_URL);
        assert!(!config.is_self_hosted());
    }

    #[test]
    fn a_self_hosted_choice_round_trips_through_disk() {
        let root = tempfile::tempdir().unwrap();
        let config =
            BackendConfig::from_parts(SELF_HOSTED, " https://rigel.example.com/api/ ").unwrap();
        assert_eq!(config.api_url, "https://rigel.example.com/api");
        let saved = save_at(root.path(), &config).unwrap();
        assert_eq!(saved, config);
        assert_eq!(load_at(root.path()), config);
        assert_eq!(
            load_at(root.path()).effective_url(),
            "https://rigel.example.com/api"
        );
    }

    #[test]
    fn switching_back_to_hosted_clears_the_url() {
        let root = tempfile::tempdir().unwrap();
        save_at(
            root.path(),
            &BackendConfig::self_hosted("http://127.0.0.1:8080").unwrap(),
        )
        .unwrap();
        let hosted = BackendConfig::from_parts(HOSTED, "https://stale.example").unwrap();
        save_at(root.path(), &hosted).unwrap();
        let loaded = load_at(root.path());
        assert_eq!(loaded.mode, HOSTED);
        assert!(loaded.api_url.is_empty());
        assert_eq!(loaded.effective_url(), HOSTED_API_URL);
    }

    #[test]
    fn a_url_must_be_http_without_credentials_or_query() {
        assert_eq!(
            normalize_api_url("https://api.example.com/").unwrap(),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_api_url("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(normalize_api_url("").is_err());
        assert!(normalize_api_url("ftp://example.com").is_err());
        assert!(normalize_api_url("https://user:pass@example.com").is_err());
        assert!(normalize_api_url("https://example.com/?x=1").is_err());
        assert!(normalize_api_url("not a url").is_err());
    }

    #[test]
    fn an_invalid_saved_document_falls_back_to_hosted() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            path_at(root.path()),
            r#"{"mode":"self_hosted","api_url":"file:///etc/passwd"}"#,
        )
        .unwrap();
        assert_eq!(load_at(root.path()), BackendConfig::hosted());
    }

    #[test]
    fn a_missing_file_is_hosted_not_an_error() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(load_at(root.path()).effective_url(), HOSTED_API_URL);
    }
}
