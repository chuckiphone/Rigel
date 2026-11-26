//! The Tauri shell: a thin IPC layer over `rigel-core`.
//!
//! Commands are deliberately stateless — each one opens the project database it needs. That
//! costs a connection open, and buys the property that no renderer crash can desynchronise
//! app state (handoff §3.2, S2).
//!
//! A native turn is the one long-lived thing: `turn_send` creates the turn row, returns it,
//! and spawns the coordinator. Its events go to the window on `rigel://events`, and its
//! cancellation token lives in managed state so `turn_cancel` can reach it.
//!
//! Two constraints shape the command signatures:
//! * commands live under `commands/`, one module per subsystem, because a child module cannot
//!   call a private function in its parent; `generate_handler!` therefore registers the
//!   `commands::<subsystem>::<name>` paths;
//! * most are synchronous and drive the async core with `block_on`, because Tauri's async
//!   command wrapper cannot express sqlx's higher-ranked `Executor` lifetimes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use rigel_core::agent::approval::{
    ApprovalBus, MailApprover, MailDecision, MailRequest, PendingApproval,
};
pub use rigel_core::agent::catalog::ModelOption;
use rigel_core::agent::routing::RouteExplanation;
use rigel_core::agent::{
    Cancels, Coordinator, DEFAULT_MAX_STEPS, ImagePart, ModelError, fixed_context_chars,
};
use rigel_core::agent::{
    capabilities, catalog, custom, health, llamaserver, openrouter, providers, vertex,
};
use rigel_core::{
    Agent, AgentEvent, AppliedReview, ApprovalScope, Attention, AttributedHunk, Bookmark,
    BudgetService, ClaimConflict, ClaimService, CommandApprover, CommandDecision, CommandRequest,
    ConflictWatcher, EventSink, HunkComment, HunkComments, InterruptSink, MemoryApprover,
    MemoryDecision, MemoryFile, MemoryRequest, MemoryService, MergeReview, MergeReviews, Model,
    NetworkPolicy, POLL_INTERVAL, Patch, PatchPipeline, PlanApprover, PlanDecision, PlanRequest,
    Provenance, RankedFact, RecoveryOffer, ResumeOutcome, Scopes, ScriptedModel, Session,
    SessionService, SessionTemplate, SessionTemplates, SkillDetail, SkillIndex, SkillsError,
    SnapshotService, StopAt, Store, SubagentPolicy, SyncPolicy, TeamAgent, Tools, Turn, TurnQuote,
    TurnTranscript, UnverifiedChange, Usage, WorktreeService, WriteMode, apply_review, credentials,
    diagnostics, skills::search::SearchHit,
};
use serde::{Deserialize, Serialize};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State};

mod claude;
mod commands;
mod concord;
mod control;
mod mcp_catalog;
mod relay;
mod terminal;
mod tray_status;
pub mod webview;
pub use claude::mcp_mode;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    pub root: String,
    pub name: String,
    pub db: String,
}

fn default_model() -> String {
    rigel_core::agent::omniroute::DEFAULT_MODEL.to_string()
}

/// Whether a saved model id still exists on the door that would receive it.
///
/// The user-visible failure this prevents is a 400 like "model X is not available in the active
/// live catalog": the id was saved when the roster listed it, and the roster moved. When the
/// provider publishes a live roster the id is checked against it; a miss re-reads that door once
/// (a cache goes stale the same way a saved id does) and then refuses with real rows instead of
/// sending a request that cannot succeed.
fn guard_live_model(provider: &str, model: &str) -> Result<(), String> {
    let model = model.trim();
    let Some(spec) = providers::find(provider) else {
        return Ok(());
    };
    // A gateway routes any id by definition, and a door with no roster has nothing to check.
    if spec.gateway || !spec.lists_models || model.is_empty() {
        return Ok(());
    }
    for force in [false, true] {
        let Ok(rows) = models_cached(force) else {
            return Ok(());
        };
        let listed: Vec<&str> = rows
            .iter()
            .filter(|row| row.provider == spec.id)
            .map(|row| row.id.as_str())
            .collect();
        if listed.is_empty() || listed.contains(&model) {
            return Ok(());
        }
        if !force {
            continue;
        }
        let sample = listed
            .iter()
            .take(4)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "`{model}` is not in {}'s live catalogue, so no request was sent; it lists {sample} — \
             pick a row in the picker, or press free mode",
            spec.name
        ));
    }
    Ok(())
}

/// Free mode reaches the native fan-out too: when the composer is on the free model Free mode
/// proved, children run on that same door and the coordinator allows the id, so a swarm does not
/// quietly spend on a paid nano default behind a free parent.
fn subagent_policy(settings: &Settings) -> SubagentPolicy {
    // Roadmap 44: the window allows one nested generation (root -> child -> leaf). The shared
    // tree budget still comes from the root turn, so depth cannot buy a new allowance.
    let mut policy = SubagentPolicy {
        max_depth: 2,
        ..SubagentPolicy::default()
    };
    let model = settings.model.trim();
    if settings.free_mode && !model.is_empty() {
        policy.model = model.to_string();
        policy.extra_models = vec![model.to_string()];
    }
    policy
}

/// The Free-mode agent the Swarm room can launch. `None` when Free mode is off, no model is
/// saved, or the provider's API is not OpenAI-compatible (Anthropic, Vertex, a custom door).
fn free_swarm_agent(settings: &Settings) -> Option<rigel_core::agent::agentchat::FreeAgent> {
    if !settings.free_mode {
        return None;
    }
    let model = settings.model.trim();
    if model.is_empty() {
        return None;
    }
    let provider = settings.provider.trim().to_lowercase();
    let door = tauri::async_runtime::block_on(rigel_core::agent::providers::compatible_door(
        &provider,
        &credentials::KeychainCredentialStore,
    ))
    .ok()
    .flatten()?;
    Some(rigel_core::agent::agentchat::FreeAgent {
        base_url: door.base_url,
        model: model.to_string(),
        api_key: door.api_key,
    })
}

fn default_provider() -> String {
    "omniroute".to_string()
}

fn default_ui_scale() -> u16 {
    100
}

/// The separate reasoning-effort setting (D28). `medium` matches the old default model.
fn default_effort() -> String {
    "medium".to_string()
}

/// D90 defaults: notifications are on, but a platform that refuses permission stays quiet.
fn default_true() -> bool {
    true
}

fn valid_effort(effort: &str) -> bool {
    matches!(effort, "low" | "medium" | "high")
}

fn default_terminal_font_size() -> u16 {
    13
}

fn default_terminal_scrollback() -> u32 {
    8_000
}

/// The coordinator's step bound, as a setting: 10 by default (D19's loop).
fn default_max_steps() -> u16 {
    DEFAULT_MAX_STEPS as u16
}

/// Who leads a Swarm: Muse splits the prompt and assigns it, or the room gets one broadcast.
fn default_swarm_lead() -> String {
    "muse".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String,
    /// The cap a new session starts with, in cents. Zero means no cap; a running session keeps
    /// the cap it was created with.
    pub default_budget_cents: i64,
    /// `omniroute` (local router), `openrouter`, or `gemini`. Old settings files gain the
    /// default.
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Free mode: the saved provider and model are the free pair Free mode proved was answering.
    /// Picking a model by hand clears it, so the composer never claims a free model it is not on.
    #[serde(default)]
    pub free_mode: bool,
    /// The Google Cloud project Vertex is billed to, saved by the Providers page. Empty means
    /// "ask the environment, then the Cloud CLI"; a saved value sits between the two.
    #[serde(default)]
    pub vertex_project: String,
    /// Sent as `reasoning_effort` on every native turn; never folded into the model id.
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default = "default_true")]
    pub notify_approval: bool,
    #[serde(default = "default_true")]
    pub notify_turn_done: bool,
    #[serde(default = "default_true")]
    pub notify_turn_failed: bool,
    /// The Terminal surface: xterm's font size and scrollback, and the program to open —
    /// empty means PowerShell on Windows and `$SHELL` elsewhere.
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: u16,
    #[serde(default = "default_terminal_scrollback")]
    pub terminal_scrollback: u32,
    #[serde(default)]
    pub terminal_shell: String,
    /// Re-sync a skill source whose data is older than the stale window, in the background, so a
    /// shelf nobody has touched for a day updates itself instead of staying stale.
    #[serde(default = "default_true")]
    pub skills_auto_update: bool,
    /// How many model steps one native turn may take: read → edit → verify → answer, with room
    /// to recover from a bad tool call.
    #[serde(default = "default_max_steps")]
    pub max_steps: u16,
    /// Whether the model picker opens on chat-eligible rows only. Saved rather than kept per
    /// popover: a person who works with image or audio models should not re-tick the box on every
    /// launch.
    #[serde(default = "default_true")]
    pub chat_models_only: bool,
    /// Who leads a Swarm: `muse` queues the prompt for Muse to split and assign, `broadcast`
    /// sends the same text to every ticked agent. Old settings files gain `muse`.
    #[serde(default = "default_swarm_lead")]
    pub swarm_lead: String,
    /// Refine each composer draft with the cheap model and send the result without the review
    /// card. Off means Send opens the card and nothing is sent until the user confirms it.
    #[serde(default)]
    pub auto_refine: bool,
    /// Publish a session after each of its turns finishes, so the phone and the web can read it
    /// without a button. Off by default: publishing a transcript is opt-in.
    #[serde(default)]
    pub cloud_sync_sessions: bool,
    /// D159: the window's scale, one of 90/100/110/125. Applied to the shell, not the terminal's
    /// own font, so the PTY keeps its measured cell size.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: u16,
    /// D159: tighter rows in the transcript, list and diff. Layout only; no content is hidden.
    #[serde(default)]
    pub compact_density: bool,
    /// Q290: an explicit high-contrast palette, independent of the OS preference.
    #[serde(default)]
    pub high_contrast: bool,
    /// Q283: the local-time window (`HH:MM`) in which desktop notifications stay silent.
    /// Empty means quiet hours are off.
    #[serde(default)]
    pub quiet_hours_start: String,
    #[serde(default)]
    pub quiet_hours_end: String,
    /// Opt-in Gravity sponsored card on Home. Off by default: an ad request is a network call,
    /// and the user decides whether the paper carries one.
    #[serde(default)]
    pub gravity_ads: bool,
    /// Request test ads: no billing, no metrics, and a sample card even when no ad matches. The
    /// switch stays on until the publisher account is approved for production.
    #[serde(default = "default_true")]
    pub gravity_test_ads: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            default_budget_cents: 500,
            provider: default_provider(),
            model: default_model(),
            free_mode: false,
            vertex_project: String::new(),
            effort: default_effort(),
            notify_approval: true,
            notify_turn_done: true,
            notify_turn_failed: true,
            terminal_font_size: default_terminal_font_size(),
            terminal_scrollback: default_terminal_scrollback(),
            terminal_shell: String::new(),
            skills_auto_update: true,
            max_steps: default_max_steps(),
            chat_models_only: true,
            swarm_lead: default_swarm_lead(),
            auto_refine: false,
            cloud_sync_sessions: false,
            ui_scale: default_ui_scale(),
            compact_density: false,
            high_contrast: false,
            quiet_hours_start: String::new(),
            quiet_hours_end: String::new(),
            gravity_ads: false,
            gravity_test_ads: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyStatus {
    /// `true` per credential name — one entry per provider and one per harness key. A map rather
    /// than a field per vendor, so the adapter table, `rigel keys status` and this screen cannot
    /// drift apart. The renderer asks *whether* a key exists and never receives one.
    pub providers: BTreeMap<String, bool>,
    /// Whether any **chat** provider has a credential. The launch banner and the composer's hint
    /// both ask that question, and the adapter table is the only thing that knows the answer —
    /// a key that belongs to a harness alone does not make a native turn possible.
    pub native_ready: bool,
}

