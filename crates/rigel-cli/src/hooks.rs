//! Roadmap 156: optional git hooks.
//!
//! `rigel hooks install` writes three hooks into a project's git directory:
//!
//! - `pre-commit` runs `rigel hooks scan`, which scans the staged diff with the same secret
//!   scanner Rigel uses before a write. A finding names the rule and line, never the secret,
//!   and blocks the commit.
//! - `prepare-commit-msg` runs `rigel hooks trailer`, which appends one `rigel: <agent>` trailer
//!   so a commit names who produced it.
//! - `commit-msg` runs `rigel hooks check-msg`, the attribution check: a message without one
//!   well-formed `rigel` trailer is refused.
//!
//! Installation is additive and reversible. Before a hook is replaced its exact bytes and Unix
//! mode are copied into `.rigel/hooks/`; `uninstall` restores those bytes or removes the file
//! when no hook existed. Uninstall refuses when a hook changed after install, so it never
//! overwrites a user's edit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use rigel_core::agent::secret_scan::{Finding, WriteScan, scan_staged_diff};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_VERSION: u32 = 1;
pub const TRAILER_TOKEN: &str = "rigel";
const SCAN_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    PreCommit,
    PrepareCommitMsg,
    CommitMsg,
}

impl Hook {
    pub const ALL: [Hook; 3] = [Hook::PreCommit, Hook::PrepareCommitMsg, Hook::CommitMsg];

    pub fn name(self) -> &'static str {
        match self {
            Hook::PreCommit => "pre-commit",
            Hook::PrepareCommitMsg => "prepare-commit-msg",
            Hook::CommitMsg => "commit-msg",
        }
    }

    /// The exact script installed for this hook. Deterministic, so uninstall can verify that
    /// the file on disk is still the one we wrote.
    pub fn script(self, project: &Path) -> String {
        let banner = format!(
            "# Installed by rigel (roadmap 156). Remove with: rigel hooks uninstall --project {}",
            shell_quote(&project.to_string_lossy())
        );
        let body = match self {
            Hook::PreCommit => {
                format!(
                    "exec rigel hooks scan --project {}",
                    shell_quote(&project.to_string_lossy())
                )
            }
            Hook::PrepareCommitMsg => "exec rigel hooks trailer --file \"$1\"".to_string(),
            Hook::CommitMsg => "exec rigel hooks check-msg --file \"$1\"".to_string(),
        };
        format!("#!/bin/sh\n{banner}\n{body}\n")
    }
}

/// Single-quote a path for a POSIX shell without letting a quote break out of the string.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Resolve the hooks directory, including linked worktrees whose `.git` is a `gitdir:` file.
fn git_hooks_dir(project: &Path) -> Result<PathBuf, String> {
    let dot_git = project.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git.join("hooks"));
    }
    if dot_git.is_file() {
        let text = fs::read_to_string(&dot_git)
            .map_err(|error| format!("could not read {}: {error}", dot_git.display()))?;
        let Some(target) = text.strip_prefix("gitdir:") else {
            return Err(format!("{} is not a gitdir pointer", dot_git.display()));
        };
        let gitdir = PathBuf::from(target.trim());
        let gitdir = if gitdir.is_absolute() {
            gitdir
        } else {
            project.join(gitdir)
        };
        return Ok(gitdir.join("hooks"));
    }
    Err(format!("{} is not a git work tree", project.display()))
}

fn manifest_path(project: &Path) -> PathBuf {
    project.join(".rigel").join("hooks.json")
}

fn backup_dir(project: &Path) -> PathBuf {
    project.join(".rigel").join("hooks")
}

fn backup_path(project: &Path, hook: Hook) -> PathBuf {
    backup_dir(project).join(format!("{}.orig", hook.name()))
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|meta| meta.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Option<u32> {
    None
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("could not stat {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("could not make {} executable: {error}", path.display()))
    }
    #[cfg(not(unix))]
    {
        Ok(())
    }
}

fn restore_mode(path: &Path, original: Option<u32>) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(mode) = original {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
            format!("could not restore the mode of {}: {error}", path.display())
        })?;
    }
    #[cfg(not(unix))]
    let _ = (path, original);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookRecord {
    /// Whether a hook was present before installation.
    pub existed: bool,
    /// Hash of the bytes we wrote, so uninstall can detect an edit by the user.
    pub installed_sha256: String,
    /// Original Unix mode, restored byte for byte along with the content.
    pub original_mode: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: u32,
    pub hooks: BTreeMap<String, HookRecord>,
}

