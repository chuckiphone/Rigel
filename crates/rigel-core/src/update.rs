//! Updates: a signed manifest, and nothing else (N5, P7.7).
//!
//! The rule N5 states is narrow on purpose: *no telemetry, no phone-home; updates are a signed
//! manifest fetch you can disable.* This module is the whole of the network behaviour:
//!
//! * **On by default, and switchable.** A release build checks the latest GitHub release once at
//!   launch and on demand; nothing checks on a timer. Settings → Updates turns both the check
//!   and auto-install off in one place, and a disabled switch makes no request at all.
//! * **Signed, and checked before parsed.** An Ed25519 signature over the canonical manifest
//!   bytes (`version\nurl\nsha256`, plus the optional MSI URL and hash) means a mirror or a
//!   proxy cannot change the version, an artifact URL or an artifact hash. The public key is a
//!   bundled resource copied into the data directory (`update-pubkey.txt`), or
//!   `RIGEL_UPDATE_PUBKEY`; there is no built-in key to leak.
//! * **The artifact is checked too.** The manifest's `sha256` is compared with the downloaded
//!   bytes before anything is executed. Signature without hash would let a stale artifact be
//!   paired with a fresh signature.
//! * **Installing is a separate choice.** Auto-install defaults to `true` but is its own switch;
//!   when it is on, a verified installer is started and the window exits so the installer can
//!   replace it. A release may also publish a raw binary with its own signature; that path uses
//!   [`apply`] to wait for this process to exit, swap the executable and start the new build. The
//!   update is never run from a path the manifest did not name and the hash did not match.

use std::io::Write;
use std::path::{Path, PathBuf};

pub mod apply;
pub use apply::{ApplyPlan, HELPER_FLAG, UPDATED_ENV};

use ed25519_dalek::{Signature, VerifyingKey};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub const SETTINGS_FILE: &str = "update.json";
pub const PUBKEY_FILE: &str = "update-pubkey.txt";
/// The manifest the window checks when the switch is on. A GitHub release attaches this file,
/// so `releases/latest` always resolves to the newest published manifest.
pub const GITHUB_MANIFEST_URL: &str =
    "https://github.com/chuckiphone/Rigel/releases/latest/download/latest.json";
/// Downloaded installers live here until the app exits to run them.
pub const UPDATE_DIR: &str = "updates";
/// A release installer is tens of megabytes; this is a ceiling, not an expectation.
const MAX_ARTIFACT_BYTES: u64 = 300 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("updates are disabled; turn them on with `rigel update enable`")]
    Disabled,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("the manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("no update public key: put 64 hex characters in {path} or set RIGEL_UPDATE_PUBKEY")]
    NoKey { path: PathBuf },

    #[error("the public key is not 32 bytes of hex: {0}")]
    BadKey(String),

    #[error("the signature is not 64 bytes of hex: {0}")]
    BadSignature(String),

    #[error("the manifest signature does not verify against this key")]
    BadSignatureCheck,

    #[error("the artifact is {found}, but the signed manifest says {expected}")]
    HashMismatch { expected: String, found: String },

    #[error("this build is {current} and the manifest is {offered}; nothing to do")]
    NotNewer { current: String, offered: String },

    #[error("could not fetch the manifest: {0}")]
    Network(String),

    #[error("no published release yet at {0}")]
    NoRelease(String),

    #[error("the update artifact is larger than 300 MB")]
    TooLarge,

    #[error("the update artifact has an unsupported type: {0}")]
    UnsupportedArtifact(String),

    #[error("the binary update needs url, sha256 and signature together")]
    IncompleteBinary,

    #[error("the binary update signature does not verify against this key")]
    BadBinarySignature,

    #[error("the download ended at {found} of {expected} bytes")]
    Truncated { expected: u64, found: u64 },

    #[error("could not start the installer: {0}")]
    Install(String),

    #[error("updates can only be installed on Windows")]
    NotWindows,

    #[error("a manifest must come from https, not {0}")]
    NotHttps(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    /// The artifact to download. `https` only; a `file://` URL is for tests and local builds.
    pub url: String,
    /// Hex sha256 of the artifact.
    pub sha256: String,
    /// Hex Ed25519 signature over `version\nurl\nsha256`, plus `msi_url\nmsi_sha256` when the
    /// optional MSI asset is present.
    pub signature: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub published_at: Option<i64>,
    /// The MSI twin of `url`. The window picks it when this copy is installed under Program
    /// Files; a per-user NSIS install keeps using `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msi_sha256: Option<String>,
    /// A raw executable for the deferred self-replace path. Its own signature keeps the three- or
    /// five-line installer payload valid for older clients, which ignore these fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin_signature: Option<String>,
    /// The platform the binary is for, e.g. `windows-x86_64`. A mismatch disables the binary path
    /// for this build rather than downloading an executable it cannot run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin_target: Option<String>,
}

impl UpdateManifest {
    /// The bytes the signature covers. Fixed order, no JSON formatting, so a re-serialised
    /// manifest still verifies. The optional MSI fields extend the payload only when both are
    /// present, which keeps every three-line manifest already in the wild verifiable.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut text = format!("{}\n{}\n{}", self.version, self.url, self.sha256);
        if let (Some(url), Some(sha256)) = (&self.msi_url, &self.msi_sha256) {
            text.push('\n');
            text.push_str(url);
            text.push('\n');
            text.push_str(sha256);
        }
        text.into_bytes()
    }

    /// The bytes the optional binary signature covers. Kept apart from [`Self::signed_bytes`] so
    /// a manifest with a binary still verifies on a build that predates the field.
    pub fn binary_signed_bytes(&self) -> Option<Vec<u8>> {
        match (&self.bin_url, &self.bin_sha256) {
            (Some(url), Some(sha256)) => Some(
                format!(
                    "{url}\n{sha256}\n{}",
                    self.bin_target.as_deref().unwrap_or_default()
                )
                .into_bytes(),
            ),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// On by default: one signed check at launch, which Settings → Updates can turn off.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The manifest location. Kept next to the switch so the file is the whole configuration.
    #[serde(default)]
    pub manifest_url: Option<String>,
    /// Download and run the installer without another click once a signed update is found.
    #[serde(default = "default_true")]
    pub auto_install: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            manifest_url: None,
            auto_install: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub offered: String,
    pub available: bool,
    pub verified: bool,
    pub url: String,
    pub sha256: String,
    pub notes: Option<String>,
    pub msi_url: Option<String>,
    pub msi_sha256: Option<String>,
    pub bin_url: Option<String>,
    pub bin_sha256: Option<String>,
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SETTINGS_FILE)
}

pub fn load_settings(data_dir: &Path) -> UpdateSettings {
    let mut settings = std::fs::read_to_string(settings_path(data_dir))
        .ok()
        .and_then(|text| parse_settings(&text).ok())
        .unwrap_or_default();
    // Environment switches win over the file so a test feed or a managed install can override a
    // saved setting without editing the data directory.
    if env_flag("RIGEL_UPDATE_DISABLE") {
        settings.enabled = false;
    }
    if let Some(url) = std::env::var("RIGEL_UPDATE_MANIFEST_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        settings.manifest_url = Some(url.trim().to_string());
    }
    if env_false("RIGEL_UPDATE_AUTO_INSTALL") {
        settings.auto_install = false;
    }
    settings
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn env_false(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// The configured manifest, or the release asset GitHub publishes from the workflow.
pub fn manifest_url(settings: &UpdateSettings) -> &str {
    settings
        .manifest_url
        .as_deref()
        .unwrap_or(GITHUB_MANIFEST_URL)
}

/// The release channel this build belongs to. A semantic pre-release suffix is the only
/// distinction the updater needs today: a build without one is the stable `latest` channel.
pub fn channel(version: &str) -> &'static str {
    if version.contains('-') {
        "prerelease"
    } else {
        "stable"
    }
}

/// The last non-empty line of one of the updater's logs. Doctor reports it as the receipt of
/// the last check or apply step; an absent or unreadable log is `None`, never an invented state.
pub fn last_log_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(180).collect())
}

