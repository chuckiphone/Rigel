//! The local llama.cpp server, started and stopped by Rigel itself.
//!
//! [`super::local`] owns the *door*: a keyless OpenAI-compatible row on the loopback interface.
//! This module owns the *process* behind one of them. The difference matters because a GGUF this
//! size (20 GiB for the benchmarked Qwen3.6-35B-A3B) is not something the app can assume the user
//! launched before opening it, and a failed launch must read as a sentence in the window rather
//! than a hung turn.
//!
//! Four rules are taken from the measured profile, not guessed:
//!
//! * **`--n-cpu-moe 36` of 40 layers keeps the experts on the CPU** and the working set at
//!   4.4–4.7 GB of a 6 GB card. Changing it without re-measuring trades a working turn for an
//!   out-of-memory window.
//! * **`-lm mmap+mlock` is worth roughly double the throughput** (21–26 tok/s against 11–17), but
//!   it only works when the whole model fits in free RAM. When it does not, the server still
//!   starts — with the slower mode made visible in [`Status::mode`], never silently.
//! * **One instance only.** A second server on the same port halves usable RAM and exhausts the
//!   card, so [`start`] attaches to a healthy server instead of spawning another.
//! * **Stop by recorded PID**, never by image name. The PID is checked against the image before it
//!   is killed, so a stale record cannot take an unrelated process with it.
//!
//! Paths are configuration, not constants: `RIGEL_LLAMACPP_SERVER` and `RIGEL_LLAMACPP_MODEL` win,
//! then the saved profile, then a bounded search of the local model harness on this machine. A
//! missing path names the variable and the command that fixes it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The loopback address the server binds. The handoff's verified command uses this host.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// The port the handoff's verified command uses.
pub const DEFAULT_PORT: u16 = 8080;
/// The binary, when it is not beside the model.
pub const ENV_SERVER: &str = "RIGEL_LLAMACPP_SERVER";
/// The GGUF to serve.
pub const ENV_MODEL: &str = "RIGEL_LLAMACPP_MODEL";
/// The door's base URL override, shared with every other local door.
pub const ENV_BASE_URL: &str = "RIGEL_LLAMACPP_BASE_URL";
/// `1` keeps the model's reasoning scratchpad on. Off by default: a thinking model spends the
/// output budget before writing `content`, and a 24-token title request would come back empty.
pub const ENV_THINKING: &str = "RIGEL_LLAMACPP_THINKING";

/// How long a start waits for `/health` before reporting "still loading". A cold load of this
/// model takes 20–40 s on the reference machine; 120 s leaves room for a busy disk.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// What mlock needs beyond the weights: the context, the compute buffers and the process itself.
const PIN_HEADROOM: u64 = 512 * 1024 * 1024;

fn host_default() -> String {
    DEFAULT_HOST.to_string()
}

fn port_default() -> u16 {
    DEFAULT_PORT
}

fn ctx_default() -> u32 {
    4_096
}

fn threads_default() -> u32 {
    8
}

fn moe_default() -> u32 {
    36
}

/// Everything the server needs to start, in one record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Absolute path to `llama-server` (`llama-server.exe` on Windows).
    pub server: String,
    /// Absolute path to the GGUF.
    pub model: String,
    #[serde(default = "host_default")]
    pub host: String,
    #[serde(default = "port_default")]
    pub port: u16,
    #[serde(default = "ctx_default")]
    pub ctx: u32,
    /// One thread per physical core; more was measured slower.
    #[serde(default = "threads_default")]
    pub threads: u32,
    /// Layers whose MoE experts stay on the CPU.
    #[serde(default = "moe_default")]
    pub moe_on_cpu: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            server: String::new(),
            model: String::new(),
            host: host_default(),
            port: DEFAULT_PORT,
            ctx: ctx_default(),
            threads: threads_default(),
            moe_on_cpu: moe_default(),
        }
    }
}

/// Whether the weights are pinned in RAM, or mapped and re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `-lm mmap+mlock`: the whole model is resident, which is the measured fast path.
    Pinned,
    /// The model does not fit in free RAM, so mlock is omitted and throughput roughly halves.
    Mmap,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Mmap => "mmap",
        }
    }
}