fn read_manifest(project: &Path) -> Result<Manifest, String> {
    let path = manifest_path(project);
    let text = fs::read_to_string(&path)
        .map_err(|_| "no rigel hooks are installed in this project".to_string())?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a valid hooks manifest: {error}", path.display()))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(format!(
            "unsupported hooks manifest version {} (expected {})",
            manifest.version, MANIFEST_VERSION
        ));
    }
    Ok(manifest)
}

fn write_manifest(project: &Path, manifest: &Manifest) -> Result<(), String> {
    let path = manifest_path(project);
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("could not encode the hooks manifest: {error}"))?;
    fs::create_dir_all(path.parent().expect("manifest has a parent"))
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    fs::write(&path, format!("{text}\n"))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub backed_up: Vec<String>,
}

/// Install the hook suite, backing up every hook that already existed.
pub fn install(project: &Path) -> Result<InstallReport, String> {
    if manifest_path(project).exists() {
        return Err("rigel hooks are already installed here; uninstall first".to_string());
    }
    let hooks = git_hooks_dir(project)?;
    fs::create_dir_all(&hooks)
        .map_err(|error| format!("could not create {}: {error}", hooks.display()))?;
    fs::create_dir_all(backup_dir(project))
        .map_err(|error| format!("could not create the hook backup directory: {error}"))?;

    let mut manifest = Manifest {
        version: MANIFEST_VERSION,
        hooks: BTreeMap::new(),
    };
    let mut installed = Vec::new();
    let mut backed_up = Vec::new();

    for hook in Hook::ALL {
        let path = hooks.join(hook.name());
        let existed = path.exists();
        let original_mode = existed.then(|| file_mode(&path)).flatten();
        let script = hook.script(project);
        let bytes = script.as_bytes();

        if existed {
            let original = fs::read(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            fs::write(backup_path(project, hook), &original)
                .map_err(|error| format!("could not back up {}: {error}", path.display()))?;
            backed_up.push(hook.name().to_string());
        }
        fs::write(&path, bytes)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        set_executable(&path)?;
        manifest.hooks.insert(
            hook.name().to_string(),
            HookRecord {
                existed,
                installed_sha256: digest(bytes),
                original_mode,
            },
        );
        installed.push(hook.name().to_string());
    }

    write_manifest(project, &manifest)?;
    Ok(InstallReport {
        installed,
        backed_up,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UninstallReport {
    pub restored: Vec<String>,
    pub removed: Vec<String>,
}

/// Restore the exact bytes that were present before install, or remove hooks we added.
pub fn uninstall(project: &Path) -> Result<UninstallReport, String> {
    let hooks = git_hooks_dir(project)?;
    let manifest = read_manifest(project)?;
    let mut restored = Vec::new();
    let mut removed = Vec::new();

    for hook in Hook::ALL {
        let Some(record) = manifest.hooks.get(hook.name()) else {
            continue;
        };
        let path = hooks.join(hook.name());
        let current = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(format!("could not read {}: {error}", path.display()));
            }
        };
        if digest(&current) != record.installed_sha256 {
            return Err(format!(
                "{} changed after install; restore it by hand instead of uninstalling",
                path.display()
            ));
        }

        if record.existed {
            let original = fs::read(backup_path(project, hook)).map_err(|error| {
                format!("could not read the backup for {}: {error}", hook.name())
            })?;
            fs::write(&path, &original)
                .map_err(|error| format!("could not restore {}: {error}", path.display()))?;
            restore_mode(&path, record.original_mode)?;
            restored.push(hook.name().to_string());
        } else {
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
            }
            removed.push(hook.name().to_string());
        }
    }

    fs::remove_file(manifest_path(project))
        .map_err(|error| format!("could not remove the hooks manifest: {error}"))?;
    let _ = fs::remove_dir_all(backup_dir(project));
    Ok(UninstallReport { restored, removed })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HookStatus {
    pub name: String,
    pub installed: bool,
    /// True when the hook is present but no longer matches what install wrote.
    pub modified: bool,
    /// True when a pre-existing copy is kept for uninstall.
    pub backed_up: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatusReport {
    pub installed: bool,
    pub hooks: Vec<HookStatus>,
}

pub fn status(project: &Path) -> Result<StatusReport, String> {
    let hooks = git_hooks_dir(project)?;
    let manifest = read_manifest(project).ok();
    let rows = Hook::ALL
        .iter()
        .map(|hook| {
            let record = manifest
                .as_ref()
                .and_then(|manifest| manifest.hooks.get(hook.name()));
            let current = fs::read(hooks.join(hook.name())).unwrap_or_default();
            HookStatus {
                name: hook.name().to_string(),
                installed: record.is_some(),
                modified: record.is_some_and(|record| digest(&current) != record.installed_sha256),
                backed_up: record.is_some_and(|record| record.existed),
            }
        })
        .collect();
    Ok(StatusReport {
        installed: manifest.is_some(),
        hooks: rows,
    })
}

/// The staged diff, from git itself. Only added lines are later considered by the scanner.
fn staged_diff(project: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["diff", "--cached", "--no-ext-diff", "--unified=0"])
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Findings in a staged diff. The scanner already ignores removed lines and never returns the
/// matched secret text.
pub fn findings(diff: &str) -> Vec<Finding> {
    match scan_staged_diff(diff) {
        WriteScan::Clean => Vec::new(),
        WriteScan::Blocked(findings) => findings,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScanReport {
    pub schema: u32,
    pub clean: bool,
    pub findings: Vec<Finding>,
}

/// The pre-commit body: scan the staged diff and refuse a commit that adds a secret.
pub fn scan(project: &Path, as_json: bool) -> ExitCode {
    let diff = match staged_diff(project) {
        Ok(diff) => diff,
        Err(error) => {
            eprintln!("rigel hooks scan: {error}");
            return ExitCode::from(2);
        }
    };
    let found = findings(&diff);
    let report = ScanReport {
        schema: SCAN_SCHEMA,
        clean: found.is_empty(),
        findings: found.clone(),
    };
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else if found.is_empty() {
        println!("staged diff is clean — no secret pattern on an added line");
    } else {
        for finding in &found {
            println!("blocked: {}", finding.sentence());
        }
        println!(
            "{} finding(s) — the matched text is deliberately not printed",
            found.len()
        );
    }
    if found.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Reduce an agent label to a single safe trailer value: no newline, no colon, no whitespace.
pub fn sanitize_agent(agent: &str) -> String {
    let cleaned: String = agent
        .chars()
        .map(|character| {
            if character.is_control() || character.is_whitespace() || character == ':' {
                '-'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned.to_string()
    }
}

fn trailer_line(agent: &str) -> String {
    format!("{TRAILER_TOKEN}: {}", sanitize_agent(agent))
}

/// The value of a well-formed `rigel:` trailer in the message's trailer block, if present.
fn trailer_value(message: &str) -> Option<&str> {
    message
        .lines()
        .rev()
        .take_while(|line| !line.trim().is_empty())
        .find_map(|line| {
            let (token, value) = line.split_once(':')?;
            if token.trim().eq_ignore_ascii_case(TRAILER_TOKEN) {
                Some(value.trim())
            } else {
                None
            }
        })
}

pub fn has_trailer(message: &str) -> bool {
    trailer_value(message).is_some_and(|value| !value.is_empty())
}

/// Add exactly one trailer, preserving the message and its existing trailers byte for byte.
pub fn append_trailer(message: &str, agent: &str) -> String {
    if has_trailer(message) {
        return message.to_string();
    }
    let trimmed = message.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        format!("{}\n", trailer_line(agent))
    } else {
        format!("{trimmed}\n\n{}\n", trailer_line(agent))
    }
}

/// The attribution check: every commit must name the agent that produced it.
pub fn attribution_check(message: &str) -> Result<(), String> {
    if has_trailer(message) {
        Ok(())
    } else {
        Err(format!(
            "commit message has no `{TRAILER_TOKEN}: <agent>` trailer"
        ))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrailerReport {
    pub added: bool,
    pub trailer: String,
    pub file: String,
}

fn read_message(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read the commit message {}: {error}",
            path.display()
        )
    })
}

/// The prepare-commit-msg body: add the trailer once, leaving an existing one alone.
pub fn trailer(path: &Path, agent: &str, as_json: bool) -> ExitCode {
    let message = match read_message(path) {
        Ok(message) => message,
        Err(error) => {
            eprintln!("rigel hooks trailer: {error}");
            return ExitCode::from(2);
        }
    };
    let updated = append_trailer(&message, agent);
    let added = updated != message;
    if added && let Err(error) = fs::write(path, &updated) {
        eprintln!(
            "rigel hooks trailer: could not write {}: {error}",
            path.display()
        );
        return ExitCode::from(2);
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&TrailerReport {
                added,
                trailer: trailer_line(agent),
                file: path.to_string_lossy().into_owned(),
            })
            .unwrap_or_default()
        );
    } else if added {
        println!("added {}", trailer_line(agent));
    }
    ExitCode::SUCCESS
}

/// The commit-msg body: refuse a message that is not attributed.
pub fn check_msg(path: &Path, as_json: bool) -> ExitCode {
    let message = match read_message(path) {
        Ok(message) => message,
        Err(error) => {
            eprintln!("rigel hooks check-msg: {error}");
            return ExitCode::from(2);
        }
    };
    match attribution_check(&message) {
        Ok(()) => {
            if as_json {
                println!("{}", serde_json::json!({ "attributed": true }));
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            if as_json {
                println!(
                    "{}",
                    serde_json::json!({ "attributed": false, "error": error })
                );
            } else {
                eprintln!("rigel hooks check-msg: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, clap::Subcommand)]
pub enum HooksCommand {
    /// Install the hook suite into this project's git directory.
    Install {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Restore the hooks that were present before install, or remove ours.
    Uninstall {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show which hooks are installed and whether they still match.
    Status {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Scan the staged diff for secrets (the pre-commit body).
    Scan {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Append the `rigel` trailer (the prepare-commit-msg body).
    Trailer {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        #[arg(long, value_name = "AGENT")]
        agent: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Check that a commit message is attributed (the commit-msg body).
    CheckMsg {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

fn fail(as_json: bool, error: String, json: impl FnOnce(String) -> serde_json::Value) -> ExitCode {
    if as_json {
        println!("{}", json(error));
    } else {
        eprintln!("rigel hooks: {error}");
    }
    ExitCode::from(2)
}

pub fn run(command: HooksCommand) -> ExitCode {
    match command {
        HooksCommand::Install { project, json } => match install(&project) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                } else {
                    println!(
                        "installed {} hook(s); backed up {}",
                        report.installed.len(),
                        if report.backed_up.is_empty() {
                            "none".to_string()
                        } else {
                            report.backed_up.join(", ")
                        }
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(
                json,
                error,
                |error| serde_json::json!({ "installed": false, "error": error }),
            ),
        },
        HooksCommand::Uninstall { project, json } => match uninstall(&project) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                } else {
                    println!(
                        "restored {} hook(s); removed {}",
                        report.restored.len(),
                        report.removed.len()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(
                json,
                error,
                |error| serde_json::json!({ "uninstalled": false, "error": error }),
            ),
        },
        HooksCommand::Status { project, json } => match status(&project) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                } else if report.installed {
                    for hook in &report.hooks {
                        println!(
                            "  {:<20} {}{}",
                            hook.name,
                            if hook.installed {
                                "installed"
                            } else {
                                "not installed"
                            },
                            if hook.modified {
                                " (modified since install)"
                            } else if hook.backed_up {
                                " (previous hook backed up)"
                            } else {
                                ""
                            }
                        );
                    }
                } else {
                    println!("no rigel hooks installed in {}", project.display());
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(
                json,
                error,
                |error| serde_json::json!({ "installed": false, "error": error }),
            ),
        },
        HooksCommand::Scan { project, json } => scan(&project, json),
        HooksCommand::Trailer { file, agent, json } => {
            let agent = agent.unwrap_or_else(|| {
                std::env::var("RIGEL_AGENT_ID").unwrap_or_else(|_| "cli".to_string())
            });
            trailer(&file, &agent, json)
        }
        HooksCommand::CheckMsg { file, json } => check_msg(&file, json),
    }
}

#[cfg(test)]
mod tests {
    use rigel_core::agent::secret_scan::Rule;

    use super::*;

    fn project() -> PathBuf {
        PathBuf::from(r"C:\repo's project")
    }

    #[test]
    fn shell_quote_keeps_a_quote_inside_one_argument() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("repo's"), "'repo'\\''s'");
    }

    #[test]
    fn scripts_call_the_right_body_and_are_deterministic() {
        let dir = project();
        for (hook, needle) in [
            (
                Hook::PreCommit,
                "exec rigel hooks scan --project 'C:\\repo'\\''s project'",
            ),
            (
                Hook::PrepareCommitMsg,
                "exec rigel hooks trailer --file \"$1\"",
            ),
            (Hook::CommitMsg, "exec rigel hooks check-msg --file \"$1\""),
        ] {
            let script = hook.script(&dir);
            assert!(script.starts_with("#!/bin/sh\n"), "{script}");
            assert!(script.contains(needle), "{script}");
            assert!(script.ends_with('\n'));
            assert_eq!(script, hook.script(&dir), "script must be stable");
        }
    }

    #[test]
    fn digests_are_stable_and_see_one_byte() {
        assert_eq!(digest(b"abc"), digest(b"abc"));
        assert_ne!(digest(b"abc"), digest(b"abd"));
    }

    #[test]
    fn a_manifest_round_trips_without_losing_a_record() {
        let record = HookRecord {
            existed: true,
            installed_sha256: digest(b"script"),
            original_mode: Some(0o100644),
        };
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            hooks: BTreeMap::from([("pre-commit".to_string(), record.clone())]),
        };
        let text = serde_json::to_string(&manifest).unwrap();
        let decoded: Manifest = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.hooks["pre-commit"], record);
    }

    #[test]
    fn an_agent_label_cannot_break_out_of_the_trailer() {
        assert_eq!(
            sanitize_agent("line cook\nrigel: evil"),
            "line-cook-rigel--evil"
        );
        assert_eq!(sanitize_agent("  "), "unknown");
        assert_eq!(sanitize_agent("agent:1"), "agent-1");
    }

    #[test]
    fn a_trailer_is_added_once_and_the_message_is_otherwise_untouched() {
        let message = "fix the parser\n\ncoach: sous chef\n";
        let once = append_trailer(message, "line cook");
        assert_eq!(
            once,
            "fix the parser\n\ncoach: sous chef\n\nrigel: line-cook\n"
        );
        assert_eq!(append_trailer(&once, "someone else"), once, "idempotent");
        assert!(has_trailer(&once));
    }

    #[test]
    fn an_empty_subject_gets_only_the_trailer() {
        assert_eq!(append_trailer("\n\n", "agent"), "rigel: agent\n");
    }

    #[test]
    fn an_empty_or_malformed_trailer_does_not_count_as_attribution() {
        assert!(!has_trailer("fix bug\n\nrigel:\n"));
        assert!(!has_trailer("fix bug"));
        assert!(attribution_check("fix bug\n\nrigel:\n").is_err());
        assert!(attribution_check("fix bug\n\nrigel: line-cook\n").is_ok());
        assert!(
            attribution_check("fix bug\n\nRIGEL: line-cook\n").is_ok(),
            "the token is case-insensitive"
        );
    }

    #[test]
    fn only_added_secret_lines_block_and_findings_never_carry_the_secret() {
        let diff = "\
diff --git a/.env b/.env
--- a/.env
+++ b/.env
@@ -1 +1,2 @@
-AKIAIOSFODNN7EXAMPLE
+safe text
+AKIAIOSFODNN7EXAMPLE
";
        let found = findings(diff);
        assert_eq!(found.len(), 1, "a removed secret is not a new secret");
        assert_eq!(found[0].rule, Rule::AwsAccessKey);
        assert!(found[0].sentence().contains("line"));
        assert!(!found[0].sentence().contains("AKIA"));
        assert!(findings("+just a normal line\n").is_empty());
    }
}