pub fn save_settings(data_dir: &Path, settings: &UpdateSettings) -> Result<(), UpdateError> {
    std::fs::create_dir_all(data_dir)?;
    let path = settings_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The key, from the environment first so a build machine can pin one without a file.
pub fn public_key(data_dir: &Path) -> Result<[u8; 32], UpdateError> {
    let hex = match std::env::var("RIGEL_UPDATE_PUBKEY") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            let path = data_dir.join(PUBKEY_FILE);
            if !path.exists() {
                return Err(UpdateError::NoKey { path });
            }
            std::fs::read_to_string(&path)?
        }
    };
    let bytes =
        decode_hex(hex.trim()).ok_or_else(|| UpdateError::BadKey(hex.trim().to_string()))?;
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| UpdateError::BadKey(format!("{} bytes", bytes.len())))?;
    Ok(key)
}

pub fn verify_manifest(manifest: &UpdateManifest, key: &[u8; 32]) -> Result<(), UpdateError> {
    let verifying =
        VerifyingKey::from_bytes(key).map_err(|error| UpdateError::BadKey(error.to_string()))?;
    verify_with(&verifying, &manifest.signature, &manifest.signed_bytes())?;

    match (
        manifest.bin_url.is_some(),
        manifest.bin_sha256.is_some(),
        manifest.bin_signature.as_deref(),
    ) {
        (false, false, _) => Ok(()),
        (true, true, Some(signature)) => verify_with(
            &verifying,
            signature,
            &manifest.binary_signed_bytes().unwrap_or_default(),
        )
        .map_err(|_| UpdateError::BadBinarySignature),
        _ => Err(UpdateError::IncompleteBinary),
    }
}

fn verify_with(
    verifying: &VerifyingKey,
    signature: &str,
    payload: &[u8],
) -> Result<(), UpdateError> {
    let signature =
        decode_hex(signature).ok_or_else(|| UpdateError::BadSignature(signature.to_string()))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|bytes: Vec<u8>| UpdateError::BadSignature(format!("{} bytes", bytes.len())))?;
    verifying
        .verify_strict(payload, &Signature::from_bytes(&signature))
        .map_err(|_| UpdateError::BadSignatureCheck)
}

pub fn parse_manifest(text: &str) -> Result<UpdateManifest, UpdateError> {
    Ok(serde_json::from_str(text)?)
}

/// Parse the update settings body. Pure, so the fuzz target exercises the same parser the CLI
/// and the window use.
pub fn parse_settings(text: &str) -> Result<UpdateSettings, UpdateError> {
    Ok(serde_json::from_str(text)?)
}

/// Fetch a manifest over https. Called only when someone typed the URL or turned the switch on;
/// nothing in the app does this on a timer. The loopback exception exists only for the E2E test;
/// production callers use this public function and never the test-only policy below.
pub async fn fetch_manifest(url: &str) -> Result<UpdateManifest, UpdateError> {
    fetch_manifest_with_policy(url, false).await
}

async fn fetch_manifest_with_policy(
    url: &str,
    allow_loopback: bool,
) -> Result<UpdateManifest, UpdateError> {
    let loopback = url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:");
    if !url.starts_with("https://") && !(allow_loopback && loopback) {
        return Err(UpdateError::NotHttps(url.to_string()));
    }
    fetch_manifest_with(url, allow_loopback, std::time::Duration::from_secs(10)).await
}

