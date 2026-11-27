//! The open project and its read-only inspectors: files, search, checkpoints and exports.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::*;

/// Roadmap 122: a dropped file is capped before any byte is written to the project.
pub(crate) const ATTACHMENT_MAX_BYTES: u64 = 25 * 1024 * 1024;

struct UploadState {
    temp: PathBuf,
    name: String,
    size: u64,
    written: u64,
}

static UPLOADS: OnceLock<Mutex<HashMap<String, UploadState>>> = OnceLock::new();

fn uploads() -> &'static Mutex<HashMap<String, UploadState>> {
    UPLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn upload_key(project: &str, token: &str) -> String {
    format!("{project}\0{token}")
}

/// The token only names a temporary file; keep it to a flat, short, safe fragment.
fn upload_token_ok(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// The renderer rejects these before upload; repeat the check at the shell so a direct invoke
/// cannot write outside `.rigel/attachments`.
fn attachment_name_ok(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 || name == "." || name == ".." {
        return false;
    }
    if name.contains(['/', '\\', '\0']) || name.chars().any(|c| c.is_control()) {
        return false;
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "txt", "md", "markdown", "rs", "toml", "json", "yaml", "yml", "csv", "log", "png", "jpg",
        "jpeg", "gif", "webp", "avif", "bmp", "pdf",
    ]
    .contains(&extension.as_str())
}

fn attachment_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(".rigel").join("attachments");
    fs::create_dir_all(&dir).map_err(message)?;
    // Attachments are local context, not source: keep the directory out of the project's diff
    // even when the project does not ignore `.rigel` itself.
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        fs::write(
            &ignore,
            "# Written by Rigel. Files attached from the composer are local to this machine.\n*\n",
        )
        .map_err(message)?;
    }
    Ok(dir)
}

fn remove_upload(key: &str) -> Result<(), String> {
    if let Some(state) = uploads()
        .lock()
        .map_err(|_| "upload state poisoned".to_string())?
        .remove(key)
    {
        let _ = fs::remove_file(&state.temp);
    }
    Ok(())
}

/// Open (creating if needed) a project and report where its session database lives.
#[tauri::command]
pub(crate) fn project_open(path: String) -> Result<ProjectInfo, String> {
    tauri::async_runtime::block_on(async move {
        let root = PathBuf::from(&path);
        let store = Store::open_project(&root).await.map_err(message)?;

        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());

        remember_project(&root);
        set_graph_target(&root);
        Ok(ProjectInfo {
            root: root.to_string_lossy().into_owned(),
            name,
            db: store.path().to_string_lossy().into_owned(),
        })
    })
}

/// Create an empty folder and open it as a project. The name is a single path segment: no
/// separators, no `.`/`..`, nothing that could escape the workspace root.
#[tauri::command]
pub(crate) fn project_create(name: String) -> Result<ProjectInfo, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the workspace a name.".to_string());
    }
    if name.len() > 64
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(|c| c.is_control())
    {
        return Err("Use a short name with no path separators or reserved characters.".to_string());
    }
    let root = workspace_root().join(name);
    if root.exists() && !root.is_dir() {
        return Err(format!("{} exists and is not a folder.", root.display()));
    }
    fs::create_dir_all(&root).map_err(message)?;
    project_open(root.to_string_lossy().into_owned())
}

/// The default directory new workspaces are created under, so the UI can show it before
/// asking for a name (never a silent location).
#[tauri::command]
pub(crate) fn project_workspace_root() -> String {
    workspace_root().to_string_lossy().into_owned()
}

