//! Deferred self-replace: the helper that finishes an update after the app exits.
//!
//! Windows holds an image section on a running executable, so an app cannot overwrite the file it
//! was started from. The app downloads and verifies the replacement, writes an [`ApplyPlan`], and
//! starts this same binary with [`HELPER_FLAG`] and the plan path. That helper process is a plain
//! console process with no window and no Tauri state: it waits for the app's PID to disappear,
//! swaps the binary through a staged copy, and starts the new build with the original arguments
//! and working directory.
//!
//! The swap is reversible at every step. The old binary is renamed to `<name>.old` before the new
//! one is moved into place; if the move or the relaunch fails, the backup is restored and the
//! error is written to `updates/apply.log`. A half-swapped install is never left behind.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::UpdateError;

/// The first argument that turns this binary into the deferred-replace helper.
pub const HELPER_FLAG: &str = "--rigel-apply-update";
/// The plan and its log both live under `<data-dir>/updates`.
const PLAN_FILE: &str = "apply.json";
const LOG_FILE: &str = "apply.log";
/// A helper never waits forever: an app that stays open leaves the update staged for next launch.
const WAIT_LIMIT: Duration = Duration::from_secs(300);
/// The environment variable the new build sees once. It suppresses the launch check for that one
/// start, so a manifest that is wrong about its own binary cannot cause a relaunch loop.
pub const UPDATED_ENV: &str = "RIGEL_UPDATED_TO";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyPlan {
    /// The process that must exit before the target can be replaced.
    pub pid: u32,
    /// The downloaded, hash-verified replacement.
    pub source: PathBuf,
    /// The running executable to replace.
    pub target: PathBuf,
    /// The SHA-256 the source was verified against before the plan was written.
    pub sha256: String,
    /// The arguments the new build is started with, unchanged.
    #[serde(default)]
    pub args: Vec<String>,
    /// The working directory the new build is started in, unchanged.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The version being applied, for the log and the one-launch guard.
    pub version: String,
}

/// The helper's plan path, from the arguments of this process. `None` means "not a helper".
pub fn helper_request<I>(args: I) -> Option<PathBuf>
where
    I: IntoIterator,
    I::Item: AsRef<std::ffi::OsStr>,
{
    let mut args = args.into_iter();
    if args.next()?.as_ref() != std::ffi::OsStr::new(HELPER_FLAG) {
        return None;
    }
    args.next().map(|path| PathBuf::from(path.as_ref()))
}