async fn fetch_manifest_with(
    url: &str,
    allow_loopback: bool,
    timeout: std::time::Duration,
) -> Result<UpdateManifest, UpdateError> {
    let loopback = url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:");
    if !url.starts_with("https://") && !(allow_loopback && loopback) {
        return Err(UpdateError::NotHttps(url.to_string()));
    }
    let client =
        build_update_client(timeout).map_err(|error| UpdateError::Network(error.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(UpdateError::NoRelease(url.to_string()));
    }
    if !response.status().is_success() {
        return Err(UpdateError::Network(format!("HTTP {}", response.status())));
    }
    // A manifest is small; refuse an endless one rather than buffering it.
    let bytes = response
        .bytes()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if bytes.len() > 64 * 1_024 {
        return Err(UpdateError::Network(
            "the manifest is larger than 64 KB".into(),
        ));
    }
    parse_manifest(&String::from_utf8_lossy(&bytes))
}

/// Check the manifest the switch points at, defaulting to GitHub's newest release asset.
pub async fn check_latest(data_dir: &Path, current: &str) -> Result<UpdateCheck, UpdateError> {
    // Loopback is allowed so a local signed feed can be tested; the trusted key still decides.
    check_latest_with_policy(data_dir, current, true).await
}

/// A check against an explicit manifest URL. Loopback http is allowed so a locally served feed
/// can exercise the updater end to end; every other URL still has to be https and signed.
pub async fn check_latest_at(
    data_dir: &Path,
    current: &str,
    url: &str,
) -> Result<UpdateCheck, UpdateError> {
    let settings = load_settings(data_dir);
    if !settings.enabled {
        return Err(UpdateError::Disabled);
    }
    public_key(data_dir)?;
    let manifest = fetch_manifest_with_policy(url, true).await?;
    check(data_dir, &manifest, current)
}

async fn check_latest_with_policy(
    data_dir: &Path,
    current: &str,
    allow_loopback: bool,
) -> Result<UpdateCheck, UpdateError> {
    let settings = load_settings(data_dir);
    if !settings.enabled {
        return Err(UpdateError::Disabled);
    }
    // Do not make a request a build cannot verify: a development binary without the bundled
    // key should fail here, before GitHub sees anything.
    public_key(data_dir)?;
    let url = manifest_url(&settings).to_string();
    // A loopback feed is allowed so a locally served signed manifest can be tested; the signature
    // and the trusted key still decide whether anything is installed.
    let manifest = fetch_manifest_with_policy(&url, allow_loopback).await?;
    check(data_dir, &manifest, current)
}

/// The two URLs an update can use. A per-machine install (MSI under Program Files) gets the MSI;
/// everything else, including a per-user NSIS install, gets the setup executable.
pub fn artifact_for(check: &UpdateCheck, prefer_msi: bool) -> (&str, &str) {
    if prefer_msi && let (Some(url), Some(sha256)) = (&check.msi_url, &check.msi_sha256) {
        return (url, sha256);
    }
    (&check.url, &check.sha256)
}

/// The artifact to install, or `None` when the signed manifest was not newer. This is the
/// production no-op branch shared by the launch path and the CLI; callers never construct an
/// installer command for a current build.
pub fn available_artifact(check: &UpdateCheck, prefer_msi: bool) -> Option<(&str, &str)> {
    check.available.then(|| artifact_for(check, prefer_msi))
}

/// Download one installer and verify the signed hash before returning its path. The file is
/// written under `data_dir/updates`; a failed or oversized download is removed rather than left
/// for the installer to find.
pub async fn download_artifact(
    url: &str,
    sha256: &str,
    data_dir: &Path,
) -> Result<PathBuf, UpdateError> {
    let loopback = url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:");
    if !url.starts_with("https://") && !loopback {
        return Err(UpdateError::NotHttps(url.to_string()));
    }
    let dir = data_dir.join(UPDATE_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(artifact_name(url));

    let client = build_update_client(std::time::Duration::from_secs(900))
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Network(format!("HTTP {}", response.status())));
    }
    let declared = response.content_length();
    if declared.is_some_and(|length| length > MAX_ARTIFACT_BYTES) {
        return Err(UpdateError::TooLarge);
    }

    let mut file = tokio::fs::File::create(&path).await?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                // A connection that closes before the declared length is a truncated download, not
                // a network failure the caller should retry blindly. Either way, remove the part.
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err(match declared {
                    Some(expected) if total < expected => UpdateError::Truncated {
                        expected,
                        found: total,
                    },
                    _ => UpdateError::Network(error.to_string()),
                });
            }
        };
        total += chunk.len() as u64;
        if total > MAX_ARTIFACT_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(UpdateError::TooLarge);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;

    if let Some(expected) = declared
        && total != expected
    {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(UpdateError::Truncated {
            expected,
            found: total,
        });
    }

    let found = format!("{:x}", hasher.finalize());
    if !found.eq_ignore_ascii_case(sha256) {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(UpdateError::HashMismatch {
            expected: sha256.to_string(),
            found,
        });
    }
    Ok(path)
}

fn build_update_client(timeout: std::time::Duration) -> Result<reqwest::Client, reqwest::Error> {
    build_update_client_with_settings(timeout, &crate::net::proxy::settings())
}

fn build_update_client_with_settings(
    timeout: std::time::Duration,
    settings: &crate::net::proxy::ProxySettings,
) -> Result<reqwest::Client, reqwest::Error> {
    crate::net::proxy::configure_builder_with_settings(reqwest::Client::builder(), settings)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(timeout)
        .user_agent("rigel/0.1")
        .build()
}

/// A fixed basename from the URL, with only filename-safe characters. The asset name is
/// `Rigel_<version>_x64-setup.exe`; a URL without a recognised suffix falls back to an exe.
fn artifact_name(url: &str) -> String {
    let tail = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let cleaned: String = tail
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .collect();
    if cleaned.ends_with(".exe") || cleaned.ends_with(".msi") {
        cleaned
    } else {
        "rigel-update.exe".to_string()
    }
}

/// Run the downloaded installer. NSIS is silent with `/S` and relaunches this app with `/R`.
/// MSI has no equivalent relaunch flag, so a per-machine update starts a detached helper that
/// waits for this process to exit, runs the MSI, and then starts the app again. The caller exits
/// the window after this returns, so either installer can replace the running executable.
pub fn install_artifact(path: &Path) -> Result<(), UpdateError> {
    if !path.is_file() {
        return Err(UpdateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("the downloaded installer is missing: {}", path.display()),
        )));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(UpdateError::NotWindows)
    }
    #[cfg(windows)]
    {
        let exe =
            std::env::current_exe().map_err(|error| UpdateError::Install(error.to_string()))?;
        let plan = installer_command(path, &exe)?;
        let mut command = std::process::Command::new(&plan.program);
        command.args(&plan.args);
        crate::process::hide(&mut command);
        command
            .spawn()
            .map_err(|error| UpdateError::Install(error.to_string()))?;
        Ok(())
    }
}

/// The exact process a downloaded installer would start. Kept separate from the spawn so the
/// launch test can assert the command without running an MSI or a downloaded executable.
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallerCommand {
    program: PathBuf,
    args: Vec<String>,
}

#[cfg(windows)]
fn installer_command(path: &Path, exe: &Path) -> Result<InstallerCommand, UpdateError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "msi" => Ok(InstallerCommand {
            program: std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe")),
            args: vec!["/D".into(), "/C".into(), msi_relaunch_script(path, exe)],
        }),
        "exe" => Ok(InstallerCommand {
            // Tauri's NSIS template reads `/R` in silent mode and starts the new build after the
            // install, so an automatic update does not leave the user with a closed app.
            program: path.to_path_buf(),
            args: vec!["/S".into(), "/R".into()],
        }),
        _ => Err(UpdateError::UnsupportedArtifact(path.display().to_string())),
    }
}