/// Copy files chosen in the native picker into `.rigel/attachments` and return project-relative
/// paths. `.rigel` is outside the checkpoint tree, so an attachment never makes the session look
/// dirty; the paths are what the composer adds to the next turn.
#[tauri::command]
pub(crate) fn attachment_import(
    project: String,
    paths: Vec<String>,
) -> Result<Vec<String>, String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err("Open a project folder before attaching files.".to_string());
    }
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let dir = attachment_dir(&root)?;

    let mut imported = Vec::with_capacity(paths.len());
    for source in paths {
        let source = PathBuf::from(source);
        if !source.is_file() {
            return Err(format!("{} is not a file.", source.display()));
        }
        let name = source
            .file_name()
            .ok_or_else(|| "the picked file has no name".to_string())?;
        let target = free_attachment_path(&dir, name)?;
        fs::copy(&source, &target).map_err(message)?;
        let relative = target.strip_prefix(&root).map_err(message)?;
        imported.push(relative.to_string_lossy().replace('\\', "/"));
    }
    Ok(imported)
}

/// Begin a chunked upload. The file lives at a hidden temporary path until `finish`, so a
/// cancelled or interrupted upload never leaves a visible attachment behind.
#[tauri::command]
pub(crate) fn attachment_upload_start(
    project: String,
    name: String,
    size: u64,
    token: String,
) -> Result<(), String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err("Open a project folder before attaching files.".to_string());
    }
    if !upload_token_ok(&token) {
        return Err("Invalid upload token.".to_string());
    }
    if !attachment_name_ok(&name) {
        return Err("That file type cannot be attached.".to_string());
    }
    if size == 0 {
        return Err("An empty file cannot be attached.".to_string());
    }
    if size > ATTACHMENT_MAX_BYTES {
        return Err(format!(
            "Files over {} MiB cannot be attached.",
            ATTACHMENT_MAX_BYTES / (1024 * 1024)
        ));
    }

    let key = upload_key(&project, &token);
    remove_upload(&key)?; // a retried token must not strand the earlier temporary file
    let dir = attachment_dir(&root)?;
    let temp = dir.join(format!(".upload-{token}"));
    fs::write(&temp, []).map_err(message)?;
    uploads()
        .lock()
        .map_err(|_| "upload state poisoned".to_string())?
        .insert(
            key,
            UploadState {
                temp,
                name,
                size,
                written: 0,
            },
        );
    Ok(())
}

/// Append one base64 chunk and return the number of bytes actually stored so far. The returned
/// count is the progress the renderer shows: it is bytes accepted by the shell, not a guess.
#[tauri::command]
pub(crate) fn attachment_upload_chunk(
    project: String,
    token: String,
    data: String,
) -> Result<u64, String> {
    let key = upload_key(&project, &token);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("Bad upload chunk: {error}"))?;
    let mut states = uploads()
        .lock()
        .map_err(|_| "upload state poisoned".to_string())?;
    let state = states.get_mut(&key).ok_or("Upload is no longer active.")?;
    let next = state
        .written
        .checked_add(bytes.len() as u64)
        .ok_or("Upload is too large.")?;
    if next > state.size {
        // Leave no partial file for a chunk that lied about the declared size.
        let temp = state.temp.clone();
        states.remove(&key);
        drop(states);
        let _ = fs::remove_file(temp);
        return Err("Upload received more bytes than the file contains.".to_string());
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&state.temp)
        .map_err(message)?;
    if let Err(error) = file.write_all(&bytes) {
        let temp = state.temp.clone();
        states.remove(&key);
        drop(states);
        let _ = fs::remove_file(temp);
        return Err(format!("Upload failed: {error}"));
    }
    state.written = next;
    Ok(next)
}