/// Q244: a successful save also says whether the key was checked, so Settings can report a
/// covered path instead of claiming every provider was tested.
#[derive(Debug, Clone, Serialize)]
pub struct KeySetResult {
    pub status: KeyStatus,
    /// `None` for a harness credential, which has no provider roster here; the harness checks it
    /// when it starts. A chat provider always carries a check, possibly `checked: false` when it
    /// publishes no model list.
    pub check: Option<catalog::KeyValidation>,
}

fn current_key_status() -> KeyStatus {
    let store = credentials::KeychainCredentialStore;
    KeyStatus {
        providers: credentials::PROVIDERS
            .iter()
            .map(|provider| (provider.to_string(), credentials::present(provider)))
            .collect(),
        native_ready: providers::all()
            .iter()
            .any(|spec| credentials::present_in(&store, spec.id)),
    }
}

/// One row per adapter, for Settings. Names, hints and whether a credential exists — never a
/// key, and never the value of one.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderRow {
    pub id: String,
    pub name: String,
    pub hint: String,
    /// A local door holds no credential, so `key_present == false` is a fact, not a gap.
    pub keyless: bool,
    /// The variable a headless run reads the key from.
    pub env: Option<String>,
    /// The credential is a short-lived token rather than a key, and the copy says so.
    pub access_token: bool,
    pub default_model: Option<String>,
    pub key_url: Option<String>,
    pub key_present: bool,
}

/// The API placement Settings shows. It is separate from the Firebase "Cloud" page: this is the
/// Rigel API the `rigel` model door and the relay bridge call, not Firestore sync.
#[derive(Debug, Clone, Serialize)]
pub struct ApiBackendStatus {
    pub mode: String,
    pub api_url: String,
    pub effective_url: String,
    pub hosted_url: String,
    pub override_env: Option<String>,
    pub relay_token_present: bool,
}

fn api_backend_status() -> ApiBackendStatus {
    let status = rigel_core::backend::status_at(&index_dir());
    ApiBackendStatus {
        mode: status.mode,
        api_url: status.api_url,
        effective_url: status.effective_url,
        hosted_url: status.hosted_url,
        override_env: status.override_env,
        relay_token_present: credentials::present(credentials::RELAY),
    }
}