/// The command a detached `cmd.exe` runs for a per-machine MSI update. `ping` is the delay: a
/// no-console helper cannot use `timeout` because that command wants an interactive console.
/// `start /wait` returns `msiexec`'s exit status, so `&&` starts Rigel only after a successful
/// install. Both paths are quoted; the MSI path comes from the signed manifest.
#[cfg(windows)]
fn msi_relaunch_script(installer: &Path, exe: &Path) -> String {
    format!(
        "ping -n 3 127.0.0.1 >nul & start \"\" /wait msiexec.exe /i \"{}\" /qb /norestart && start \"\" \"{}\"",
        installer.display(),
        exe.display()
    )
}

/// Verify a downloaded artifact against either signed hash. A release has a setup executable and
/// an optional MSI; both hashes are covered by the manifest signature, so either file is accepted.
pub fn verify_artifact(path: &Path, manifest: &UpdateManifest) -> Result<String, UpdateError> {
    let bytes = std::fs::read(path)?;
    let found = format!("{:x}", Sha256::digest(&bytes));
    let matches = found.eq_ignore_ascii_case(&manifest.sha256)
        || manifest
            .msi_sha256
            .as_deref()
            .is_some_and(|sha| found.eq_ignore_ascii_case(sha))
        || manifest
            .bin_sha256
            .as_deref()
            .is_some_and(|sha| found.eq_ignore_ascii_case(sha));
    if matches {
        Ok(found)
    } else {
        Err(UpdateError::HashMismatch {
            expected: manifest.sha256.clone(),
            found,
        })
    }
}

/// Compare a manifest with this build. A manifest that verifies but is not newer is a `NoNewer`
/// answer, not an error the caller has to interpret.
pub fn check(
    data_dir: &Path,
    manifest: &UpdateManifest,
    current: &str,
) -> Result<UpdateCheck, UpdateError> {
    if !load_settings(data_dir).enabled {
        return Err(UpdateError::Disabled);
    }
    let key = public_key(data_dir)?;
    verify_manifest(manifest, &key)?;
    let available = is_newer(&manifest.version, current);
    // A binary is only usable on the platform its signed tag names. A Windows asset on a Mac is
    // not an update this build can apply, so it is not offered at all.
    let bin_matches = manifest
        .bin_target
        .as_deref()
        .is_none_or(|target| target.eq_ignore_ascii_case(&platform_tag()));
    let (bin_url, bin_sha256) = if bin_matches {
        (manifest.bin_url.clone(), manifest.bin_sha256.clone())
    } else {
        (None, None)
    };
    Ok(UpdateCheck {
        current: current.to_string(),
        offered: manifest.version.clone(),
        available,
        verified: true,
        url: manifest.url.clone(),
        sha256: manifest.sha256.clone(),
        notes: manifest.notes.clone(),
        msi_url: manifest.msi_url.clone(),
        msi_sha256: manifest.msi_sha256.clone(),
        bin_url,
        bin_sha256,
    })
}

/// The tag a release signs for this build: `<os>-<arch>` in Rust's own names.
pub fn platform_tag() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Download and stage the verified binary for the deferred-replace helper. `None` means the
/// signed manifest has no binary asset (an installed copy still uses its installer) or this build
/// is already current; neither case downloads anything.
pub async fn stage_binary(
    data_dir: &Path,
    check: &UpdateCheck,
    target: &Path,
    pid: u32,
    args: Vec<String>,
    cwd: Option<PathBuf>,
) -> Result<Option<StagedUpdate>, UpdateError> {
    if !check.available {
        return Ok(None);
    }
    let (Some(url), Some(sha256)) = (check.bin_url.as_deref(), check.bin_sha256.as_deref()) else {
        return Ok(None);
    };
    let artifact = download_artifact(url, sha256, data_dir).await?;
    let plan = apply::write_plan(
        data_dir,
        &apply::ApplyPlan {
            pid,
            source: artifact.clone(),
            target: target.to_path_buf(),
            sha256: sha256.to_string(),
            args,
            cwd,
            version: check.offered.clone(),
        },
    )?;
    Ok(Some(StagedUpdate {
        plan,
        artifact,
        version: check.offered.clone(),
    }))
}

#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub plan: PathBuf,
    pub artifact: PathBuf,
    pub version: String,
}

/// One quiet line in the data directory's own log. Nothing here reaches the window.
pub fn log_line(data_dir: &Path, message: &str) {
    let _ = std::fs::create_dir_all(data_dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("update.log"))
    {
        let _ = writeln!(file, "{message}");
    }
}

/// Semantic comparison. A tag is accepted only when its core is one to three numeric segments
/// with an optional semver pre-release; anything else (a branch name, a four-part version, an
/// empty pre-release) is never an update. Equal versions and downgrades are not updates either.
pub fn is_newer(offered: &str, current: &str) -> bool {
    match (parse_version(offered), parse_version(current)) {
        (Some(offered), Some(current)) => offered.cmp(&current) == std::cmp::Ordering::Greater,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    numbers: [u64; 3],
    pre: Option<Vec<Identifier>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

impl Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.numbers.cmp(&other.numbers) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => {
                for (a, b) in left.iter().zip(right.iter()) {
                    let ordering = match (a, b) {
                        (Identifier::Numeric(a), Identifier::Numeric(b)) => a.cmp(b),
                        (Identifier::Numeric(_), Identifier::Text(_)) => Ordering::Less,
                        (Identifier::Text(_), Identifier::Numeric(_)) => Ordering::Greater,
                        (Identifier::Text(a), Identifier::Text(b)) => a.cmp(b),
                    };
                    if ordering != Ordering::Equal {
                        return ordering;
                    }
                }
                left.len().cmp(&right.len())
            }
        }
    }
}

fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    // Build metadata is accepted and ignored, as semver says.
    let core = core.split('+').next().unwrap_or(core);
    let mut numbers = [0u64; 3];
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        numbers[index] = part.parse().ok()?;
    }
    let pre = match pre {
        None => None,
        Some(text) => {
            let text = text.split('+').next().unwrap_or("");
            if text.is_empty() {
                return None;
            }
            let mut identifiers = Vec::new();
            for identifier in text.split('.') {
                if identifier.is_empty()
                    || !identifier
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                {
                    return None;
                }
                if identifier.bytes().all(|byte| byte.is_ascii_digit()) {
                    if identifier.len() > 1 && identifier.starts_with('0') {
                        return None;
                    }
                    identifiers.push(Identifier::Numeric(identifier.parse().ok()?));
                } else {
                    identifiers.push(Identifier::Text(identifier.to_string()));
                }
            }
            Some(identifiers)
        }
    };
    Some(Version { numbers, pre })
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