/// Publish a complete upload under its original name and return the project-relative path.
#[tauri::command]
pub(crate) fn attachment_upload_finish(project: String, token: String) -> Result<String, String> {
    let root = PathBuf::from(&project);
    let key = upload_key(&project, &token);
    let state = uploads()
        .lock()
        .map_err(|_| "upload state poisoned".to_string())?
        .remove(&key)
        .ok_or("Upload is no longer active.")?;
    if state.written != state.size {
        let _ = fs::remove_file(&state.temp);
        return Err("Upload ended before the whole file arrived.".to_string());
    }
    let dir = attachment_dir(&root)?;
    let target = free_attachment_path(&dir, std::ffi::OsStr::new(&state.name))?;
    if let Err(error) = fs::rename(&state.temp, &target) {
        let _ = fs::remove_file(&state.temp);
        return Err(format!("Upload failed: {error}"));
    }
    Ok(target
        .strip_prefix(&root)
        .map_err(message)?
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Cancel an upload: drop its state and delete the temporary file. Nothing reaches the visible
/// attachment directory, and no blob is left to clean up later.
#[tauri::command]
pub(crate) fn attachment_upload_cancel(project: String, token: String) -> Result<(), String> {
    remove_upload(&upload_key(&project, &token))
}

#[tauri::command]
pub(crate) async fn browser_capture(url: String) -> Result<BrowserCapture, String> {
    let parsed = reqwest::Url::parse(&url)
        .map_err(|_| "Browser captures need an absolute http or https URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Browser captures need an absolute http or https URL.".to_string());
    }
    let exe = rigel_core::agent::obscura::locate().ok_or_else(|| {
        format!(
            "Rigel b is not available. {}",
            rigel_core::agent::obscura::INSTALL_HINT
        )
    })?;
    let path = rigel_core::agent::obscura::capture_path(&index_dir(), "snapshots");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(message)?;
    }

    let mut command = tokio::process::Command::new(exe);
    command
        .arg("fetch")
        .arg(parsed.as_str())
        .arg("--screenshot")
        .arg(&path)
        .args(["--wait-until", "networkidle0", "--timeout", "30", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000);
    }

    let output = match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(format!("could not start Rigel b: {error}"));
        }
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err("Rigel b did not finish within 60 seconds".to_string());
        }
    };
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&path).await;
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(2_000)
            .collect::<String>();
        return Err(if stderr.is_empty() {
            format!("Rigel b exited with {}", output.status)
        } else {
            stderr
        });
    }

    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(format!("Rigel b returned no screenshot: {error}"));
        }
    };
    let _ = tokio::fs::remove_file(&path).await;
    if bytes.is_empty() {
        return Err("Rigel b returned an empty screenshot.".to_string());
    }
    Ok(BrowserCapture {
        image: image_data_url(&bytes),
        url: parsed.to_string(),
        bytes: bytes.len(),
    })
}

#[tauri::command]
pub(crate) async fn browser_harness_capture(
    session: String,
    agent: String,
) -> Result<BrowserHarnessCapture, String> {
    let path = rigel_core::agent::obscura::harness_capture_path(&index_dir(), &session, &agent);
    let base = fs::canonicalize(index_dir().join("browser").join("external"))
        .map_err(|_| "Rigel b has not captured a harness page yet.".to_string())?;
    let file = fs::canonicalize(&path)
        .map_err(|_| "Rigel b has not captured this page yet.".to_string())?;
    if !file.starts_with(&base) {
        return Err("That path is not a Rigel b harness capture.".to_string());
    }
    let metadata = fs::metadata(&file).map_err(message)?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("That Rigel b harness capture is not displayable.".to_string());
    }
    let bytes = tokio::fs::read(&file).await.map_err(message)?;
    let meta: serde_json::Value = fs::read_to_string(file.with_extension("json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let captured_at = meta["at"]
        .as_u64()
        .or_else(|| {
            metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_millis() as u64)
        })
        .unwrap_or_default();
    Ok(BrowserHarnessCapture {
        image: image_data_url(&bytes),
        url: meta["url"].as_str().map(str::to_string),
        tool: meta["tool"].as_str().map(str::to_string),
        captured_at,
        bytes: bytes.len(),
    })
}