/// Write the plan atomically beside the downloaded artifact.
pub fn write_plan(data_dir: &Path, plan: &ApplyPlan) -> Result<PathBuf, UpdateError> {
    let dir = data_dir.join(super::UPDATE_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(PLAN_FILE);
    let tmp = dir.join("apply.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(plan)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Start this same executable as a detached helper. The child shares nothing with the app except
/// the plan path, so it outlives the window and its exit.
pub fn spawn_helper(plan: &Path) -> Result<(), UpdateError> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command
        .arg(HELPER_FLAG)
        .arg(plan)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::hide(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| UpdateError::Install(error.to_string()))
}

/// Append one line to `updates/apply.log` (or `update.log` in the data dir). Logging is
/// best-effort: a full disk must not turn an update failure into a second failure.
pub fn log_at(dir: &Path, line: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(LOG_FILE);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Run the helper to completion. Called only from the binary's early-start helper branch.
pub fn run_helper(plan_path: &Path) -> Result<(), UpdateError> {
    let dir = plan_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let text = std::fs::read_to_string(plan_path)?;
    let plan: ApplyPlan = serde_json::from_str(&text)?;
    log_at(
        &dir,
        &format!("apply {}: waiting for pid {}", plan.version, plan.pid),
    );
    if let Err(error) = apply(&dir, &plan) {
        log_at(&dir, &format!("apply {}: failed: {error}", plan.version));
        return Err(error);
    }
    Ok(())
}

fn apply(dir: &Path, plan: &ApplyPlan) -> Result<(), UpdateError> {
    verify_source(&plan.source, &plan.sha256)?;
    wait_for_exit(plan.pid);
    let backup = swap(&plan.source, &plan.target, &plan.sha256)?;
    log_at(
        dir,
        &format!("apply {}: swapped {}", plan.version, plan.target.display()),
    );

    let mut command = Command::new(&plan.target);
    command.args(&plan.args);
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    command
        .env(UPDATED_ENV, &plan.version)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process::hide(&mut command);
    match command.spawn() {
        Ok(_) => {
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::remove_file(&plan.source);
            log_at(
                dir,
                &format!("apply {}: started {}", plan.version, plan.target.display()),
            );
            Ok(())
        }
        Err(error) => {
            // Put the working install back before returning the error.
            let _ = std::fs::remove_file(&plan.target);
            let _ = std::fs::rename(&backup, &plan.target);
            Err(UpdateError::Install(error.to_string()))
        }
    }
}

/// Wait for a process to exit, bounded. A PID that is already gone returns immediately, which is
/// the normal path when the app exits right after spawning the helper.
fn wait_for_exit(pid: u32) {
    let deadline = Instant::now() + WAIT_LIMIT;
    while process_alive(pid) {
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn process_alive(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    system.process(sysinfo::Pid::from_u32(pid)).is_some()
}

fn verify_source(source: &Path, expected: &str) -> Result<(), UpdateError> {
    if !source.is_file() {
        return Err(UpdateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("the staged update is missing: {}", source.display()),
        )));
    }
    let found = hash_file(source)?;
    if !found.eq_ignore_ascii_case(expected) {
        return Err(UpdateError::HashMismatch {
            expected: expected.to_string(),
            found,
        });
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, UpdateError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Move the replacement into place, keeping a restorable backup until the new build is running.
fn swap(source: &Path, target: &Path, expected: &str) -> Result<PathBuf, UpdateError> {
    let dir = target.parent().ok_or_else(|| {
        UpdateError::Install(format!("{} has no parent directory", target.display()))
    })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| UpdateError::Install(format!("{} has no file name", target.display())))?;
    let staged = dir.join(format!("{name}.new"));
    let backup = dir.join(format!("{name}.old"));

    let _ = std::fs::remove_file(&staged);
    std::fs::copy(source, &staged)?;
    let copied = hash_file(&staged)?;
    if !copied.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&staged);
        return Err(UpdateError::HashMismatch {
            expected: expected.to_string(),
            found: copied,
        });
    }

    let _ = std::fs::remove_file(&backup);
    std::fs::rename(target, &backup)?;
    if let Err(error) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&backup, target);
        let _ = std::fs::remove_file(&staged);
        return Err(error.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
    }
    Ok(backup)
}

/// Whether this process can replace its own executable. A per-machine install under Program Files
/// is not writable by the user, so the app falls back to the signed installer instead of staging a
/// replacement it could never apply.
pub fn can_replace(target: &Path) -> bool {
    let Some(dir) = target.parent() else {
        return false;
    };
    let probe = dir.join(".rigel-update-write-test");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_flag_is_recognised_and_nothing_else_is() {
        let args = vec![HELPER_FLAG.to_string(), "plan.json".to_string()];
        assert_eq!(helper_request(args), Some(PathBuf::from("plan.json")));
        assert_eq!(helper_request(vec!["x".to_string()]), None);
        assert_eq!(helper_request(vec![HELPER_FLAG.to_string()]), None);
    }

    #[test]
    fn a_plan_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let plan = ApplyPlan {
            pid: 42,
            source: dir.path().join("new.exe"),
            target: dir.path().join("rigel-app.exe"),
            sha256: "a".repeat(64),
            args: vec!["--profile".into(), "work".into()],
            cwd: Some(dir.path().to_path_buf()),
            version: "0.4.0".into(),
        };
        let path = write_plan(dir.path(), &plan).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        let parsed: ApplyPlan = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn a_source_with_the_wrong_hash_is_refused_before_the_target_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.exe");
        std::fs::write(&source, b"new bytes").unwrap();
        let error = verify_source(&source, &"f".repeat(64)).unwrap_err();
        assert!(matches!(error, UpdateError::HashMismatch { .. }));
    }

    #[test]
    fn a_missing_source_is_an_error_not_a_swap() {
        let dir = tempfile::tempdir().unwrap();
        let error = verify_source(&dir.path().join("absent.exe"), &"a".repeat(64)).unwrap_err();
        assert!(matches!(error, UpdateError::Io(_)));
    }

    #[test]
    fn the_swap_keeps_a_backup_until_it_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rigel-app.exe");
        let source = dir.path().join("downloaded.exe");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&source, b"new").unwrap();
        let sha = format!("{:x}", Sha256::digest(b"new"));

        let backup = swap(&source, &target, &sha).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        // The downloaded source stays until the relaunch succeeds, so a failed start can retry.
        assert!(source.exists());
        assert!(!dir.path().join("rigel-app.exe.new").exists());
    }

    #[test]
    fn a_bad_staged_copy_never_replaces_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rigel-app.exe");
        let source = dir.path().join("downloaded.exe");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&source, b"new").unwrap();

        let error = swap(&source, &target, &"f".repeat(64)).unwrap_err();
        assert!(matches!(error, UpdateError::HashMismatch { .. }));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn only_a_writable_directory_can_replace_its_binary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(can_replace(&dir.path().join("rigel-app.exe")));
        assert!(!can_replace(Path::new("")));
    }
}