impl Profile {
    /// The door's base URL, without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}:{}/v1", self.host, self.port)
    }

    /// The launch arguments, in the order the measured command used.
    pub fn args(&self, mode: Mode) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-m".into(),
            self.model.clone(),
            "-ngl".into(),
            "99".into(),
            "--n-cpu-moe".into(),
            self.moe_on_cpu.to_string(),
            "-c".into(),
            self.ctx.to_string(),
            "-fa".into(),
            "on".into(),
            "-t".into(),
            self.threads.to_string(),
        ];
        if mode == Mode::Pinned {
            args.push("-lm".into());
            args.push("mmap+mlock".into());
        }
        args.extend([
            "--host".into(),
            self.host.clone(),
            "--port".into(),
            self.port.to_string(),
        ]);
        args
    }
}

/// What the window and `rigel local status` show. One shape for both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// `ready`, `loading` or `down`.
    pub state: String,
    /// One sentence naming what happened and what to do about it.
    pub detail: String,
    pub base_url: String,
    pub server: Option<String>,
    pub model: Option<String>,
    pub mode: String,
    pub port: u16,
    /// The process Rigel started, when the record still describes this server.
    pub pid: Option<u32>,
    /// Both paths exist and the server can be launched.
    pub configured: bool,
    /// Free physical RAM at the time of the report, in GiB to one decimal.
    pub free_ram_gb: Option<f64>,
    /// The GGUF's size, in GiB to one decimal.
    pub model_gb: Option<f64>,
}

impl Status {
    pub fn ready(&self) -> bool {
        self.state == "ready"
    }
}

/// The environment override for the door, when one is set. This is the value the provider table
/// sends turns to, and the first place [`status`] and [`start`] look.
pub fn base_url() -> String {
    if let Ok(value) = std::env::var(ENV_BASE_URL) {
        let value = value.trim().trim_end_matches('/');
        if !value.is_empty() {
            return value.to_string();
        }
    }
    let profile = load_profile().unwrap_or_default();
    Profile {
        port: profile.port,
        ..Profile::default()
    }
    .base_url()
}