/// Read a screenshot `browser_open` saved. Only paths below this machine's Rigel browser
/// directory are readable, so a transcript cannot turn the image reader into a file oracle.
#[tauri::command]
pub(crate) async fn browser_image(path: String) -> Result<String, String> {
    let base = fs::canonicalize(index_dir().join("browser"))
        .map_err(|_| "No browser capture has been saved yet.".to_string())?;
    let file = fs::canonicalize(PathBuf::from(&path))
        .map_err(|_| "That browser capture is no longer available.".to_string())?;
    if !file.starts_with(&base) {
        return Err("That path is not a Rigel browser capture.".to_string());
    }
    let metadata = fs::metadata(&file).map_err(message)?;
    if !metadata.is_file() {
        return Err("That browser capture is not a file.".to_string());
    }
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("That browser capture is too large to display.".to_string());
    }
    let bytes = tokio::fs::read(&file).await.map_err(message)?;
    Ok(image_data_url(&bytes))
}

#[tauri::command]
pub(crate) fn search_all(
    project: Option<String>,
    query: String,
    current_session: Option<String>,
) -> Result<PaletteResults, String> {
    tauri::async_runtime::block_on(async move {
        let mut skills = Vec::new();
        if !query.trim().is_empty()
            && let Ok(index) = SkillIndex::open(&index_dir()).await
            && let Ok(hits) = index.search(&query, 6).await
        {
            skills = hits
                .into_iter()
                .map(|hit| PaletteSkill {
                    id: hit.id,
                    name: hit.name,
                    description: hit.description,
                    grade: hit.grade,
                    source: hit.source,
                })
                .collect();
        }

        let workspace = match project.as_deref().filter(|root| !root.is_empty()) {
            Some(root) => {
                let service = SessionService::open(Path::new(root))
                    .await
                    .map_err(message)?;
                rigel_core::search::workspace(service.pool(), &query, current_session.as_deref(), 6)
                    .await
                    .map_err(message)?
            }
            None => rigel_core::WorkspaceHits::default(),
        };

        // Roadmap 113: files and symbols come from the same on-demand graph the Graph tab reads.
        // A graph that cannot be scanned leaves the other palette sections usable.
        let (files, symbols) = match project.as_deref().filter(|root| !root.is_empty()) {
            Some(root) => {
                let needle = query.trim().to_lowercase();
                match rigel_core::codegraph::snapshot(Path::new(root), None, 1, 700) {
                    Ok(graph) => {
                        let file_rows = graph
                            .nodes
                            .iter()
                            .filter(|node| node.kind == "file")
                            .filter(|node| {
                                needle.is_empty() || node.path.to_lowercase().contains(&needle)
                            })
                            .take(6)
                            .map(|node| PaletteFile {
                                path: node.path.clone(),
                                language: node.language.clone(),
                            })
                            .collect();
                        let symbol_rows = graph
                            .nodes
                            .iter()
                            .filter(|node| node.kind == "symbol")
                            .filter(|node| {
                                needle.is_empty() || node.label.to_lowercase().contains(&needle)
                            })
                            .take(6)
                            .map(|node| PaletteSymbol {
                                name: node.label.clone(),
                                path: node.path.clone(),
                                line: node.line,
                                language: node.language.clone(),
                            })
                            .collect();
                        (file_rows, symbol_rows)
                    }
                    Err(_) => (Vec::new(), Vec::new()),
                }
            }
            None => (Vec::new(), Vec::new()),
        };

        Ok(PaletteResults {
            skills,
            sessions: workspace.sessions,
            turns: workspace.turns,
            patches: workspace.patches,
            files,
            symbols,
        })
    })
}

/// Roadmap 116: the newest crash records for the reporter. The core already redacted each line
/// before appending it to `crash.log`; this reads that file and nothing else, and never sends it.
#[tauri::command]
pub(crate) fn crash_report() -> Result<CrashReport, String> {
    let dir = index_dir();
    Ok(CrashReport {
        path: diagnostics::crash_log_path(&dir)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "crash.log".to_string()),
        text: diagnostics::recent_crash(&dir),
    })
}