/// The route decision both the inspector and the turn path read. One function means the two
/// surfaces cannot drift: the turn uses `resolved`, the inspector prints the same struct.
fn explain_route(
    settings: &Settings,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Result<RouteExplanation, String> {
    rigel_core::agent::routing::explain(
        &settings.provider,
        &settings.model,
        requested_provider,
        requested_model,
    )
}

/// The window-driver bootstrap: which project to open before the first human click. Shipped
/// builds return `None`; only the E2E runner sets `RIGEL_E2E_PROJECT`.
#[derive(Debug, Clone, Serialize)]
pub struct E2eConfig {
    pub project: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnReply {
    pub turn: Turn,
    pub budget_cents: Option<i64>,
    /// The exact explanation the inspector reads for the same inputs.
    pub route: RouteExplanation,
}

struct AppRuntime {
    cancels: Arc<Cancels>,
}

/// Forwards normalised events to the one channel the renderer subscribes to.
struct WindowSink {
    app: tauri::AppHandle,
}

impl EventSink for WindowSink {
    fn emit(&self, event: AgentEvent) {
        let _ = self.app.emit("rigel://events", event);
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn settings_path() -> PathBuf {
    rigel_core::default_index_dir().join("settings.toml")
}

/// Where the global index lives. `RIGEL_INDEX_DIR` exists so the window can be pointed at a
/// test index, the same way the CLI's `--data-dir` works.
fn index_dir() -> PathBuf {
    std::env::var_os("RIGEL_INDEX_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(rigel_core::default_index_dir)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// The two stores that cover every harness Rigel currently launches: the shared `.agents`
/// directory for Codex/Cursor/Cline and Claude's own directory for Claude Code.
fn skill_roots() -> Result<Vec<PathBuf>, String> {
    let home = home_dir().ok_or("no USERPROFILE/HOME to install a skill into")?;
    Ok(vec![
        home.join(".agents").join("skills"),
        home.join(".claude").join("skills"),
    ])
}

fn load_settings() -> Settings {
    let path = settings_path();
    if !path.exists() {
        return Settings::default();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/* --------------------------------- shelf --------------------------------- */

/* -------------------------------- plugins --------------------------------- */

/* ---------------------------------- MCPs ---------------------------------- */

/* -------------------------------- project -------------------------------- */

/// Where "New workspace" creates folders: `%USERPROFILE%\Rigel\<name>` (or `~/Rigel/<name>`).
fn workspace_root() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Rigel")
}

/* ------------------------------- attachments ------------------------------ */

/// Keep the original filename unless that name is already taken, then add the smallest suffix
/// that is free. Two files with the same name in one pick coexist instead of one overwriting the
/// other.
fn free_attachment_path(dir: &Path, name: &std::ffi::OsStr) -> Result<PathBuf, String> {
    let mut target = dir.join(name);
    if !target.exists() {
        return Ok(target);
    }

    let original = Path::new(name);
    let stem = original
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".to_string());
    let extension = original
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for suffix in 1..=9_999 {
        target = dir.join(format!("{stem}-{suffix}{extension}"));
        if !target.exists() {
            return Ok(target);
        }
    }
    Err(format!(
        "could not find a free name for {}",
        original.display()
    ))
}

fn image_data_url(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// One rendered page for the session inspector. This is a fresh Rigel b render of the URL an
/// external harness visited; the fork normally writes the exact frame itself, and this remains
/// the fallback for a harness configured without `--capture`.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserCapture {
    pub image: String,
    pub url: String,
    pub bytes: usize,
}

/// The exact frame the forked MCP server wrote after an external harness action.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserHarnessCapture {
    pub image: String,
    pub url: Option<String>,
    pub tool: Option<String>,
    pub captured_at: u64,
    pub bytes: usize,
}

/// D147: the palette's one read. Skills come from the local index; sessions, turns and patches
/// from this project's database. The current session sorts first, and every section is capped so
/// a one-letter query cannot return the whole history. A missing index is not a failed search —
/// the workspace half still answers.
#[derive(Debug, Clone, Serialize)]
pub struct PaletteSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub grade: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaletteResults {
    pub skills: Vec<PaletteSkill>,
    pub sessions: Vec<rigel_core::SessionHit>,
    pub turns: Vec<rigel_core::TurnHit>,
    pub patches: Vec<rigel_core::PatchHit>,
}

/// Reconcile a session with the tree and the patch journal (P3.11).
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsBundle {
    pub path: String,
    pub body: String,
}

/// Roadmap 116: the newest local crash records, already redacted before they were written by the
/// core's crash hook. The renderer builds one preview from this and the build facts in About.
#[derive(Debug, Clone, Serialize)]
pub struct CrashReport {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionExport {
    pub markdown_path: String,
    pub json_path: String,
    pub markdown: String,
}

/* --------------------------------- agents -------------------------------- */

async fn project_permissions(
    project: String,
    session: Option<String>,
) -> Result<rigel_core::PermissionMatrix, String> {
    let service = SessionService::open(std::path::Path::new(&project))
        .await
        .map_err(message)?;
    // The matrix is a project-wide ceiling, so it lists every session's agents, not only the one
    // Settings happened to have open. Muse's coordinator lives in its own session; omitting that
    // session made its column disappear and left tools that only a custom role covers (MCP, web,
    // mail) looking unassigned. The active session sorts first where it exists.
    let mut sessions = service.list().await.map_err(message)?;
    if let Some(active) = session.as_deref() {
        sessions.sort_by_key(|candidate| candidate.id != active);
    }
    let mut agents = Vec::new();
    for candidate in &sessions {
        agents.extend(service.agents(&candidate.id).await.map_err(message)?);
    }
    let rows: Vec<rigel_core::AgentPermissionRow> = agents
        .into_iter()
        .map(|agent| rigel_core::AgentPermissionRow {
            id: agent.id,
            session: Some(agent.session_id),
            role: agent.role,
            tool_overrides: agent.tool_overrides,
        })
        .collect();
    rigel_core::permissions::ProjectPermissions::matrix(std::path::Path::new(&project), &rows)
        .map_err(message)
}

/* --------------------------------- turns --------------------------------- */

/* -------------------------------- approvals ------------------------------- */

/// Answers `run_command` from the window instead of from a terminal.
///
/// The question goes onto the approval channel, the card lists it, and this waits for the scope
/// the user picked. Tools already holds the grants, so a "session" answer is checked there before
/// this is ever called; here every question is new. End of turn, a closed window or ten minutes of
/// silence all end as a refusal.
struct WindowApprover {
    bus: ApprovalBus,
    /// The same token registry `turn_cancel` uses. Waiting on a card must end when Stop lands,
    /// not when the ten-minute silence deadline does.
    cancels: Arc<Cancels>,
    /// D154: the turn this approver answers for. One is built per `turn_send`, so every
    /// question it publishes can point back at the transcript row that asked.
    session_id: String,
    agent_id: String,
    turn_id: String,
    /// D154: the project this question belongs to; Attention only shows its own.
    project_root: PathBuf,
}

impl WindowApprover {
    async fn wait_for_cancel(&self) {
        while !self.cancels.is_cancelled(&self.turn_id) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[async_trait::async_trait]
impl CommandApprover for WindowApprover {
    async fn approve(&self, request: &CommandRequest) -> CommandDecision {
        let key = format!("command:{}", request.command);
        let pending = PendingApproval::now("command", &key, &request.command)
            .in_project(&self.project_root)
            .in_session(&self.session_id, &self.agent_id)
            .on_turn(&self.turn_id)
            .with_detail(serde_json::json!({
                "cwd": request.cwd.display().to_string(),
                "project_root": request.project_root.display().to_string(),
                "timeout_secs": request.timeout_secs,
                "destructive": request.destructive.iter().map(|finding| {
                    serde_json::json!({ "id": finding.id, "summary": finding.summary })
                }).collect::<Vec<_>>(),
                // Q239: resolved targets and counts computed from the same classifier. This
                // is part of the question, not a separate approval path.
                "blast_radius": request.blast_radius,
                // Q73: the tool's own preview output (or an explicit no-preview note) is part
                // of the question, so the card never guesses at the effect.
                "dry_run": request.dry_run,
                // Q243: a script contributed by a loaded skill is part of the question, and
                // its own grant is required even when the command text is already approved.
                "skill_scripts": request.skill_scripts,
                "deliberation": request.deliberation.as_ref().map(|round| {
                    serde_json::json!({
                        "note": round.note,
                        "positions": round.positions.iter().map(|position| {
                            serde_json::json!({ "stance": position.stance, "text": position.text })
                        }).collect::<Vec<_>>(),
                    })
                }),
            }));
        // Q36: one silent deadline earns exactly one republish. Both attempts are still
        // refusals when nobody answers; the retry never turns silence into an approval.
        let pending_id = pending.id.clone();
        tokio::select! {
            result = self
                .bus
                .ask_with_retry(pending, std::time::Duration::from_secs(600), 1) =>
            {
                match result {
                    Ok(scope) => CommandDecision::Allow(scope),
                    Err(_) => CommandDecision::Deny,
                }
            }
            // Stop cancels the token. Answer the card too, or it would stay on screen after
            // the turn is gone.
            _ = self.wait_for_cancel() => {
                let _ = self.bus.answer(&pending_id, None);
                CommandDecision::Deny
            }
        }
    }
}

/// Q251: a committed-memory write is a question on the same card. It is project-local but
/// permanent, so it is never approved by a command grant; the card shows the exact file and
/// content, and silence is a refusal.
struct WindowMemoryApprover {
    bus: ApprovalBus,
    project_root: PathBuf,
}

#[async_trait::async_trait]
impl MemoryApprover for WindowMemoryApprover {
    async fn approve(&self, request: &MemoryRequest) -> MemoryDecision {
        let key = format!("memory:{}:{}", request.agent_id, request.topic);
        let pending = PendingApproval::now("memory", &key, format!("write {}", request.path))
            .in_project(&self.project_root)
            .with_detail(serde_json::json!({
                "agent_id": request.agent_id,
                "topic": request.topic,
                "mode": request.mode,
                "path": request.path,
                "content": request.content,
                "preview": request.preview,
            }));
        match self
            .bus
            .ask(pending, std::time::Duration::from_secs(600))
            .await
        {
            Ok(_) => MemoryDecision::Allow,
            Err(_) => MemoryDecision::Deny,
        }
    }
}

/// One outgoing email is a question on the same card. A sent message cannot be recalled, so the
/// recipient, subject and body are all put in front of the user; silence is a refusal, exactly
/// as it is for a command.
#[async_trait::async_trait]
impl MailApprover for WindowApprover {
    async fn approve(&self, request: &MailRequest) -> MailDecision {
        let key = format!("mail:{}:{}", request.to, request.subject);
        let title = format!("Send email to {}", request.to);
        let pending = PendingApproval::now("mail", &key, &title)
            .in_project(&self.project_root)
            .in_session(&self.session_id, &self.agent_id)
            .on_turn(&self.turn_id)
            .with_detail(serde_json::json!({
                "from": request.from,
                "to": request.to,
                "subject": request.subject,
                "text": request.text,
            }));
        let pending_id = pending.id.clone();
        tokio::select! {
            result = self.bus.ask(pending, std::time::Duration::from_secs(600)) => {
                match result {
                    Ok(_) => MailDecision::Allow,
                    Err(_) => MailDecision::Deny,
                }
            }
            _ = self.wait_for_cancel() => {
                let _ = self.bus.answer(&pending_id, None);
                MailDecision::Deny
            }
        }
    }
}

/// D114: a writable fan-out is a plan question on the same card, with its own verbs.
///
/// The card lists each child's role, files and plan; approving starts the fan-out, skipping
/// records the user's reason, and anything else refuses. The same ten-minute silence rule
/// applies: no answer is no permission.
#[async_trait::async_trait]
impl PlanApprover for WindowApprover {
    async fn approve_plan(&self, request: &PlanRequest) -> PlanDecision {
        let roles = request
            .agents
            .iter()
            .map(|agent| agent.role.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let pending = PendingApproval::now(
            "plan",
            format!("plan:{}:{}", request.session_id, request.agents.len()),
            format!("writable agents: {roles}"),
        )
        .in_project(&self.project_root)
        .in_session(&self.session_id, &self.agent_id)
        .on_turn(&self.turn_id)
        .with_detail(serde_json::json!({
            "session_id": request.session_id,
            "agents": request.agents,
        }));
        let pending_id = pending.id.clone();
        tokio::select! {
            decision = self
                .bus
                .ask_plan(pending, std::time::Duration::from_secs(600)) =>
            {
                decision
            }
            _ = self.wait_for_cancel() => {
                let _ = self.bus.answer(&pending_id, None);
                PlanDecision::Deny
            }
        }
    }
}

/// How often the window looks for a change in the approval channel. The channel is a directory
/// of small files, and the surfaces keep their own poll as the fallback.
const APPROVAL_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// D154: tell the window when the shared approval channel changes, so the Session card and the
/// Attention count refresh on the question itself instead of on their next poll.
///
/// The channel is a directory — one JSON file per open question — so watching its entries covers
/// every publisher, including the MCP sidecar, which has no window handle and publishes from
/// another process. A file's name and length are the whole signal: a new question, an answer
/// that removed one, and the last write completing a file all register as a change.
fn spawn_approval_watcher(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("rigel-approvals".to_string())
        .spawn(move || {
            let dir = ApprovalBus::new(index_dir()).pending_dir();
            let mut seen: Option<Vec<(String, u64)>> = None;
            loop {
                let mut now = std::fs::read_dir(&dir)
                    .map(|entries| {
                        entries
                            .flatten()
                            .map(|entry| {
                                let length = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                                (entry.file_name().to_string_lossy().into_owned(), length)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                now.sort();
                if seen.as_ref() != Some(&now) {
                    // The first look is only the baseline: a question already waiting is read
                    // when a surface mounts. After that, every change is worth a refresh.
                    if seen.is_some() {
                        let _ = app.emit("rigel://approval", ());
                    }
                    seen = Some(now);
                }
                std::thread::sleep(APPROVAL_POLL);
            }
        })
        .ok();
}

/* ---------------------------- claims and conflicts ------------------------ */

/// The project the window is watching. A global so the watcher thread can start at app boot,
/// before any window has opened a session, and simply idle until one does.
static WATCHED_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Everything the Diff view needs about coordination in one call.
#[derive(Debug, Clone, Serialize)]
pub struct CoordinationView {
    pub claims: Vec<rigel_core::Claim>,
    pub unverified_changes: Vec<UnverifiedChange>,
    pub conflicts: Vec<ClaimConflict>,
}

fn remember_project(root: &Path) {
    if let Ok(mut watched) = WATCHED_ROOT.lock() {
        *watched = Some(root.to_path_buf());
    }
    persist_current_project(root);
    persist_app_state(true);
}

/// Publish the project the window has open so a headless `rigel serve` can follow it. The
/// server polls this file and reopens its session database when the window switches projects;
/// a temp file plus rename means a reader never sees half a path. Failure is not fatal to the
/// window, which still has its project in memory.
fn persist_current_project(root: &Path) {
    let path = index_dir().join("current-workspace.json");
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let body = serde_json::json!({ "project": root.to_string_lossy() });
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, body.to_string()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// The window's liveness file. `updated_at` is refreshed every ten seconds; a stale file is a
/// crashed window, not an open one. The API reads this to decide whether Muse should wait.
fn persist_app_state(open: bool) {
    let path = index_dir().join("app-state.json");
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let body = serde_json::json!({
        "open": open,
        "project": current_project_root().map(|root| root.to_string_lossy().into_owned()),
        "updated_at": rigel_core::migrate::now_ms(),
    });
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, body.to_string()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Restart a room harness this window started when it stays busy without posting a reply.
/// The API runs the same watcher for the agents it started; each process can only restart its
/// own wrappers, because the room status carries no pid.
fn watch_room_agents() {
    if std::env::var("RIGEL_ROOM_WATCHDOG")
        .map(|value| value.trim() == "0")
        .unwrap_or(false)
    {
        return;
    }
    let stuck_after = std::env::var("RIGEL_ROOM_STUCK_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds >= 30)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(900));
    let state = Arc::new(Mutex::new(rigel_core::agent::agentchat::Watchdog::default()));
    rigel_core::agent::agentchat::spawn_watchdog(
        state,
        || current_project_root().map(|path| path.to_string_lossy().into_owned()),
        stuck_after,
        Duration::from_secs(30),
    );
}

pub(crate) fn current_project_root() -> Option<PathBuf> {
    WATCHED_ROOT.lock().ok().and_then(|root| root.clone())
}

/// Who leads a Swarm. Rigel is the Muse coordinator; a model override asks that same coordinator
/// to think with a particular model. Broadcast has no boss and is sent by the window directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum SwarmLead {
    Rigel,
    Broadcast,
    Model { provider: String, model: String },
}

/// One writable subagent, as the Diff view lists it (D80). The pending diff is deliberately
/// not here: this is read by the two-second coordination poll, and a worktree diff can be a
/// whole file.
#[derive(Debug, Clone, Serialize)]
pub struct WritableView {
    pub agent_id: String,
    pub role: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub worktree: Option<String>,
    pub plan: Option<String>,
    pub plan_note: Option<String>,
    /// The latest merge verdict, merged or not, approved or overridden.
    pub review: Option<MergeReview>,
}

/// The worktree's pending changes as one unified diff. Read once, when the row is expanded.
#[derive(Debug, Clone, Serialize)]
pub struct WritableDiff {
    pub agent_id: String,
    pub files: Vec<String>,
    pub diff: String,
}

/// Watch the open project for writes by agents other than the claimant. The watcher ticks every
/// [`POLL_INTERVAL`], which is also the worst-case detection latency (P5.8: within two seconds).
fn spawn_conflict_watcher(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("rigel-conflicts".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async move {
                let mut watcher: Option<(PathBuf, ConflictWatcher)> = None;
                loop {
                    tokio::time::sleep(POLL_INTERVAL).await;
                    let root = match WATCHED_ROOT.lock() {
                        Ok(watched) => watched.clone(),
                        Err(_) => None,
                    };
                    let Some(root) = root else {
                        watcher = None;
                        continue;
                    };
                    if watcher.as_ref().map(|(path, _)| path) != Some(&root) {
                        watcher = Some((root.clone(), ConflictWatcher::new(&root)));
                    }
                    let Some((_, conflicts)) = watcher.as_mut() else {
                        continue;
                    };
                    let Ok(service) = SessionService::open(&root).await else {
                        continue;
                    };
                    let Ok(sessions) = service.list().await else {
                        continue;
                    };
                    let Some(session) = sessions
                        .iter()
                        .find(|session| session.status == rigel_core::SessionStatus::Active)
                        .or_else(|| sessions.first())
                    else {
                        continue;
                    };
                    let Ok(agents) = service.agents(&session.id).await else {
                        continue;
                    };
                    let running: Vec<Agent> = agents
                        .into_iter()
                        .filter(|agent| agent.status == "running")
                        .collect();
                    if running.is_empty() {
                        continue;
                    }
                    let claims = ClaimService::new(service.pool().clone(), &root);
                    if let Ok(recorded) = conflicts.step(&claims, &running).await {
                        for conflict in recorded {
                            let _ = app.emit("rigel://conflict", &conflict);
                        }
                    }
                }
            });
        })
        .ok();
}

/* --------------------------------- network -------------------------------- */

/* --------------------------------- budget -------------------------------- */

/* --------------------------------- safety -------------------------------- */

/// D113 and D116 in one read: how many agents may write at once, what the day may cost, and
/// what it has cost so far.
#[derive(Debug, Clone, Serialize)]
pub struct SafetyView {
    pub writable_agents: i64,
    pub daily_cap_micros: Option<i64>,
    pub day: String,
    pub today_micros: i64,
}

/* ---------------------------------- plan --------------------------------- */

/* --------------------------------- patches ------------------------------- */

/* ----------------------------- hunk comments ----------------------------- */

/* --------------------------------- memory -------------------------------- */

/* --------------------------------- keys ---------------------------------- */

/* -------------------------------- gravity -------------------------------- */

/// The opt-in Gravity integration, as Settings sees it. The broker URL, never a publisher key.
#[derive(Debug, Clone, Serialize)]
pub struct GravityStatus {
    pub enabled: bool,
    pub test_ads: bool,
    pub placement_id: String,
    pub endpoint: String,
}

fn open_in_browser(url: &str) -> Result<(), String> {
    // The window suite clicks sponsor cards. Opening the developer's real default browser would
    // leave tabs behind (and can block on a browser that is still starting), so the runner asks
    // this process to exercise the click path without leaving the app.
    if std::env::var("RIGEL_E2E_SUPPRESS_EXTERNAL_BROWSER").as_deref() == Ok("1") {
        eprintln!("test: external browser open suppressed");
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not open the browser: {error}"))
}

/* ------------------------------- harnesses -------------------------------- */

/* ------------------------------- omniroute -------------------------------- */

/// The picker's last answer, held for five minutes. The read is network-bound and the two pickers
/// (Session and Settings) would otherwise each pay for it on mount.
type ModelCache = std::sync::Mutex<Option<(std::time::Instant, String, Vec<ModelOption>)>>;
static MODEL_CACHE: std::sync::OnceLock<ModelCache> = std::sync::OnceLock::new();

fn clear_model_cache() {
    if let Some(cache) = MODEL_CACHE.get()
        && let Ok(mut guard) = cache.lock()
    {
        *guard = None;
    }
}

fn models_cached(force: bool) -> Result<Vec<ModelOption>, String> {
    let provider = load_settings().provider.trim().to_lowercase();
    let cache = MODEL_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if !force
        && let Ok(guard) = cache.lock()
        && let Some((read_at, cached_for, rows)) = guard.as_ref()
        && cached_for == &provider
        // A minute, not five: free rows churn, and a picker that shows a row the door has
        // already dropped sends a 400 the user cannot explain.
        && read_at.elapsed() < std::time::Duration::from_secs(60)
    {
        return Ok(rows.clone());
    }
    let rows = tauri::async_runtime::block_on(catalog::roster(
        &provider,
        &credentials::KeychainCredentialStore,
    ))?;
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((std::time::Instant::now(), provider, rows.clone()));
    }
    Ok(rows)
}

/// Effort is its own setting (D28). OmniRoute happens to encode the same value in the model
/// id, so when a picked id already carries an effort suffix, keep the two consistent on the
/// wire rather than sending a request that contradicts itself. A `-tiered` id names the family,
/// not a tier: it stays intact, and the tier travels as `reasoning_effort`.
fn wire_model(model: &str, effort: &str, provider: &str) -> String {
    if provider != "omniroute" {
        return model.to_string();
    }
    // Do not invent `...-tiered-low`; the effort field is the tier.
    if model.ends_with("-tiered") {
        return model.to_string();
    }
    for suffix in ["-minimal", "-low", "-medium", "-high", "-xhigh"] {
        if let Some(base) = model.strip_suffix(suffix) {
            return format!("{base}-{effort}");
        }
    }
    model.to_string()
}

/* ------------------------------- agent chat ------------------------------- */

/* ------------------------------ prompt refine ----------------------------- */

/// Refinement is a fixed, small job: a cheap Antigravity model on the local router. Keeping it
/// off the turn's selected door means tidying a draft for the `rigel` agent never spends a relay
/// round-trip.
const REFINE_PROVIDER: &str = credentials::OMNIROUTE;
const REFINE_MODEL: &str = "antigravity/gemini-3.8-flash-tiered";

/* -------------------------------- settings ------------------------------- */

/// Roadmap 124: everything the About dialog needs, read from the build itself. No network call
/// and no machine-identifying value; the licences come from the generated SBOM list.
#[derive(Debug, Clone, Serialize)]
pub struct AboutInfo {
    pub version: String,
    pub commit: String,
    pub licences: Vec<String>,
    pub components: usize,
    pub update_channel: String,
    pub privacy: String,
}

#[derive(Deserialize)]
struct AboutLicenceSource {
    components: usize,
    expressions: Vec<String>,
}

fn about_snapshot() -> AboutInfo {
    // One generated file feeds both sides: TypeScript reads the exported constant, Rust reads
    // the JSON object inside it. The generator owns this shape and `make-about-licences` tests it.
    let raw = include_str!("../../../ui/src/about-licences.ts");
    let start = raw
        .find('{')
        .expect("the generated licence list has an object");
    let end = raw
        .rfind("} as const;")
        .expect("the generated licence list ends with a typed constant");
    let source: AboutLicenceSource = serde_json::from_str(&raw[start..end + 1])
        .expect("the generated About licence list must be valid JSON");
    let update = rigel_core::update::load_settings(&index_dir());
    AboutInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: env!("RIGEL_BUILD_COMMIT").to_string(),
        licences: source.expressions,
        components: source.components,
        update_channel: format!(
            "signed GitHub releases/latest · checks {} · automatic install {}",
            if update.enabled { "on" } else { "off" },
            if update.auto_install { "on" } else { "off" },
        ),
        privacy:
            "No telemetry. Sessions and diagnostics stay on this machine until you copy or export them."
                .to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub current: String,
    pub enabled: bool,
    pub auto_install: bool,
    pub manifest_url: String,
    pub key_present: bool,
}

fn update_status_now() -> Result<UpdateStatus, String> {
    let dir = index_dir();
    let settings = rigel_core::update::load_settings(&dir);
    Ok(UpdateStatus {
        current: env!("CARGO_PKG_VERSION").to_string(),
        enabled: settings.enabled,
        auto_install: settings.auto_install,
        manifest_url: rigel_core::update::manifest_url(&settings).to_string(),
        key_present: rigel_core::update::public_key(&dir).is_ok(),
    })
}

/// The project the background indexer follows. One watcher serves every project the window
/// opens; switching projects retargets it instead of starting a second scanner.
static GRAPH_TARGET: std::sync::OnceLock<Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
static GRAPH_STATUS: std::sync::OnceLock<Mutex<Option<rigel_core::codegraph::watch::WatchStatus>>> =
    std::sync::OnceLock::new();

fn graph_target() -> &'static Mutex<Option<PathBuf>> {
    GRAPH_TARGET.get_or_init(|| Mutex::new(None))
}

fn set_graph_target(root: &Path) {
    if let Ok(mut guard) = graph_target().lock() {
        *guard = Some(root.to_path_buf());
    }
}

fn graph_status_slot() -> &'static Mutex<Option<rigel_core::codegraph::watch::WatchStatus>> {
    GRAPH_STATUS.get_or_init(|| Mutex::new(None))
}

/// Watch the open project and keep `.rigel/codegraph.json` current. The scan itself is local.
/// When the tree changes and ten minutes have passed, one live free OpenRouter row names the
/// architecture concepts; a failure records the reason and leaves the deterministic graph intact.
fn spawn_code_graph_watch() {
    std::thread::Builder::new()
        .name("rigel-graph".to_string())
        .spawn(|| {
            use rigel_core::codegraph::watch;
            let mut fingerprint: Option<u64> = None;
            // The first completed scan enriches immediately; later passes are at most every
            // ten minutes, and only when the tree actually changed.
            let mut last_enrich = std::time::Instant::now() - Duration::from_secs(700);
            loop {
                let target = graph_target().lock().ok().and_then(|guard| guard.clone());
                let Some(root) = target else {
                    std::thread::sleep(Duration::from_secs(20));
                    continue;
                };
                let current = match watch::fingerprint(&root) {
                    Ok(current) => Some(current),
                    Err(error) => {
                        record_graph_status(&root, None, Some(error));
                        None
                    }
                };
                if let Some(current) = current
                    && fingerprint != Some(current)
                {
                    match watch::index(&root) {
                        Ok(mut graph) => {
                            let previous_agent =
                                watch::read_status(&root).and_then(|status| status.agent);
                            let mut agent = previous_agent;
                            if last_enrich.elapsed() >= Duration::from_secs(600) {
                                last_enrich = std::time::Instant::now();
                                let store = credentials::KeychainCredentialStore;
                                match tauri::async_runtime::block_on(watch::concepts(
                                    &root, &graph, &store,
                                )) {
                                    Ok((nodes, edges, model)) => {
                                        let count = nodes.len();
                                        watch::apply_concepts(&mut graph, nodes, edges);
                                        if let Err(error) = watch::write_graph(&root, &graph) {
                                            agent = Some(format!("index write failed: {error}"));
                                        } else {
                                            agent = Some(format!("{model} · {count} concepts"));
                                        }
                                    }
                                    Err(error) => {
                                        agent = Some(format!("free agent skipped: {error}"));
                                    }
                                }
                            }
                            record_graph_status(&root, Some((&graph, agent)), None);
                            fingerprint = Some(current);
                        }
                        Err(error) => record_graph_status(&root, None, Some(error)),
                    }
                }
                std::thread::sleep(Duration::from_secs(20));
            }
        })
        .ok();
}

fn record_graph_status(
    root: &Path,
    graph: Option<(&rigel_core::codegraph::CodeGraph, Option<String>)>,
    error: Option<String>,
) {
    let previous = rigel_core::codegraph::watch::read_status(root);
    let status = if let Some((graph, agent)) = graph {
        rigel_core::codegraph::watch::WatchStatus {
            project: graph.root.clone(),
            updated_unix: rigel_core::codegraph::watch::now_unix(),
            files: graph.files,
            nodes: graph.nodes.len(),
            edges: graph.edges.len(),
            revision: previous.as_ref().map(|status| status.revision).unwrap_or(0) + 1,
            concepts: graph
                .nodes
                .iter()
                .filter(|node| node.kind == "concept")
                .count(),
            agent,
            error: None,
        }
    } else {
        let mut status = previous.unwrap_or_default();
        status.project = root.to_string_lossy().into_owned();
        status.error = error;
        status
    };
    rigel_core::codegraph::watch::write_status(root, &status);
    if let Ok(mut guard) = graph_status_slot().lock() {
        *guard = Some(status);
    }
}

/// The deferred-replace helper branch. This process only waits for the app to exit, swaps the
/// executable and starts the new build; it never opens a window or a Tauri runtime.
pub fn update_helper_mode<I>(args: I) -> Option<i32>
where
    I: IntoIterator,
    I::Item: AsRef<std::ffi::OsStr>,
{
    let plan = rigel_core::update::apply::helper_request(args)?;
    let code = match rigel_core::update::apply::run_helper(&plan) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("rigel: update helper: {error}");
            1
        }
    };
    Some(code)
}

/// One signed check at launch, off the window thread. A current build, a missing release, a
/// timeout and a rate-limited GitHub all end the same way: nothing visible, and at most one line
/// in `update.log`. A newer signed release stages its binary and restarts into it.
fn spawn_launch_update(app: tauri::AppHandle) {
    if std::env::var_os(rigel_core::update::UPDATED_ENV).is_some() {
        // The build that just started is the update; do not check again on this launch.
        return;
    }
    if cfg!(debug_assertions) && std::env::var_os("RIGEL_UPDATE_LAUNCH").is_none() {
        // A development build must never replace the executable `cargo run` is using.
        return;
    }
    std::thread::spawn(move || {
        let dir = index_dir();
        let settings = rigel_core::update::load_settings(&dir);
        if !settings.enabled || !settings.auto_install {
            return;
        }
        let current = env!("CARGO_PKG_VERSION");
        let check =
            match tauri::async_runtime::block_on(rigel_core::update::check_latest(&dir, current)) {
                Ok(check) => check,
                Err(error) => {
                    // Offline, 404 and rate-limited all land here. One quiet line, no window change.
                    rigel_core::update::log_line(&dir, &format!("no update: {error}"));
                    return;
                }
            };
        if check.available && rigel_core::update::apply::rollback_blocked(&dir, &check.offered) {
            rigel_core::update::log_line(
                &dir,
                &format!(
                    "no update: {} was rolled back after a failed launch; retrying after 24h",
                    check.offered
                ),
            );
            return;
        }
        if !check.available {
            rigel_core::update::log_line(&dir, &format!("up to date {current}"));
            return;
        }

        let exe = std::env::current_exe().unwrap_or_default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if check.bin_url.is_some() && rigel_core::update::apply::can_replace(&exe) {
            let cwd = std::env::current_dir().ok();
            let staged = tauri::async_runtime::block_on(rigel_core::update::stage_binary(
                &dir,
                &check,
                &exe,
                std::process::id(),
                args,
                cwd,
            ));
            match staged {
                Ok(Some(staged)) => match rigel_core::update::apply::spawn_helper(&staged.plan) {
                    Ok(()) => {
                        rigel_core::update::log_line(
                            &dir,
                            &format!("staged {} {}", staged.version, staged.plan.display()),
                        );
                        std::thread::sleep(Duration::from_millis(400));
                        quit(&app);
                    }
                    Err(error) => {
                        rigel_core::update::log_line(&dir, &format!("update failed: {error}"))
                    }
                },
                Ok(None) => rigel_core::update::log_line(
                    &dir,
                    &format!("no update: {} has no binary asset", check.offered),
                ),
                Err(error) => {
                    rigel_core::update::log_line(&dir, &format!("update failed: {error}"))
                }
            }
            return;
        }

        // A per-machine install cannot write its own directory; its signed installer does the
        // replacement and relaunch instead.
        let Some((url, sha256)) =
            rigel_core::update::available_artifact(&check, installed_per_machine())
        else {
            rigel_core::update::log_line(&dir, &format!("up to date {current}"));
            return;
        };
        match tauri::async_runtime::block_on(rigel_core::update::download_artifact(
            url, sha256, &dir,
        )) {
            Ok(path) => match rigel_core::update::install_artifact(&path) {
                Ok(()) => {
                    rigel_core::update::log_line(
                        &dir,
                        &format!("staged installer {}", check.offered),
                    );
                    std::thread::sleep(Duration::from_millis(400));
                    quit(&app);
                }
                Err(error) => {
                    rigel_core::update::log_line(&dir, &format!("update failed: {error}"))
                }
            },
            Err(error) => rigel_core::update::log_line(&dir, &format!("update failed: {error}")),
        }
    });
}

/// A per-machine MSI lives under Program Files; a per-user NSIS install does not. The updater
/// uses this only to choose between the two signed URLs in the same manifest.
fn installed_per_machine() -> bool {
    let exe = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|root| PathBuf::from(root).to_string_lossy().to_lowercase())
        .any(|root| !root.is_empty() && exe.starts_with(&root))
}

/* ----------------------------------- mail ----------------------------------- */

/* ---------------------------------- cloud ---------------------------------- */

/// Launch the local router as the window opens, so the first turn does not pay for a cold
/// start. Lazy start in `turn_send` stays as the fallback: this is an optimisation and must
/// never block the window, which is why it runs on its own thread and its error is ignored.
fn warm_router() {
    if load_settings().provider != "omniroute" || !credentials::present(credentials::OMNIROUTE) {
        return;
    }
    std::thread::Builder::new()
        .name("rigel-omniroute".to_string())
        .spawn(|| {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime.block_on(rigel_core::agent::omniroute::ensure_server());
        })
        .ok();
}

/// The bundled sidecar, when this build has one. An install reads `$RESOURCE/sidecar`; only a
/// debug build may fall back to the repository copy. Baking the build machine's path into a
/// release would let a packaged app read a sidecar from wherever it happened to be compiled.
fn chat_sidecar_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    let packaged = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("sidecar"))
        .filter(|dir| dir.join("agentchattr/run.py").is_file());
    if packaged.is_some() {
        return packaged;
    }
    #[cfg(debug_assertions)]
    {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/sidecar");
        if dev.join("agentchattr/run.py").is_file() {
            return Some(dev);
        }
    }
    None
}

/// Hand the bundled chat sidecar to the core. The source is copied into app data on first launch,
/// so the room can write while `python.exe` stays in the read-only install directory.
///
/// Absent is not an error: a developer without the sidecar keeps the `vendor/agentchattr` fallback,
/// and the Chat tab explains that case.
fn install_chat_sidecar(app: &tauri::AppHandle) -> Result<String, String> {
    let Some(source) = chat_sidecar_dir(app) else {
        return Ok(String::new());
    };
    rigel_core::agent::agentchat::install_sidecar(source, index_dir().join("agentchat"))
}

/// The release publisher's public key ships beside the executable. Copy it into app data so the
/// CLI and the window verify the same manifest; a rotation replaces the old copy on next launch.
fn install_update_pubkey(app: &tauri::AppHandle) -> Result<(), String> {
    let source = app
        .path()
        .resource_dir()
        .map_err(message)?
        .join("update-pubkey.txt");
    if !source.is_file() {
        return Ok(());
    }
    let target = index_dir().join(rigel_core::UPDATE_PUBKEY_FILE);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(message)?;
    }
    if fs::read_to_string(&source).ok() != fs::read_to_string(&target).ok() {
        fs::copy(&source, &target).map_err(message)?;
    }
    Ok(())
}

/// Launch the chat server when the open project carries its checkout and nothing is answering.
/// Same contract as `warm_router`: a thread, a dropped error, and the Chat tab as the place the
/// reason appears. `ensure_server` already returns without spawning when the checkout is not
/// there, so a fresh clone costs one failed lookup rather than a hanging launch.
fn warm_chat(project: Option<String>) {
    std::thread::Builder::new()
        .name("rigel-agentchat".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime.block_on(rigel_core::agent::agentchat::ensure_server(
                project.as_deref(),
            ));
        })
        .ok();
}

/// How often the open window asks the index which sources are due. Fresh sources cost a SQLite
/// read, not a request; a stale one is re-synced on that pass and a broken one is retried on the
/// core's bounded backoff.
const SKILLS_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Keep skill source health current for as long as Rigel is running. The first pass is forced, so
/// a shelf that was fresh when the window last closed is still compared with its remote on this
/// launch; after that, a five-minute pass re-syncs whatever the core says is due. A successful
/// pass emits `rigel://skills`, so the status line and Settings update without a manual refresh
/// or a reopen. Same contract as `warm_router`: a thread, a dropped error, and the shelf as the
/// place the result appears.
///
/// The switch is checked on every tick rather than only at launch, so turning it off stops the
/// network work immediately and turning it back on resumes it. The thread deliberately survives a
/// disabled stretch: that is what makes re-enabling immediate. A static flag keeps the two
/// `servers_start` calls (no project, then the restored project) from starting two checkers.
fn warm_skills(app: tauri::AppHandle) {
    if !load_settings().skills_auto_update {
        return;
    }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("rigel-skills".to_string())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                STARTED.store(false, Ordering::SeqCst);
                return;
            };
            runtime.block_on(async move {
                let Ok(index) = SkillIndex::open(&index_dir()).await else {
                    STARTED.store(false, Ordering::SeqCst);
                    return;
                };
                // Forced, GitHub repositories included: a shelf that was fresh on the last
                // launch is still checked against its remote on this one.
                if index.refresh_all().await.is_ok() {
                    let _ = app.emit("rigel://skills", ());
                }
                loop {
                    tokio::time::sleep(SKILLS_CHECK_INTERVAL).await;
                    if !load_settings().skills_auto_update {
                        continue;
                    }
                    // The core leaves fresh sources alone and spaces out retries of a dead one.
                    // Only a pass that actually wrote rows reloads the shelf's list; the status
                    // read has its own poll, so a failed retry still updates its row.
                    if matches!(index.refresh_stale().await, Ok(report) if report.refreshed > 0) {
                        let _ = app.emit("rigel://skills", ());
                    }
                }
            });
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// Warm the picker's roster off the interaction path. The read is three vendor calls over the
/// network; doing it at launch means the first open is served from the cache instead of
/// waiting on them. A failure is dropped: the picker retries when it opens.
fn warm_models() {
    std::thread::Builder::new()
        .name("rigel-models".to_string())
        .spawn(|| {
            // Forced every three minutes: free rows churn, and a picker that shows a row the
            // door has already dropped sends a 400 the user cannot explain. The first pass is
            // immediate; a failed pass leaves the previous cache in place and tries again.
            loop {
                let _ = commands::providers::models_refresh();
                std::thread::sleep(Duration::from_secs(180));
            }
        })
        .ok();
}

/// A close request hides the window rather than quitting while the tray exists. The flag flips
/// only when the tray was built, so a platform that refuses a tray keeps a working close button
/// instead of stranding a hidden process with no way back.
static TRAY_READY: AtomicBool = AtomicBool::new(false);
/// The window heartbeat keeps `app-state.json` fresh while this process is alive. A crashed
/// window stops refreshing it, so the API can report the app closed without an exit hook.
static APP_OPEN: AtomicBool = AtomicBool::new(false);
/// Set by the tray's Quit item and by an update that needs the executable replaced. A close
/// request is allowed through once this is true, so shutdown is never trapped by the hide.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

fn hide_tray_panel(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("tray") {
        let _ = window.hide();
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    hide_tray_panel(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn show_tray_panel(app: &tauri::AppHandle, x: f64, y: f64) {
    let Some(window) = app.get_webview_window("tray") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let width = window
        .outer_size()
        .map(|size| size.width as i32)
        .unwrap_or(380);
    let height = window
        .outer_size()
        .map(|size| size.height as i32)
        .unwrap_or(560);
    let x = (x.round() as i32 - width + 12).max(8);
    let y = (y.round() as i32 - height - 12).max(8);
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = window.show();
    let _ = window.set_focus();
}

/// Quit through the same path the tray uses, so an updater-triggered exit is not swallowed by
/// the close-to-tray handler.
fn quit(app: &tauri::AppHandle) {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// The tray is the way back to a hidden window and the only way to stop the agents while the
/// window is closed. It opens a themed panel instead of an OS-owned menu.
fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id("rigel")
        .tooltip("Rigel — agents and spend live here while the window is closed")
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => show_main_window(tray.app_handle()),
            TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                position,
                ..
            } => show_tray_panel(tray.app_handle(), position.x, position.y),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    TRAY_READY.store(true, Ordering::SeqCst);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // D82: a panic writes a redacted record beside the app data before the window disappears.
    diagnostics::install_crash_hook(index_dir());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // The project the window saved is read after the environment and before the Cloud
            // CLI; load it before the window can start a turn so it survives every launch.
            vertex::set_project_override(Some(&load_settings().vertex_project));
            // The sidecar must be in place before `servers_start` looks for one. The project is
            // not known until the window restores it; `servers_start` runs again from there, and
            // the copied config is pointed at whichever project is open. The router needs none.
            if let Err(problem) = install_chat_sidecar(app.handle()) {
                eprintln!("rigel: chat sidecar not installed: {problem}");
            }
            if let Err(problem) = install_update_pubkey(app.handle()) {
                eprintln!("rigel: update public key not installed: {problem}");
            }
            if let Err(problem) = install_tray(app.handle()) {
                eprintln!("rigel: tray icon not installed: {problem}");
            }
            spawn_launch_update(app.handle().clone());
            // The window driver attaches to a single webview, and the tray is a second one; it
            // would be the target it lands on. A shipped build keeps both windows.
            if std::env::var_os("RIGEL_E2E_PROJECT").is_some()
                && let Some(tray) = app.get_webview_window("tray")
            {
                let _ = tray.destroy();
            }
            std::thread::spawn(tray_status::warm);
            // The skills that ship with Rigel are in the local index before the first search.
            // Idempotent and offline: the body blob is addressed by SHA-256, and a second launch
            // changes no row. Nothing is copied into a harness store here; that is an explicit
            // `rigel skills builtin --install`.
            std::thread::spawn(|| {
                tauri::async_runtime::block_on(async {
                    if let Ok(index) = SkillIndex::open(&index_dir()).await {
                        let _ = index.install_builtin().await;
                    }
                });
            });
            relay::start(index_dir(), current_project_root);
            // The window is the supervisor the user actually keeps running: it starts the API on
            // launch and again after a crash, so a reboot no longer leaves Muse with a refusal.
            // Wait for the API before the first paint: a fresh logon otherwise opened the window
            // while the server was still binding, and a localhost request looked like a dead app.
            if let Err(problem) =
                control::ensure(&index_dir(), current_project_root, Duration::from_secs(12))
            {
                eprintln!("rigel: {problem}");
                control::log(&index_dir(), &problem);
            }
            control::start(index_dir(), current_project_root);
            watch_room_agents();
            spawn_code_graph_watch();
            warm_router();
            APP_OPEN.store(true, Ordering::SeqCst);
            persist_app_state(true);
            std::thread::spawn(|| {
                while APP_OPEN.load(Ordering::SeqCst) {
                    persist_app_state(true);
                    std::thread::sleep(Duration::from_secs(10));
                }
            });
            spawn_approval_watcher(app.handle().clone());
            spawn_conflict_watcher(app.handle().clone());
            // The main window is created hidden so the API wait is the first thing the user
            // sees, or rather does not see. Show it once setup has reached this point even when
            // there was no project or the API failed, so the app is never a hidden process.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }
            // A deferred update keeps the previous binary until this process proves it reached
            // the UI. The helper set the marker path for this one launch; a normal launch is a
            // no-op because no path is present.
            rigel_core::update::apply::mark_healthy();
            Ok(())
        })
        .manage(AppRuntime {
            cancels: Arc::new(Cancels::new()),
        })
        .on_window_event(|window, event| {
            if window.label() == "tray" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                } else if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && TRAY_READY.load(Ordering::SeqCst)
                && !SHUTTING_DOWN.load(Ordering::SeqCst)
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::workspace::project_open,
            commands::workspace::project_create,
            commands::workspace::project_workspace_root,
            commands::workspace::attachment_import,
            commands::sessions::session_start,
            commands::sessions::session_list,
            commands::sessions::session_rename,
            commands::sessions::session_pin,
            commands::sessions::session_skill_pins,
            commands::sessions::session_skill_pin,
            commands::sessions::session_skill_unpin,
            commands::sessions::session_skill_review,
            commands::sessions::session_skill_keep,
            commands::sessions::session_skill_keep_all,
            commands::sessions::session_skill_update,
            commands::sessions::session_skill_update_all,
            commands::sessions::session_archive,
            commands::sessions::session_delete,
            commands::sessions::bookmark_list,
            commands::sessions::bookmark_toggle,
            commands::sessions::patch_provenance,
            commands::sessions::template_list,
            commands::sessions::template_save,
            commands::sessions::template_delete,
            commands::sessions::attention_list,
            commands::sessions::session_get,
            commands::sessions::session_transcript,
            commands::workspace::browser_capture,
            commands::workspace::browser_harness_capture,
            commands::workspace::browser_image,
            commands::workspace::search_all,
            commands::sessions::session_resume,
            commands::sessions::session_recovery,
            commands::sessions::turn_discard,
            commands::workspace::diagnostics_export,
            commands::workspace::crash_report,
            commands::workspace::session_export,
            commands::workspace::tests_run,
            commands::workspace::rules_sources,
            commands::workspace::pull_request_plan,
            commands::workspace::pull_request_create,
            commands::workspace::rules_export_memory,
            commands::workspace::checkpoint_list,
            commands::workspace::checkpoint_restore,
            commands::agents::agent_list,
            commands::agents::permissions_get,
            commands::agents::permissions_set,
            commands::agents::permissions_set_agent,
            commands::agents::agent_team,
            commands::agents::agent_spawn,
            commands::agents::agent_stop,
            commands::turns::turn_send,
            commands::turns::turn_cancel,
            commands::turns::turn_pause,
            commands::turns::turn_resume,
            commands::turns::turn_steer,
            commands::safety::network_get,
            commands::safety::network_set,
            commands::safety::network_check,
            commands::turns::approval_pending,
            commands::turns::approval_answer,
            commands::turns::watch_project,
            commands::swarm::swarm_enqueue,
            commands::coordination::claim_list,
            commands::coordination::claim_release,
            commands::settings::about_info,
            commands::audit::audit_query,
            commands::audit::audit_export,
            commands::coordination::merge_queue,
            commands::coordination::writable_list,
            commands::coordination::writable_diff,
            commands::coordination::writable_merge,
            commands::coordination::conflict_resolve,
            commands::coordination::conflict_sides,
            commands::coordination::conflict_merge,
            commands::coordination::conflict_resolve_merged,
            commands::turns::turn_estimate,
            commands::safety::budget_get,
            commands::safety::budget_set,
            commands::safety::budget_raise,
            commands::safety::safety_get,
            commands::safety::safety_set,
            commands::patches::patch_list,
            commands::patches::patch_hunks,
            commands::patches::hunk_comment_add,
            commands::patches::hunk_comments,
            commands::patches::hunk_comment_deliver,
            commands::patches::patch_revert,
            commands::memory::memory_read,
            commands::memory::memory_write,
            commands::memory::memory_facts,
            commands::memory::memory_recall,
            commands::memory::memory_pin,
            commands::providers::key_status,
            commands::providers::key_set,
            commands::providers::key_delete,
            commands::providers::gravity_settings,
            commands::providers::gravity_ad,
            commands::providers::gravity_impression,
            commands::providers::gravity_click,
            commands::turns::plan_get,
            commands::turns::plan_set,
            commands::turns::subagent_graph,
            commands::agents::role_presets,
            commands::agents::role_apply,
            commands::agents::role_clear,
            commands::agents::agent_configure,
            commands::safety::spend_report,
            commands::safety::task_cap_set,
            commands::providers::harness_probe,
            commands::providers::backend_status,
            commands::providers::backend_set,
            commands::providers::backend_set_token,
            commands::providers::backend_clear_token,
            commands::providers::providers_list,
            commands::providers::routing_policy,
            commands::providers::custom_provider_get,
            commands::providers::custom_provider_probe,
            commands::providers::custom_provider_save,
            commands::providers::route_explain,
            commands::providers::e2e_config,
            commands::shell::servers_start,
            commands::providers::omniroute_status,
            commands::providers::omniroute_start,
            commands::providers::local_model_status,
            commands::providers::local_model_start,
            commands::providers::local_model_stop,
            commands::providers::models_list,
            commands::providers::models_refresh,
            commands::providers::provider_health,
            commands::providers::model_capabilities,
            commands::settings::free_mode,
            commands::settings::free_agents,
            commands::settings::code_graph_watch_status,
            commands::chat::chat_status,
            commands::chat::chat_start,
            commands::chat::chat_channels,
            commands::chat::chat_agents,
            commands::chat::chat_messages,
            commands::chat::chat_attachment,
            commands::chat::chat_send,
            commands::chat::chat_start_agents,
            commands::chat::chat_start_free,
            commands::chat::chat_launched,
            commands::chat::chat_harnesses,
            commands::chat::chat_start_agent,
            commands::chat::chat_stop_agent,
            concord::concord_inbox,
            concord::concord_reply,
            terminal::terminal_open,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_close,
            commands::settings::prompt_refine,
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::settings::update_status,
            commands::settings::update_set,
            commands::settings::code_graph,
            commands::settings::update_check,
            commands::settings::update_install,
            commands::cloud::cloud_status,
            commands::cloud::cloud_configure,
            commands::cloud::cloud_login,
            commands::cloud::cloud_signup,
            commands::cloud::cloud_reset_password,
            commands::cloud::cloud_send_verification,
            commands::cloud::cloud_check_verification,
            commands::cloud::cloud_app_check,
            commands::cloud::cloud_app_check_client_token,
            commands::cloud::cloud_app_check_clear_client_token,
            commands::cloud::cloud_logout,
            commands::cloud::mail_status,
            commands::cloud::mail_configure,
            commands::cloud::mail_set_key,
            commands::cloud::mail_threads,
            commands::cloud::mail_thread,
            commands::cloud::mail_send,
            commands::cloud::cloud_push,
            commands::cloud::cloud_pull,
            commands::cloud::cloud_publish_session,
            commands::cloud::cloud_publish_all,
            commands::shell::usage_summary,
            commands::shell::tray_snapshot,
            commands::shell::tray_open,
            commands::shell::tray_hide,
            commands::shell::tray_quit,
            commands::skills::skills_search,
            commands::skills::skills_count,
            commands::skills::skills_load,
            commands::skills::skills_status,
            commands::skills::skills_index,
            commands::skills::skills_refresh,
            commands::skills::skills_install,
            commands::skills::skills_installed,
            commands::skills::plugins_catalog,
            commands::skills::plugins_list,
            commands::skills::plugins_install,
            commands::skills::plugins_remove,
            commands::skills::plugins_set_enabled,
            commands::skills::mcp_catalog_search,
            commands::skills::mcp_catalog_count,
            commands::skills::mcp_install,
            commands::skills::mcp_installed
        ])
        .build(tauri::generate_context!())
        .expect("rigel: failed to build the window")
        .run(|_, event| {
            // The agents are this app's children; without this, closing the window leaves hidden
            // wrappers holding room slots that the next launch cannot clean up.
            if matches!(event, tauri::RunEvent::Exit) {
                APP_OPEN.store(false, Ordering::SeqCst);
                persist_app_state(false);
                rigel_core::agent::agentchat::stop_all();
                terminal::close_all();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::safety::{network_get, network_set};
    use crate::commands::sessions::{
        session_skill_pin, session_skill_pins, session_skill_review, session_start, template_save,
    };
    use crate::commands::workspace::attachment_import;

    #[test]
    fn about_names_the_build_commit_and_the_sbom_licence_list() {
        let info = about_snapshot();
        assert_eq!(info.commit, env!("RIGEL_BUILD_COMMIT"));
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.components > 0);
        assert!(info.licences.contains(&"MIT".to_string()));
        assert!(info.licences.contains(&"Apache-2.0".to_string()));
        assert!(
            info.update_channel
                .contains("signed GitHub releases/latest")
        );
        assert!(info.privacy.contains("No telemetry"));
    }

    #[test]
    fn the_refiner_is_the_cheap_antigravity_model_on_the_local_router() {
        let spec = providers::find(REFINE_PROVIDER).expect("the local router is a provider");
        assert!(REFINE_MODEL.starts_with("antigravity/"));
        assert!(REFINE_MODEL.contains("3.8-flash-tiered"));
        assert!(providers::is_model(spec, REFINE_MODEL));
    }

    #[test]
    fn a_network_rule_the_core_rejects_is_not_written_by_the_editor_command() {
        let tmp = tempfile::tempdir().unwrap();
        let project = path_string(tmp.path());
        let bad = NetworkPolicy {
            rules: vec![rigel_core::NetworkRule {
                action: rigel_core::NetworkAction::Deny,
                pattern: "example.com/path".to_string(),
                note: None,
            }],
            ..NetworkPolicy::default()
        };
        assert!(network_set(project.clone(), bad).is_err());
        assert!(!tmp.path().join(".rigel/network.json").exists());

        let good = NetworkPolicy {
            rules: vec![rigel_core::NetworkRule {
                action: rigel_core::NetworkAction::Deny,
                pattern: "tracker.example".to_string(),
                note: None,
            }],
            limits: rigel_core::EgressLimits {
                per_origin_requests: 3,
                max_response_bytes: 8_192,
                ..rigel_core::EgressLimits::default()
            },
            ..NetworkPolicy::default()
        };
        network_set(project.clone(), good.clone()).unwrap();
        assert_eq!(network_get(project).unwrap(), good);
    }

    #[test]
    fn a_tiered_id_keeps_its_id_and_effort_is_sent_separately() {
        assert_eq!(
            wire_model("antigravity/gemini-3.8-flash-tiered", "low", "omniroute"),
            "antigravity/gemini-3.8-flash-tiered"
        );
        assert_eq!(
            wire_model("antigravity/gemini-3.7-flash-low", "high", "omniroute"),
            "antigravity/gemini-3.7-flash-high"
        );
        assert_eq!(
            wire_model("antigravity/gemini-3.8-flash-tiered", "high", "openrouter"),
            "antigravity/gemini-3.8-flash-tiered"
        );
    }

    #[test]
    fn a_swarm_lead_records_who_the_big_boss_is() {
        assert_eq!(
            serde_json::to_value(SwarmLead::Rigel).unwrap(),
            serde_json::json!({ "kind": "rigel" })
        );
        assert_eq!(
            serde_json::to_value(SwarmLead::Model {
                provider: "meta".to_string(),
                model: "muse-spark-1.3".to_string(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "model",
                "provider": "meta",
                "model": "muse-spark-1.3"
            })
        );
        let parsed: SwarmLead = serde_json::from_value(serde_json::json!({
            "kind": "model",
            "provider": "meta",
            "model": "muse-spark-1.3"
        }))
        .unwrap();
        assert!(matches!(parsed, SwarmLead::Model { .. }));
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn attachment_import_copies_into_the_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = outside.path().join("notes.txt");
        fs::write(&source, "hello").unwrap();

        let imported = attachment_import(path_string(project.path()), vec![path_string(&source)])
            .expect("the file should import");

        assert_eq!(imported, vec![".rigel/attachments/notes.txt"]);
        assert_eq!(
            fs::read_to_string(project.path().join(".rigel/attachments/notes.txt")).unwrap(),
            "hello"
        );
        assert!(
            project
                .path()
                .join(".rigel/attachments/.gitignore")
                .is_file()
        );
    }

    #[test]
    fn a_second_file_with_the_same_name_gets_a_suffix() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let first = outside.path().join("one").join("notes.txt");
        let second = outside.path().join("two").join("notes.txt");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "one").unwrap();
        fs::write(&second, "two").unwrap();

        let imported = attachment_import(
            path_string(project.path()),
            vec![path_string(&first), path_string(&second)],
        )
        .expect("both files should import");

        assert_eq!(
            imported,
            vec![
                ".rigel/attachments/notes.txt",
                ".rigel/attachments/notes-1.txt"
            ]
        );
        assert_eq!(
            fs::read_to_string(project.path().join(".rigel/attachments/notes-1.txt")).unwrap(),
            "two"
        );
    }

    #[test]
    fn a_directory_is_not_an_attachment() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let directory = outside.path().join("folder");
        fs::create_dir_all(&directory).unwrap();

        let error = attachment_import(path_string(project.path()), vec![path_string(&directory)])
            .expect_err("a directory must be refused");

        assert!(error.contains("is not a file"), "{error}");
    }

    #[test]
    fn an_older_settings_file_gains_the_chat_filter() {
        // A settings file written before D124 has no `chat_models_only`. The picker must still
        // open on chat rows, and the missing field must not cost the rest of the file.
        let old = "theme = \"dark\"\ndefault_budget_cents = 500\nunattended_default = false\n";
        let settings: Settings = toml::from_str(old).expect("an older file should still parse");
        assert!(settings.chat_models_only);
        assert_eq!(settings.default_budget_cents, 500);
    }

    #[test]
    fn cloud_publishing_is_off_in_fresh_and_older_settings() {
        assert!(
            !Settings::default().cloud_sync_sessions,
            "publishing a transcript must be opt-in"
        );
        let old = "theme = \"dark\"\ndefault_budget_cents = 500\nunattended_default = false\n";
        let settings: Settings = toml::from_str(old).expect("an older file should still parse");
        assert!(
            !settings.cloud_sync_sessions,
            "a file written before cloud sync existed must not silently turn it on"
        );
    }

    #[test]
    fn a_desktop_template_restores_a_real_pin_at_its_original_body() {
        use rigel_core::skills::{FetchedSkill, RepoSnapshot, SkillRecord};
        use rigel_core::{SessionTemplate, SkillIndex, TemplateSkill};

        const ID: &str = "github:acme/skills:pdf/SKILL.md";
        const OLD: &str = "---\nname: pdf-reader\ndescription: Read scanned PDFs.\n---\n# PDF reader\n\nAsk for the file, then summarise it.\n";
        const NEW: &str = "---\nname: pdf-reader\ndescription: Read scanned PDFs.\n---\n# PDF reader\n\nAsk for the file, wait for the scan, then summarise it.\n";

        fn snapshot(body: &str) -> RepoSnapshot {
            RepoSnapshot {
                source: "github".to_string(),
                repo: "acme/skills".to_string(),
                branch: "main".to_string(),
                stars: 0,
                license: None,
                truncated: false,
                found: 1,
                http_requests: 0,
                skills: vec![FetchedSkill {
                    record: SkillRecord {
                        id: ID.to_string(),
                        name: "pdf-reader".to_string(),
                        description: "Read scanned PDFs.".to_string(),
                        source: "github".to_string(),
                        repo: Some("acme/skills".to_string()),
                        path: Some("pdf/SKILL.md".to_string()),
                        url: None,
                        body_sha: None,
                        grade: None,
                        grade_scope: None,
                        license: None,
                        stars: None,
                        installs: None,
                        tokens: None,
                        capabilities: Vec::new(),
                        simhash: None,
                    },
                    body: body.to_string(),
                }],
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let index_root = tmp.path().join("index");
        let project_path = tmp.path().join("project");
        fs::create_dir_all(&project_path).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let index = runtime.block_on(SkillIndex::open(&index_root)).unwrap();
        runtime.block_on(index.ingest(&snapshot(OLD))).unwrap();
        let (_, old_sha) = runtime.block_on(index.body(ID)).unwrap().unwrap();

        let previous = std::env::var_os("RIGEL_INDEX_DIR");
        // SAFETY: this test is the only app test that reads `RIGEL_INDEX_DIR`, and it restores
        // the value before returning. The desktop command wrappers are the subject here.
        unsafe { std::env::set_var("RIGEL_INDEX_DIR", &index_root) };
        let project = path_string(&project_path);

        let original = session_start(project.clone(), Some("original".to_string()), None).unwrap();
        let pinned =
            session_skill_pin(project.clone(), original.id.clone(), ID.to_string(), None).unwrap();
        assert_eq!(pinned.body_sha, old_sha);

        let saved = template_save(
            project.clone(),
            SessionTemplate {
                id: String::new(),
                name: "PDF review".to_string(),
                model: None,
                effort: None,
                max_steps: None,
                agents: Vec::new(),
                skills: vec![TemplateSkill {
                    skill_id: ID.to_string(),
                    body_sha: old_sha.clone(),
                }],
                prompt: "Read the scan.".to_string(),
                created_at: 0,
            },
        )
        .unwrap();

        // The shelf moves after the template was saved. The start loop in SessionView pins each
        // template skill by its recorded hash, so the new session must read the old body.
        runtime.block_on(index.ingest(&snapshot(NEW))).unwrap();
        let started = session_start(project.clone(), Some(saved.name.clone()), None).unwrap();
        for skill in &saved.skills {
            session_skill_pin(
                project.clone(),
                started.id.clone(),
                skill.skill_id.clone(),
                Some(skill.body_sha.clone()),
            )
            .unwrap();
        }

        let pins = session_skill_pins(project.clone(), started.id.clone()).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].body_sha, old_sha);
        assert!(pins[0].changed);
        let review = session_skill_review(project, started.id, ID.to_string()).unwrap();
        assert_eq!(review.pinned_body.as_deref(), Some(OLD));
        assert_eq!(review.current_body.as_deref(), Some(NEW));

        match previous {
            Some(value) => unsafe { std::env::set_var("RIGEL_INDEX_DIR", value) },
            None => unsafe { std::env::remove_var("RIGEL_INDEX_DIR") },
        }
    }
}