/// Whether the model should keep its reasoning scratchpad. Off unless `RIGEL_LLAMACPP_THINKING`
/// says otherwise: the measured build returns empty `content` when the budget runs out mid-think.
pub fn thinking_enabled() -> bool {
    std::env::var(ENV_THINKING)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// The file the saved profile lives in, beside the index.
pub fn profile_path_at(dir: &Path) -> PathBuf {
    dir.join("llama-server.json")
}

pub fn profile_path() -> PathBuf {
    profile_path_at(&crate::backend::data_dir())
}

fn pid_path_at(dir: &Path) -> PathBuf {
    dir.join("llama-server.pid")
}

fn log_path_at(dir: &Path) -> PathBuf {
    dir.join("llama-server.log")
}

/// The saved profile, or the defaults when nothing has been saved. An unreadable file is reported
/// rather than silently replaced: the user's path is worth more than a clean start.
pub fn load_profile_at(dir: &Path) -> Result<Profile, String> {
    let path = profile_path_at(dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("`{}` is not a server profile: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Profile::default()),
        Err(error) => Err(format!("could not read `{}`: {error}", path.display())),
    }
}

pub fn load_profile() -> Result<Profile, String> {
    load_profile_at(&crate::backend::data_dir())
}

/// Write the profile, after checking that both paths exist. A profile that names a missing file
/// would fail on the next launch, far from the command that could have said so.
pub fn save_profile_at(dir: &Path, profile: &Profile) -> Result<(), String> {
    // A saved relative path would mean something different from every working directory the
    // server is later started in. Both paths are made absolute here, once.
    let mut profile = profile.clone();
    profile.server = absolute(&profile.server);
    profile.model = absolute(&profile.model);
    let profile = &profile;
    validate(profile)?;
    let path = profile_path_at(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create `{}`: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(profile)
        .map_err(|error| format!("could not encode the profile: {error}"))?;
    std::fs::write(&path, text)
        .map_err(|error| format!("could not write `{}`: {error}", path.display()))
}

pub fn save_profile(profile: &Profile) -> Result<(), String> {
    save_profile_at(&crate::backend::data_dir(), profile)
}

/// An absolute form of a path, without resolving it through links: the file must not have to
/// exist for this to be meaningful, and a UNC prefix is noise in the status line.
fn absolute(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    std::path::absolute(value)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| value.to_string())
}

fn validate(profile: &Profile) -> Result<(), String> {
    if profile.server.trim().is_empty() {
        return Err(format!(
            "no llama-server binary; set {ENV_SERVER} or run `rigel local setup --server <path>`"
        ));
    }
    if profile.model.trim().is_empty() {
        return Err(format!(
            "no GGUF to serve; set {ENV_MODEL} or run `rigel local setup --model <path>`"
        ));
    }
    let server = Path::new(&profile.server);
    if !server.is_file() {
        return Err(format!(
            "`{}` is not a file; set {ENV_SERVER} to the llama-server binary",
            profile.server
        ));
    }
    let model = Path::new(&profile.model);
    if !model.is_file() {
        return Err(format!(
            "`{}` is not a file; set {ENV_MODEL} to the GGUF",
            profile.model
        ));
    }
    if profile.port == 0 {
        return Err("the server port must be between 1 and 65535".to_string());
    }
    Ok(())
}

/// The profile a launch would use: the saved one, with the environment on top and the local
/// harness filling in anything neither names.
pub fn resolve_at(dir: &Path) -> Result<Profile, String> {
    let mut profile = load_profile_at(dir)?;
    if let Some(value) = non_empty(ENV_SERVER) {
        profile.server = value;
    }
    if let Some(value) = non_empty(ENV_MODEL) {
        profile.model = value;
    }
    if profile.server.trim().is_empty()
        && let Some(found) = discover_server()
    {
        profile.server = found.display().to_string();
    }
    if profile.model.trim().is_empty()
        && let Some(found) = discover_model()
    {
        profile.model = found.display().to_string();
    }
    validate(&profile)?;
    Ok(profile)
}

pub fn resolve() -> Result<Profile, String> {
    resolve_at(&crate::backend::data_dir())
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The local model harness's root, when this machine has one. This is where a locally served GGUF
/// already lives; the search below is bounded to two levels and never walks a home directory.
fn harness_root() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let root = PathBuf::from(home)
        .join(".config")
        .join("freebuff-desktop")
        .join("local-projects");
    root.is_dir().then_some(root)
}

fn server_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// `llama-server` on `PATH`, then in each project's `tools/llamacpp` folder.
pub fn discover_server() -> Option<PathBuf> {
    if let Some(found) = on_path(server_name()) {
        return Some(found);
    }
    let root = harness_root()?;
    project_dirs(&root).find_map(|project| {
        let candidate = project.join("tools").join("llamacpp").join(server_name());
        candidate.is_file().then_some(candidate)
    })
}

/// The largest GGUF under each project's `models/gguf`. Largest, not first: a folder can hold
/// several quants and the one that was benchmarked is the full-size file.
pub fn discover_model() -> Option<PathBuf> {
    let root = harness_root()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for project in project_dirs(&root) {
        let dir = project.join("models").join("gguf");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("gguf") {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(known, _)| size > *known) {
                best = Some((size, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn project_dirs(root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs.into_iter()
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

/// Whether the weights fit in free RAM with the headroom mlock needs.
pub fn mode_for(free_bytes: u64, model_bytes: u64) -> Mode {
    if free_bytes >= model_bytes.saturating_add(PIN_HEADROOM) {
        Mode::Pinned
    } else {
        Mode::Mmap
    }
}

/// Free physical RAM in bytes, as the launch decision sees it.
pub fn free_ram_bytes() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.available_memory()
}

fn gib(bytes: u64) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0 * 1024.0) * 10.0).round() / 10.0
}

fn model_bytes(path: &str) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Ready,
    Loading,
    Down,
}

async fn health_at(host: &str, port: u16) -> Health {
    let client = match crate::net::proxy::configure_builder(reqwest::Client::builder())
        .timeout(Duration::from_secs(2))
        .connect_timeout(Duration::from_secs(1))
        .build()
    {
        Ok(client) => client,
        Err(_) => return Health::Down,
    };
    let url = format!("http://{host}:{port}/health");
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            match body.get("status").and_then(serde_json::Value::as_str) {
                Some("ok") | None => Health::Ready,
                Some(_) => Health::Loading,
            }
        }
        // A server that is still loading answers 503 while it maps the weights.
        Ok(response) if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
            Health::Loading
        }
        Ok(_) => Health::Loading,
        Err(_) => Health::Down,
    }
}