/// D82: assemble a diagnostics bundle only when the user clicks the action. It gathers the
/// doctor-shaped facts this window can check, the newest turn and the local logs; every string
/// is redacted with the provider keys before it is written, and nothing is sent anywhere.
#[tauri::command]
pub(crate) fn diagnostics_export(
    project: String,
    session: Option<String>,
) -> Result<DiagnosticsBundle, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let (integrity, schema) = diagnostics::database_health(service.pool()).await;

        let mut chosen = match session {
            Some(id) => service.get(&id).await.ok(),
            None => None,
        };
        if chosen.is_none() {
            chosen = service.list().await.ok().and_then(|mut all| {
                all.sort_by_key(|session| std::cmp::Reverse(session.created_at));
                all.into_iter().next()
            });
        }
        let last_turn: Option<TurnTranscript> = match &chosen {
            Some(session) => service
                .transcript(&session.id)
                .await
                .ok()
                .and_then(|mut turns| turns.pop()),
            None => None,
        };

        let data_dir = index_dir();
        let keys: serde_json::Map<String, serde_json::Value> = credentials::PROVIDERS
            .iter()
            .map(|provider| {
                (
                    (*provider).to_string(),
                    serde_json::json!(credentials::present(provider)),
                )
            })
            .collect();
        let doctor = serde_json::json!({
            "app": {
                "version": env!("CARGO_PKG_VERSION"),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            },
            "project": root.display().to_string(),
            "database": {
                "integrity": integrity,
                "schema": schema.iter().map(|(version, name)| serde_json::json!({
                    "version": version, "name": name
                })).collect::<Vec<_>>(),
            },
            "keychain": credentials::available().is_ok(),
            "keys_present": keys,
            "crash_log": diagnostics::crash_log_path(&data_dir).display().to_string(),
        });

        let mut log = diagnostics::recent_crash(&data_dir);
        if let Some(tail) = diagnostics::log_tail(&data_dir.join("logs/rigel.log"), 64 * 1024) {
            if !log.is_empty() {
                log.push('\n');
            }
            log.push_str(&tail);
        }
        if log.is_empty() {
            log.push_str("No local log or crash record has been written yet.");
        }

        let secrets = diagnostics::secrets();
        let body = diagnostics::assemble(&doctor, last_turn.as_ref(), &log, &secrets);
        let path = diagnostics::write_bundle(&data_dir, &body).map_err(message)?;
        Ok(DiagnosticsBundle {
            path: path.display().to_string(),
            body,
        })
    })
}

/// D91: which rule files a turn would read, for Settings. Read-only; no file is changed.
#[tauri::command]
pub(crate) fn rules_sources(project: String) -> Result<Vec<rigel_core::rules::RuleSource>, String> {
    let root = std::fs::canonicalize(&project).map_err(message)?;
    Ok(rigel_core::rules::load(&root).sources)
}

/// D91: the committed memory rendered as one copyable `AGENTS.md` block.
#[tauri::command]
pub(crate) fn rules_export_memory(project: String) -> Result<String, String> {
    let root = std::fs::canonicalize(&project).map_err(message)?;
    Ok(rigel_core::rules::export_memory(&root))
}

/// D97: the exact title and body the next click would send to gh.
#[tauri::command]
pub(crate) fn pull_request_plan(
    project: String,
    session: String,
) -> Result<rigel_core::forge::PullRequestPlan, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let session = service.get(&session).await.map_err(message)?;
        let turns = service.transcript(&session.id).await.map_err(message)?;
        rigel_core::forge::Forge::plan(
            &rigel_core::forge::GitHubForge::default(),
            &root,
            &session,
            &turns,
        )
        .map_err(message)
    })
}

/// D97: the explicit click. gh authenticates itself; Rigel never sees a forge token.
#[tauri::command]
pub(crate) fn pull_request_create(project: String, session: String) -> Result<String, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let session = service.get(&session).await.map_err(message)?;
        let turns = service.transcript(&session.id).await.map_err(message)?;
        let plan = rigel_core::forge::Forge::plan(
            &rigel_core::forge::GitHubForge::default(),
            &root,
            &session,
            &turns,
        )
        .map_err(message)?;
        rigel_core::forge::Forge::create(&rigel_core::forge::GitHubForge::default(), &root, &plan)
            .map_err(message)
    })
}