/* ------------------------------------------------------------------ *
 * tests
 * ------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn signed_manifest_at(
        seed: u8,
        version: &str,
        url: &str,
        sha: &str,
    ) -> (UpdateManifest, [u8; 32]) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let mut manifest = UpdateManifest {
            version: version.to_string(),
            url: url.to_string(),
            sha256: sha.to_string(),
            signature: String::new(),
            notes: Some("a test release".to_string()),
            published_at: None,
            msi_url: None,
            msi_sha256: None,
            bin_url: None,
            bin_sha256: None,
            bin_signature: None,
            bin_target: None,
        };
        let signature = key.sign(&manifest.signed_bytes());
        manifest.signature = signature
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        (manifest, key.verifying_key().to_bytes())
    }

    fn signed_manifest(seed: u8, version: &str, sha: &str) -> (UpdateManifest, [u8; 32]) {
        signed_manifest_at(
            seed,
            version,
            &format!("https://example.test/rigel-{version}.msi"),
            sha,
        )
    }

    fn update_data_dir(key: &[u8; 32], manifest_url: &str, enabled: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PUBKEY_FILE), encode_hex(key)).unwrap();
        save_settings(
            dir.path(),
            &UpdateSettings {
                enabled,
                manifest_url: Some(manifest_url.to_string()),
                auto_install: true,
            },
        )
        .unwrap();
        dir
    }

    async fn bind_release_server() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback release server");
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    #[tokio::test]
    async fn update_and_mcp_clients_use_the_shared_explicit_proxy() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = proxy.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut targets = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = proxy.accept().await.unwrap();
                let mut bytes = [0u8; 2048];
                let count = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                targets.push(request.lines().next().unwrap_or_default().to_string());
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
            targets
        });
        let settings = crate::net::proxy::ProxySettings {
            explicit: Some(format!("http://{address}")),
            bypass: Vec::new(),
        };
        let update = build_update_client_with_settings(Duration::from_secs(3), &settings).unwrap();
        let update_response = update
            .get("http://updates.example.test/latest.json")
            .send()
            .await
            .unwrap();
        assert!(update_response.status().is_success());

        let mcp = crate::mcp::transport::http_client_with_settings(&settings).unwrap();
        let mcp_response = mcp.get("http://mcp.example.test/rpc").send().await.unwrap();
        assert!(mcp_response.status().is_success());

        let targets = server.await.unwrap();
        assert!(targets[0].contains("http://updates.example.test/latest.json"));
        assert!(targets[1].contains("http://mcp.example.test/rpc"));
    }

    /// A two-route release channel: the signed manifest and the installer named by it. `seen`
    /// records the exact request paths, which is how the no-op cases prove they never downloaded.
    fn serve_release(
        listener: TcpListener,
        manifest: String,
        installer: Vec<u8>,
        artifact_path: String,
        manifest_status: u16,
    ) -> Arc<Mutex<Vec<String>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut request = [0u8; 4096];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                recorded.lock().unwrap().push(path.clone());
                let (status, body) = if path == "/latest.json" {
                    if manifest_status == 200 {
                        ("200 OK", manifest.as_bytes().to_vec())
                    } else {
                        ("404 Not Found", Vec::new())
                    }
                } else if path == artifact_path {
                    ("200 OK", installer.clone())
                } else {
                    ("404 Not Found", Vec::new())
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        seen
    }

    #[cfg(windows)]
    fn fake_installer(dir: &Path, marker: &Path) -> PathBuf {
        let source = dir.join("fake-installer.rs");
        let exe = dir.join("Rigel_9.9.9_x64-setup.exe");
        let marker = marker
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        std::fs::write(
            &source,
            format!(
                r#"fn main() {{
    use std::io::Write;
    let args = std::env::args().skip(1).collect::<Vec<_>>().join("|");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("{marker}")
        .unwrap();
    writeln!(log, "{{args}}").unwrap();
}}
"#
            ),
        )
        .unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = std::process::Command::new(rustc)
            .args(["--edition", "2021", "-o"])
            .arg(&exe)
            .arg(&source)
            .output()
            .expect("rustc builds the fake installer");
        assert!(
            output.status.success(),
            "rustc failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        exe
    }

    #[test]
    fn a_signed_manifest_verifies_and_a_changed_field_does_not() {
        let seed_sha = "a".repeat(64);
        let (manifest, key) = signed_manifest(7, "0.2.0", &seed_sha);
        verify_manifest(&manifest, &key).expect("the signature is valid");

        let mut edited = manifest.clone();
        edited.version = "0.9.0".into();
        assert!(matches!(
            verify_manifest(&edited, &key),
            Err(UpdateError::BadSignatureCheck)
        ));

        let mut moved = manifest.clone();
        moved.url = "https://evil.test/rigel.msi".into();
        assert!(matches!(
            verify_manifest(&moved, &key),
            Err(UpdateError::BadSignatureCheck)
        ));

        let mut rehashed = manifest.clone();
        rehashed.sha256 = "b".repeat(64);
        assert!(matches!(
            verify_manifest(&rehashed, &key),
            Err(UpdateError::BadSignatureCheck)
        ));
    }

    #[test]
    fn the_optional_msi_asset_is_covered_by_the_signature() {
        let (mut manifest, key) = signed_manifest(7, "0.2.0", &"a".repeat(64));
        manifest.msi_url = Some("https://example.test/rigel-0.2.0.msi".into());
        manifest.msi_sha256 = Some("c".repeat(64));
        // Re-sign the five-line payload the release workflow emits.
        let signing = SigningKey::from_bytes(&[7; 32]);
        manifest.signature = signing
            .sign(&manifest.signed_bytes())
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        verify_manifest(&manifest, &key).expect("the MSI fields are part of the payload");

        manifest.msi_url = Some("https://evil.test/rigel.msi".into());
        assert!(matches!(
            verify_manifest(&manifest, &key),
            Err(UpdateError::BadSignatureCheck)
        ));
    }

    #[test]
    fn the_installer_choice_prefers_the_msi_only_when_this_copy_is_per_machine() {
        let check = UpdateCheck {
            current: "0.1.0".into(),
            offered: "0.2.0".into(),
            available: true,
            verified: true,
            url: "https://example.test/setup.exe".into(),
            sha256: "a".repeat(64),
            notes: None,
            msi_url: Some("https://example.test/rigel.msi".into()),
            msi_sha256: Some("b".repeat(64)),
            bin_url: None,
            bin_sha256: None,
        };
        assert_eq!(
            artifact_for(&check, true),
            ("https://example.test/rigel.msi", "b".repeat(64).as_str())
        );
        assert_eq!(
            artifact_for(&check, false),
            ("https://example.test/setup.exe", "a".repeat(64).as_str())
        );
        let no_msi = UpdateCheck {
            msi_url: None,
            msi_sha256: None,
            ..check
        };
        assert_eq!(
            artifact_for(&no_msi, true).0,
            "https://example.test/setup.exe"
        );
    }

    #[test]
    fn the_manifest_defaults_to_the_latest_github_release_asset() {
        let settings = UpdateSettings::default();
        assert_eq!(manifest_url(&settings), GITHUB_MANIFEST_URL);
        let custom = UpdateSettings {
            manifest_url: Some("https://example.test/latest.json".into()),
            ..settings
        };
        assert_eq!(manifest_url(&custom), "https://example.test/latest.json");
    }

    #[test]
    fn installer_names_are_taken_from_the_asset_and_never_from_a_path() {
        assert_eq!(
            artifact_name(
                "https://github.com/chuckiphone/Rigel/releases/download/v1/Rigel_1_x64-setup.exe?download=1"
            ),
            "Rigel_1_x64-setup.exe"
        );
        assert_eq!(
            artifact_name("https://example.test/Rigel.msi#fragment"),
            "Rigel.msi"
        );
        assert_eq!(
            artifact_name("https://example.test/download"),
            "rigel-update.exe"
        );
        assert_eq!(
            artifact_name("https://example.test/../../evil.exe"),
            "evil.exe"
        );
    }

    #[tokio::test]
    async fn a_download_is_hash_checked_and_a_bad_one_is_deleted() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a local listener");
        let address = listener.local_addr().expect("an address");
        let body = b"installer bytes".to_vec();
        let expected = format!("{:x}", Sha256::digest(&body));
        let server_body = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one request");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                server_body.len()
            );
            socket.write_all(header.as_bytes()).await.expect("header");
            socket.write_all(&server_body).await.expect("body");
        });

        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{}/rigel-setup.exe", address.port());
        let path = download_artifact(&url, &expected, dir.path())
            .await
            .expect("the matching bytes download");
        assert_eq!(std::fs::read(&path).unwrap(), body);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a second listener");
        let address = listener.local_addr().expect("an address");
        let server_body = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one request");
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                server_body.len()
            );
            socket.write_all(header.as_bytes()).await.expect("header");
            socket.write_all(&server_body).await.expect("body");
        });
        let url = format!("http://127.0.0.1:{}/rigel-setup.exe", address.port());
        let wrong = "f".repeat(64);
        assert!(matches!(
            download_artifact(&url, &wrong, dir.path()).await,
            Err(UpdateError::HashMismatch { .. })
        ));
        assert!(!path.exists(), "a mismatched download is removed");
    }

    #[test]
    fn another_key_cannot_sign_this_publishers_manifest() {
        let (manifest, _key) = signed_manifest(7, "0.2.0", &"a".repeat(64));
        let other = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert!(matches!(
            verify_manifest(&manifest, &other),
            Err(UpdateError::BadSignatureCheck)
        ));
    }

    #[test]
    fn the_artifact_hash_is_checked_against_the_signature() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("rigel.msi");
        std::fs::write(&artifact, b"the installer bytes").unwrap();
        let sha = format!("{:x}", Sha256::digest(b"the installer bytes"));
        let (manifest, _key) = signed_manifest(7, "0.2.0", &sha);
        verify_artifact(&artifact, &manifest).expect("matching bytes");

        std::fs::write(&artifact, b"the installer bytez").unwrap();
        match verify_artifact(&artifact, &manifest) {
            Err(UpdateError::HashMismatch { expected, found }) => {
                assert_eq!(expected, sha);
                assert_ne!(found, sha);
            }
            other => panic!("expected a hash mismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_msi_artifact_is_checked_against_its_own_signed_hash() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("rigel.msi");
        std::fs::write(&artifact, b"the MSI bytes").unwrap();
        let msi_sha = format!("{:x}", Sha256::digest(b"the MSI bytes"));
        let (mut manifest, _key) = signed_manifest(7, "0.2.0", &"a".repeat(64));
        manifest.msi_url = Some("https://example.test/rigel.msi".into());
        manifest.msi_sha256 = Some(msi_sha.clone());

        assert_eq!(
            verify_artifact(&artifact, &manifest).expect("matching MSI bytes"),
            msi_sha
        );

        std::fs::write(&artifact, b"a different installer").unwrap();
        assert!(matches!(
            verify_artifact(&artifact, &manifest),
            Err(UpdateError::HashMismatch { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn the_msi_relaunch_helper_waits_for_the_installer_and_starts_rigel_again() {
        let installer = Path::new(r"C:\Users\rigel\AppData\Local\Rigel\updates\Rigel 0.4.0.msi");
        let exe = Path::new(r"C:\Program Files\Rigel\rigel-app.exe");
        let script = msi_relaunch_script(installer, exe);
        assert!(
            script.contains("ping -n 3"),
            "the helper delays until the app exits"
        );
        assert!(
            script.contains("start \"\" /wait msiexec.exe /i"),
            "the MSI is awaited"
        );
        assert!(
            script.contains("Rigel 0.4.0.msi"),
            "the signed installer path is passed"
        );
        assert!(
            script.contains("/qb /norestart"),
            "the MSI stays quiet and does not reboot"
        );
        assert!(
            script.contains("&& start \"\" \"C:\\Program Files\\Rigel\\rigel-app.exe\""),
            "Rigel restarts only after a successful install"
        );

        let plan = installer_command(installer, exe).expect("an MSI plan");
        assert_eq!(
            plan.program,
            std::env::var_os("COMSPEC")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe"))
        );
        assert_eq!(plan.args.len(), 3);
        assert_eq!(plan.args[0], "/D");
        assert_eq!(plan.args[1], "/C");
        assert_eq!(plan.args[2], script);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_newer_signed_release_downloads_verifies_and_invokes_the_installer() {
        let work = tempfile::tempdir().unwrap();
        let marker = work.path().join("installer-args.txt");
        let fake = fake_installer(work.path(), &marker);
        let installer = std::fs::read(&fake).unwrap();
        let sha256 = format!("{:x}", Sha256::digest(&installer));
        let (listener, port) = bind_release_server().await;
        let artifact_name = "Rigel_9.9.9_x64-setup.exe";
        let artifact_path = format!("/{artifact_name}");
        let artifact_url = format!("http://127.0.0.1:{port}{artifact_path}");
        let (manifest, key) = signed_manifest_at(21, "9.9.9", &artifact_url, &sha256);
        let manifest_url = format!("http://127.0.0.1:{port}/latest.json");
        let seen = serve_release(
            listener,
            serde_json::to_string(&manifest).unwrap(),
            installer.clone(),
            artifact_path.clone(),
            200,
        );
        let data = update_data_dir(&key, &manifest_url, true);

        let check = check_latest_with_policy(data.path(), "0.3.0", true)
            .await
            .expect("the signed newer release is fetched and verified");
        assert!(check.available);
        assert!(check.verified);
        assert_eq!(check.offered, "9.9.9");
        let (url, sha) = available_artifact(&check, false).expect("an installable artifact");
        assert_eq!(url, artifact_url);
        assert_eq!(sha, sha256);

        let downloaded = download_artifact(url, sha, data.path())
            .await
            .expect("the artifact downloads and its SHA-256 matches");
        assert_eq!(std::fs::read(&downloaded).unwrap(), installer);
        assert_eq!(
            verify_artifact(&downloaded, &manifest).unwrap(),
            sha256,
            "the same downloaded bytes verify against the signed manifest"
        );

        let app = data.path().join("rigel-app.exe");
        let plan = installer_command(&downloaded, &app).expect("the NSIS install plan");
        assert_eq!(plan.program, downloaded);
        assert_eq!(plan.args, vec!["/S".to_string(), "/R".to_string()]);

        install_artifact(&downloaded).expect("the installer starts");
        let started = std::time::Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "the downloaded installer actually ran");
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().trim(),
            "/S|/R",
            "NSIS receives the exact silent-install and relaunch flags"
        );

        let requests = seen.lock().unwrap().clone();
        assert_eq!(
            requests,
            vec!["/latest.json".to_string(), artifact_path],
            "only the signed manifest and the one installer were fetched"
        );
    }

    #[tokio::test]
    async fn a_current_release_has_no_artifact_and_does_not_download() {
        let installer = b"not downloaded by a current build".to_vec();
        let sha256 = format!("{:x}", Sha256::digest(&installer));
        let (listener, port) = bind_release_server().await;
        let artifact_path = "/Rigel_0.3.0_x64-setup.exe".to_string();
        let artifact_url = format!("http://127.0.0.1:{port}{artifact_path}");
        let (manifest, key) = signed_manifest_at(22, "0.3.0", &artifact_url, &sha256);
        let manifest_url = format!("http://127.0.0.1:{port}/latest.json");
        let seen = serve_release(
            listener,
            serde_json::to_string(&manifest).unwrap(),
            installer,
            artifact_path,
            200,
        );
        let data = update_data_dir(&key, &manifest_url, true);

        let check = check_latest_with_policy(data.path(), "0.3.0", true)
            .await
            .expect("the current manifest verifies");
        assert!(!check.available);
        assert!(
            available_artifact(&check, true).is_none(),
            "a current build has no installer to run"
        );
        assert!(!data.path().join(UPDATE_DIR).exists());
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["/latest.json".to_string()],
            "a current build fetches the manifest but never the installer"
        );
    }

    #[tokio::test]
    async fn a_missing_release_makes_no_download_and_no_install() {
        let (listener, port) = bind_release_server().await;
        let manifest_url = format!("http://127.0.0.1:{port}/latest.json");
        let key = [23u8; 32];
        let seen = serve_release(
            listener,
            String::new(),
            Vec::new(),
            "/Rigel_9.9.9_x64-setup.exe".to_string(),
            404,
        );
        let data = update_data_dir(&key, &manifest_url, true);

        let error = check_latest_with_policy(data.path(), "0.3.0", true)
            .await
            .expect_err("a 404 is not a release");
        assert!(matches!(error, UpdateError::NoRelease(_)));
        assert!(!data.path().join(UPDATE_DIR).exists());
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec!["/latest.json".to_string()],
            "the missing release is checked but never downloaded"
        );
    }

    #[tokio::test]
    async fn a_disabled_update_switch_makes_no_request_and_no_download() {
        let (listener, port) = bind_release_server().await;
        let seen = serve_release(
            listener,
            String::new(),
            Vec::new(),
            "/Rigel_9.9.9_x64-setup.exe".to_string(),
            200,
        );
        let data = update_data_dir(
            &[24u8; 32],
            &format!("http://127.0.0.1:{port}/latest.json"),
            false,
        );

        let error = check_latest_with_policy(data.path(), "0.3.0", true)
            .await
            .expect_err("updates are disabled");
        assert!(matches!(error, UpdateError::Disabled));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            seen.lock().unwrap().is_empty(),
            "a disabled switch makes no HTTP request at all"
        );
        assert!(!data.path().join(UPDATE_DIR).exists());
    }

    #[test]
    fn versions_compare_numerically_and_pre_releases_sort_lower() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(is_newer("1.0.0", "1.0.0-rc1"));
        assert!(!is_newer("1.0.0-rc1", "1.0.0"));
        assert!(is_newer("1.0.0-rc2", "1.0.0-rc1"));
        assert!(!is_newer("1.0.0-alpha", "1.0.0-beta"));
        assert!(!is_newer("1.0.0-rc1", "1.0.0-rc1"));
        assert!(is_newer("v2", "1.9.9"));
        assert!(!is_newer("0.3.0", "0.3.0+build.7"));
    }

    #[test]
    fn a_malformed_tag_or_version_is_never_an_update() {
        for offered in [
            "",
            "latest",
            "0.4.x",
            "1.2.3.4",
            "-1.0.0",
            "0.4.0-",
            "0.4.0-01",
            "v0.4.0.1",
            "  ",
            "0.4.0+meta-",
        ] {
            assert!(
                !is_newer(offered, "0.3.0"),
                "{offered:?} must not be an update"
            );
        }
        // A malformed current version cannot be safely compared either.
        assert!(!is_newer("0.4.0", "not-a-version"));
    }

    #[test]
    fn a_binary_for_another_platform_is_not_offered() {
        let key = SigningKey::from_bytes(&[7u8; 32])
            .verifying_key()
            .to_bytes();
        let dir = update_data_dir(&key, "https://example.test/latest.json", true);
        let (mut manifest, _key) = signed_manifest(7, "0.4.0", &"a".repeat(64));
        manifest.bin_url = Some("https://example.test/rigel-app.exe".into());
        manifest.bin_sha256 = Some("b".repeat(64));
        manifest.bin_target = Some("plan9-sparc".into());
        let signing = SigningKey::from_bytes(&[7; 32]);
        manifest.bin_signature = Some(
            signing
                .sign(&manifest.binary_signed_bytes().unwrap())
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        let check = check(dir.path(), &manifest, "0.3.0").unwrap();
        assert!(check.available, "the release itself is newer");
        assert!(
            check.bin_url.is_none(),
            "a foreign binary is not staged on this platform"
        );
    }

    #[test]
    fn the_binary_asset_has_its_own_signature() {
        let (mut manifest, key) = signed_manifest(7, "0.4.0", &"a".repeat(64));
        manifest.bin_url = Some("https://example.test/rigel-app.exe".into());
        manifest.bin_sha256 = Some("b".repeat(64));
        manifest.bin_target = Some(platform_tag());
        let signing = SigningKey::from_bytes(&[7; 32]);
        manifest.bin_signature = Some(
            signing
                .sign(&manifest.binary_signed_bytes().unwrap())
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        verify_manifest(&manifest, &key).expect("both signatures verify");

        let mut moved = manifest.clone();
        moved.bin_url = Some("https://evil.test/rigel-app.exe".into());
        assert!(matches!(
            verify_manifest(&moved, &key),
            Err(UpdateError::BadBinarySignature)
        ));

        let mut incomplete = manifest.clone();
        incomplete.bin_signature = None;
        assert!(matches!(
            verify_manifest(&incomplete, &key),
            Err(UpdateError::IncompleteBinary)
        ));
    }

    #[tokio::test]
    async fn a_slow_feed_times_out_instead_of_holding_startup() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                tokio::time::sleep(Duration::from_secs(5)).await;
                drop(socket);
            }
        });
        let url = format!("http://127.0.0.1:{port}/latest.json");
        let started = std::time::Instant::now();
        let error = fetch_manifest_with(&url, true, Duration::from_millis(200))
            .await
            .expect_err("a slow feed is a no-update case");
        assert!(matches!(error, UpdateError::Network(_)));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the check gives up promptly"
        );
    }

    #[tokio::test]
    async fn a_missing_asset_leaves_no_downloaded_file() {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{port}/missing.exe");
        let error = download_artifact(&url, &"a".repeat(64), dir.path())
            .await
            .expect_err("a missing asset is not an update");
        assert!(matches!(error, UpdateError::Network(_)));
        assert!(
            !dir.path().join(UPDATE_DIR).exists()
                || dir
                    .path()
                    .join(UPDATE_DIR)
                    .read_dir()
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true),
            "nothing is left beside the data dir"
        );
    }

    #[tokio::test]
    async fn a_truncated_download_is_refused_and_removed() {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut request = [0u8; 1024];
                let _ = socket.read(&mut request).await;
                // Promise a full artifact, then close after a short body.
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\nshort",
                    )
                    .await;
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let url = format!("http://127.0.0.1:{port}/truncated.exe");
        let error = download_artifact(&url, &"a".repeat(64), dir.path())
            .await
            .expect_err("a truncated body is not an update");
        assert!(
            matches!(
                error,
                UpdateError::Truncated {
                    expected: 4096,
                    found: 5
                }
            ),
            "the short body is named: {error:?}"
        );
        assert!(
            !dir.path().join(UPDATE_DIR).join("truncated.exe").exists(),
            "the partial file is removed"
        );
    }

    #[test]
    fn the_switch_defaults_on_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let defaults = load_settings(dir.path());
        assert!(defaults.enabled, "a release checks at launch by default");
        assert!(defaults.auto_install, "auto-install is the default");

        save_settings(
            dir.path(),
            &UpdateSettings {
                enabled: true,
                manifest_url: Some("https://example.test/latest.json".into()),
                auto_install: false,
            },
        )
        .unwrap();
        let saved = load_settings(dir.path());
        assert!(saved.enabled);
        assert!(!saved.auto_install);
        assert_eq!(
            saved.manifest_url.as_deref(),
            Some("https://example.test/latest.json")
        );
    }

    #[tokio::test]
    async fn a_build_without_a_key_does_not_fetch_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        save_settings(
            dir.path(),
            &UpdateSettings {
                enabled: true,
                manifest_url: Some("https://127.0.0.1:1/latest.json".into()),
                auto_install: false,
            },
        )
        .unwrap();
        assert!(matches!(
            check_latest(dir.path(), "0.1.0").await,
            Err(UpdateError::NoKey { .. })
        ));
    }

    #[test]
    fn a_disabled_check_makes_no_request_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let (manifest, key) = signed_manifest(7, "0.2.0", &"a".repeat(64));
        std::fs::write(dir.path().join(PUBKEY_FILE), encode_hex(&key)).unwrap();
        save_settings(
            dir.path(),
            &UpdateSettings {
                enabled: false,
                manifest_url: None,
                auto_install: false,
            },
        )
        .unwrap();
        assert!(matches!(
            check(dir.path(), &manifest, "0.1.0"),
            Err(UpdateError::Disabled)
        ));

        save_settings(
            dir.path(),
            &UpdateSettings {
                enabled: true,
                manifest_url: None,
                auto_install: false,
            },
        )
        .unwrap();
        let result = check(dir.path(), &manifest, "0.1.0").unwrap();
        assert!(result.verified && result.available);
    }

    #[test]
    fn a_prerelease_suffix_is_the_prerelease_channel() {
        assert_eq!(channel("0.4.0-1"), "prerelease");
        assert_eq!(channel("0.4.0"), "stable");
        assert_eq!(channel("not a version"), "stable");
    }

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