async fn port_open(host: &str, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(400),
            tokio::net::TcpStream::connect((host, port))
        )
        .await,
        Ok(Ok(_))
    )
}

fn recorded_pid_at(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(pid_path_at(dir))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
}

fn forget_pid_at(dir: &Path) {
    let _ = std::fs::remove_file(pid_path_at(dir));
}

/// The process behind a PID is the server Rigel expects, so a stale record cannot be acted on.
fn is_server_image(pid: u32) -> bool {
    image_name(pid)
        .map(|name| name.to_ascii_lowercase().contains("llama-server"))
        .unwrap_or(false)
}

/// The shortest tail of the server log, for an error that has to fit in a sentence.
fn log_tail_at(dir: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(log_path_at(dir)) else {
        return String::new();
    };
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
}

#[cfg(windows)]
fn image_name(pid: u32) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    parse_tasklist_image(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
fn parse_tasklist_image(csv: &str) -> Option<String> {
    let line = csv.lines().find(|line| line.starts_with('"'))?;
    let name = line.split('"').nth(1)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(windows))]
fn image_name(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Read the server's state without starting anything.
pub async fn status_at(dir: &Path) -> Status {
    // The same resolution a start would use, so the paths shown are the ones that would run.
    // When they are unusable, the raw saved profile is still worth showing.
    let profile = resolve_at(dir)
        .or_else(|_| load_profile_at(dir))
        .unwrap_or_default();
    let host = profile.host.clone();
    let port = profile.port;
    let health = health_at(&host, port).await;
    let free = free_ram_bytes();
    let model_size = if profile.model.is_empty() {
        0
    } else {
        model_bytes(&profile.model)
    };
    let mode = mode_for(free, model_size);
    let configured = validate(&profile).is_ok();
    let pid = recorded_pid_at(dir).filter(|pid| is_server_image(*pid));
    let (state, detail) = match health {
        Health::Ready => (
            "ready",
            format!("llama-server is answering on port {port}."),
        ),
        Health::Loading => (
            "loading",
            format!(
                "llama-server is loading the weights; the first turn waits for /health on \
                 port {port}."
            ),
        ),
        _ if !configured => (
            "down",
            format!(
                "no local server is configured. Set {ENV_SERVER} and {ENV_MODEL}, or run \
                 `rigel local setup`."
            ),
        ),
        _ => (
            "down",
            format!(
                "nothing is answering on port {port}. Start it with `rigel local start` or from \
                 Settings → Providers."
            ),
        ),
    };
    Status {
        state: state.to_string(),
        detail,
        base_url: base_url(),
        server: (!profile.server.is_empty()).then_some(profile.server),
        model: (!profile.model.is_empty()).then_some(profile.model),
        mode: mode.label().to_string(),
        port,
        pid,
        configured,
        free_ram_gb: Some(gib(free)),
        model_gb: (model_size > 0).then(|| gib(model_size)),
    }
}

pub async fn status() -> Status {
    status_at(&crate::backend::data_dir()).await
}

/// Start the server, or attach to the healthy one already there. `detail` always says which.
pub async fn start_at(dir: &Path) -> Result<Status, String> {
    let profile = resolve_at(dir)?;
    match health_at(&profile.host, profile.port).await {
        Health::Ready => {
            let mut status = status_at(dir).await;
            status.detail = format!(
                "a llama-server is already answering on port {}; attached instead of starting a \
                 second one.",
                profile.port
            );
            return Ok(status);
        }
        Health::Loading => {
            let mut status = status_at(dir).await;
            status.detail = format!(
                "a llama-server is already loading on port {}; the turn waits for it.",
                profile.port
            );
            return Ok(status);
        }
        Health::Down => {}
    }
    if port_open(&profile.host, profile.port).await {
        return Err(format!(
            "port {} is held by a process that does not answer `/health`; stop it first — Rigel \
             will not start a second server on the same port.",
            profile.port
        ));
    }

    let model_size = model_bytes(&profile.model);
    let free = free_ram_bytes();
    let mode = mode_for(free, model_size);
    if mode == Mode::Mmap {
        // Not a refusal: the server runs, at half the measured throughput, and the reason is
        // carried into the status and the log rather than left as a mystery.
        eprintln!(
            "rigel: {} GiB free is below the {} GiB model plus headroom; starting with mmap \
             instead of mlock (roughly half the tokens per second).",
            gib(free),
            gib(model_size)
        );
    }

    let mut command = std::process::Command::new(&profile.server);
    command
        .args(profile.args(mode))
        .current_dir(
            Path::new(&profile.server)
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    if let Some(parent) = log_path_at(dir).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path_at(dir))
        .map_err(|error| format!("could not open `{}`: {error}", log_path_at(dir).display()))?;
    command.stderr(std::process::Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start `{}`: {error}", profile.server))?;
    let pid = child.id();
    let _ = std::fs::write(pid_path_at(dir), pid.to_string());

    let deadline = tokio::time::Instant::now() + LOAD_TIMEOUT;
    loop {
        if matches!(health_at(&profile.host, profile.port).await, Health::Ready) {
            let mut status = status_at(dir).await;
            status.pid = Some(pid);
            status.mode = mode.label().to_string();
            status.detail = format!(
                "llama-server is answering on port {} in {} mode (PID {pid}).",
                profile.port,
                mode.label()
            );
            return Ok(status);
        }
        if let Ok(Some(exit)) = child.try_wait() {
            forget_pid_at(dir);
            let tail = log_tail_at(dir);
            return Err(format!(
                "llama-server exited with {exit} before it answered{}",
                if tail.is_empty() {
                    format!("; see `{}`", log_path_at(dir).display())
                } else {
                    format!(": {tail}")
                }
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            // Left running on purpose: it is still loading, and killing it would throw away the
            // load the next status call can report as ready.
            return Err(format!(
                "llama-server is still loading after {} s; it was left running (PID {pid}). \
                 Check `rigel local status`, or read `{}`.",
                LOAD_TIMEOUT.as_secs(),
                log_path_at(dir).display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub async fn start() -> Result<Status, String> {
    start_at(&crate::backend::data_dir()).await
}

/// Stop the server Rigel started. A PID that is not the server is refused, and the record is
/// dropped rather than killing whatever now holds the number.
pub fn stop_at(dir: &Path) -> Result<Option<u32>, String> {
    let Some(pid) = recorded_pid_at(dir) else {
        return Ok(None);
    };
    if !is_server_image(pid) {
        forget_pid_at(dir);
        return Err(format!(
            "PID {pid} is no longer llama-server; the record was stale and nothing was stopped."
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("could not run taskkill: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "taskkill refused PID {pid}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .map_err(|error| format!("could not run kill: {error}"))?;
        if !output.status.success() {
            return Err(format!("kill refused PID {pid}"));
        }
    }
    forget_pid_at(dir);
    Ok(Some(pid))
}

pub fn stop() -> Result<Option<u32>, String> {
    stop_at(&crate::backend::data_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn profile() -> Profile {
        Profile {
            server: "C:\\llama\\llama-server.exe".to_string(),
            model: "C:\\models\\qwen.gguf".to_string(),
            ..Profile::default()
        }
    }

    #[test]
    fn the_launch_line_is_the_measured_one() {
        assert_eq!(
            profile().args(Mode::Pinned),
            vec![
                "-m",
                "C:\\models\\qwen.gguf",
                "-ngl",
                "99",
                "--n-cpu-moe",
                "36",
                "-c",
                "4096",
                "-fa",
                "on",
                "-t",
                "8",
                "-lm",
                "mmap+mlock",
                "--host",
                "127.0.0.1",
                "--port",
                "8080",
            ]
        );
    }

    #[test]
    fn a_model_that_does_not_fit_drops_mlock_and_says_so() {
        // 20 GiB model on a 16 GiB box: mmap, and it is the status that says which mode ran.
        let mode = mode_for(16 * 1024 * 1024 * 1024, 20 * 1024 * 1024 * 1024);
        assert_eq!(mode, Mode::Mmap);
        assert!(!profile().args(mode).iter().any(|arg| arg == "-lm"));
        // Exactly the model plus headroom is enough; nothing below it is.
        let model = 20 * 1024 * 1024 * 1024;
        assert_eq!(mode_for(model + PIN_HEADROOM, model), Mode::Pinned);
        assert_eq!(mode_for(model + PIN_HEADROOM - 1, model), Mode::Mmap);
    }

    #[test]
    fn the_base_url_is_loopback_and_movable() {
        assert_eq!(profile().base_url(), "http://127.0.0.1:8080/v1");
        let moved = Profile {
            port: 8091,
            ..Profile::default()
        };
        assert_eq!(moved.base_url(), "http://127.0.0.1:8091/v1");
        assert!(super::super::local::is_loopback(&moved.base_url()));
    }

    #[test]
    fn a_missing_path_names_the_variable_that_fixes_it() {
        let error = validate(&Profile::default()).unwrap_err();
        assert!(error.contains(ENV_SERVER), "{error}");
        let missing = Profile {
            server: "C:\\nope\\llama-server.exe".to_string(),
            model: "C:\\nope\\qwen.gguf".to_string(),
            ..Profile::default()
        };
        let error = validate(&missing).unwrap_err();
        assert!(error.contains("llama-server.exe"), "{error}");
        assert!(error.contains(ENV_SERVER), "{error}");
    }

    #[test]
    fn a_tasklist_row_names_the_image_and_an_empty_answer_does_not() {
        assert_eq!(
            parse_tasklist_image("\"llama-server.exe\",\"4124\",\"Console\",\"1\",\"4,120 K\"\r\n"),
            Some("llama-server.exe".to_string())
        );
        assert_eq!(
            parse_tasklist_image("INFO: No tasks are running.\r\n"),
            None
        );
    }

    #[test]
    fn thinking_is_off_unless_the_environment_asks_for_it() {
        // The test binary shares one environment, so this asserts the default rather than
        // mutating a process-wide value another test may read.
        if std::env::var(ENV_THINKING).is_err() {
            assert!(!thinking_enabled());
        }
    }

    #[tokio::test]
    async fn health_separates_ready_loading_and_down() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let body = r#"{"status":"ok"}"#;
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            }
        });
        assert_eq!(health_at("127.0.0.1", address.port()).await, Health::Ready);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 2048];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\
                          Connection: close\r\n\r\n",
                    )
                    .await;
            }
        });
        assert_eq!(
            health_at("127.0.0.1", address.port()).await,
            Health::Loading
        );

        // A closed port is down, not loading: the distinction is what a start acts on.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(health_at("127.0.0.1", closed).await, Health::Down);
    }

    #[test]
    fn the_discovery_search_reads_the_harness_layout() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("7d581d08-2bb1-47aa-a071-b662a24821ff");
        std::fs::create_dir_all(project.join("tools").join("llamacpp")).unwrap();
        std::fs::create_dir_all(project.join("models").join("gguf")).unwrap();
        let server = project.join("tools").join("llamacpp").join(server_name());
        std::fs::write(&server, b"stub").unwrap();
        std::fs::write(project.join("models").join("gguf").join("small.gguf"), b"a").unwrap();
        std::fs::write(project.join("models").join("gguf").join("big.gguf"), b"aaa").unwrap();

        let projects: Vec<PathBuf> = project_dirs(root.path()).collect();
        assert_eq!(projects.len(), 1);
        // The same two lookups discover_server/discover_model make, rooted at the fixture.
        let found = &projects[0]
            .join("tools")
            .join("llamacpp")
            .join(server_name());
        assert_eq!(found, &server);
        assert!(found.is_file());
    }
}