#[tauri::command]
pub(crate) fn checkpoint_list(
    project: String,
    session: String,
) -> Result<Vec<rigel_core::Checkpoint>, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        SessionService::open(&root)
            .await
            .map_err(message)?
            .checkpoints(&session)
            .await
            .map_err(message)
    })
}

/// D89: put the tree back to the snapshot a turn recorded. Explicit only; the preview and the
/// journal stay available, so this is a reviewed restore rather than a rollback of history.
#[tauri::command]
pub(crate) fn checkpoint_restore(
    project: String,
    id: String,
) -> Result<rigel_core::snapshot::RestoreReport, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let journal = service
            .checkpoint_journal(&id)
            .await
            .map_err(message)?
            .ok_or_else(|| "checkpoint not found".to_string())?;
        let snapshot = journal
            .get("snapshot_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                "this checkpoint predates per-turn snapshots; resume it instead".to_string()
            })?;
        SnapshotService::new(&root, root.join(".rigel"))
            .restore(snapshot, &root)
            .map_err(message)
    })
}

/// D95: one Markdown bundle per session, with the JSON beside it. The same redaction path as
/// diagnostics; writes under the app data folder so a session's machine paths never land in a
/// project that might be committed.
#[tauri::command]
pub(crate) fn session_export(project: String, id: String) -> Result<SessionExport, String> {
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        let session = service.get(&id).await.map_err(message)?;
        let agents = service.agents(&id).await.map_err(message)?;
        let turns = service.transcript(&id).await.map_err(message)?;
        let patches = PatchPipeline::new(&root, service.pool().clone())
            .list(&id)
            .await
            .unwrap_or_default();
        let bookmarks = service.bookmarks(&id).await.unwrap_or_default();
        let (markdown, json) = diagnostics::session_bundle(
            &session,
            &agents,
            &turns,
            &patches,
            &bookmarks,
            &diagnostics::secrets(),
        );
        let (md_path, json_path) =
            diagnostics::write_session_bundle(&index_dir(), &id, &markdown, &json)
                .map_err(message)?;
        Ok(SessionExport {
            markdown_path: md_path.display().to_string(),
            json_path: json_path.display().to_string(),
            markdown,
        })
    })
}

/// Import Claude or Codex JSONL, returning the complete row-by-row dry-run report before writes.
#[tauri::command]
pub(crate) fn session_import(
    project: String,
    content: String,
    source: String,
    dry_run: bool,
    title: Option<String>,
) -> Result<rigel_core::import::ImportReport, String> {
    let source = match source.as_str() {
        "claude" => rigel_core::import::HistorySource::Claude,
        "codex" => rigel_core::import::HistorySource::Codex,
        _ => return Err("Choose a Claude or Codex history source.".to_string()),
    };
    if dry_run {
        return Ok(rigel_core::import::dry_run(&content, source));
    }
    tauri::async_runtime::block_on(async move {
        let root = std::fs::canonicalize(&project).map_err(message)?;
        let service = SessionService::open(&root).await.map_err(message)?;
        rigel_core::import::import_history(&service, &content, source, title.as_deref())
            .await
            .map_err(message)
    })
}

/// Q268: run the one test command Rigel can recognise for this project. The action is explicit,
/// local, and outside the agent loop; the core bounds the captured output.
#[tauri::command]
pub(crate) async fn tests_run(project: String) -> Result<rigel_core::testrun::TestRun, String> {
    let root = PathBuf::from(project);
    if !root.is_dir() {
        return Err("That project folder is not available.".to_string());
    }
    rigel_core::testrun::run(&root).await.map_err(message)
}
