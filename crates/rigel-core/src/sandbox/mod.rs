//! Sandbox profiles for commands that leave the editor.
//!
//! The profiles are deliberately described by what they do *not* contain as well as what they
//! do. A Job Object is a lifetime/resource boundary, not a filesystem or network sandbox; the
//! macOS and Linux profiles wrap the command in the platform tool and state which writes,
//! processes and sockets remain reachable.
//!
//! The platform wrappers are selected explicitly. [`SandboxProfile::None`] is the default so an
//! existing tool set keeps its old behaviour; a caller that wants a boundary installs one with
//! [`crate::agent::Tools::with_sandbox`]. A requested profile that is unavailable on the host is
//! an error, never a silent unsandboxed fallback.

use std::path::{Path, PathBuf};

/// Windows Job Object limits. `None` leaves that resource unbounded; the job always uses
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobLimits {
    /// Maximum processes alive in the job at once. `None` means no active-process cap.
    pub active_processes: Option<u32>,
    /// Maximum committed memory for the whole job. `None` means no job-memory cap.
    pub memory_bytes: Option<usize>,
}

/// The macOS `sandbox-exec` policy: reads are unrestricted, writes are limited to the listed
/// roots, and network access is denied unless explicitly enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExecPolicy {
    pub write_roots: Vec<PathBuf>,
    pub allow_network: bool,
}

impl SandboxExecPolicy {
    /// The normal project profile: write only under the project, no network.
    pub fn for_project(root: impl Into<PathBuf>) -> Self {
        Self {
            write_roots: vec![root.into()],
            allow_network: false,
        }
    }

    /// The SBPL handed to `sandbox-exec -p`. Paths are quoted with SBPL's string escaping.
    pub fn profile(&self) -> String {
        let mut profile = String::from(
            "(version 1)\n\
             (deny default)\n\
             (allow process*)\n\
             (allow signal)\n\
             (allow file-read*)\n\
             (allow sysctl-read)\n\
             (allow mach-lookup)\n",
        );
        if self.allow_network {
            profile.push_str("(allow network*)\n");
        }
        for root in &self.write_roots {
            profile.push_str(&format!(
                "(allow file-write* (subpath \"{}\"))\n",
                sbpl_escape(root)
            ));
        }
        profile
    }
}

/// Linux namespace policy. `user` enables a user namespace and maps the caller to root inside
/// it; `mount`, `pid` and `network` request the corresponding namespaces. The wrapper uses
/// `unshare`, so a host that disables user namespaces reports an unavailable profile rather
/// than running the command outside the requested namespaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxNamespacePolicy {
    pub user: bool,
    pub mount: bool,
    pub pid: bool,
    pub network: bool,
}

impl Default for LinuxNamespacePolicy {
    fn default() -> Self {
        Self {
            user: true,
            mount: true,
            pid: true,
            network: true,
        }
    }
}

/// A requested containment profile. All variants exist on every platform so a policy can be
/// parsed and explained on a machine that cannot execute it; [`Sandbox::new`] refuses an
/// unavailable profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SandboxProfile {
    #[default]
    None,
    JobObject(JobLimits),
    SandboxExec(SandboxExecPolicy),
    LinuxNamespaces(LinuxNamespacePolicy),
}

impl SandboxProfile {
    pub fn job_object(limits: JobLimits) -> Self {
        Self::JobObject(limits)
    }

    pub fn sandbox_exec(policy: SandboxExecPolicy) -> Self {
        Self::SandboxExec(policy)
    }

    pub fn linux_namespaces(policy: LinuxNamespacePolicy) -> Self {
        Self::LinuxNamespaces(policy)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::JobObject(_) => "windows-job-object",
            Self::SandboxExec(_) => "macos-sandbox-exec",
            Self::LinuxNamespaces(_) => "linux-namespaces",
        }
    }

    /// The written containment statement. It names both sides: what the profile blocks and what
    /// it still allows. [`Sandbox::new`] refuses a profile the host cannot execute.
    pub fn containment(&self) -> &'static str {
        match self {
            Self::None => {
                "No sandbox: this profile blocks nothing and allows the command the same filesystem, network and child processes as Rigel itself."
            }
            Self::JobObject(_) => {
                "Windows Job Object: blocks a child process from surviving after the job handle closes and can cap active processes and job memory; allows filesystem access, network access, and child processes inside the job."
            }
            Self::SandboxExec(_) => {
                "macOS sandbox-exec: blocks writes outside the configured roots and network access unless allow_network is set; allows command execution, reads, and writes under the configured roots."
            }
            Self::LinuxNamespaces(_) => {
                "Linux namespaces through unshare: blocks the requested network, PID and mount visibility (a mount namespace alone does not remount the filesystem); allows command execution, reads and writes visible in the namespace, and child processes."
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("the {0} sandbox profile is not available on this platform")]
    Unsupported(&'static str),
    #[error("the sandbox tool {0} is not installed")]
    ToolMissing(String),
    #[error("could not create the Windows Job Object: {0}")]
    JobObject(std::io::Error),
    #[error("could not assign the child to the Windows Job Object: {0}")]
    Assign(std::io::Error),
}

/// A prepared profile. The Job Object handle stays alive until the value is dropped, so
/// `KILL_ON_JOB_CLOSE` is what makes a child tree unable to outlive the command.
pub struct Sandbox {
    profile: SandboxProfile,
    #[cfg(windows)]
    job: Option<JobHandle>,
}

impl Sandbox {
    pub fn new(profile: SandboxProfile) -> Result<Self, SandboxError> {
        let selected = profile.clone();
        match &selected {
            SandboxProfile::None => Ok(Self {
                profile,
                #[cfg(windows)]
                job: None,
            }),
            SandboxProfile::JobObject(limits) => {
                #[cfg(windows)]
                {
                    Ok(Self {
                        profile,
                        job: Some(create_job(limits)?),
                    })
                }
                #[cfg(not(windows))]
                {
                    let _ = limits;
                    Err(SandboxError::Unsupported("Windows Job Object"))
                }
            }
            SandboxProfile::SandboxExec(policy) => {
                #[cfg(target_os = "macos")]
                {
                    if !Path::new("/usr/bin/sandbox-exec").is_file() {
                        return Err(SandboxError::ToolMissing("sandbox-exec".to_string()));
                    }
                    let _ = policy;
                    Ok(Self {
                        profile,
                        #[cfg(windows)]
                        job: None,
                    })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = policy;
                    Err(SandboxError::Unsupported("macOS sandbox-exec"))
                }
            }
            SandboxProfile::LinuxNamespaces(policy) => {
                #[cfg(target_os = "linux")]
                {
                    if find_in_path("unshare").is_none() {
                        return Err(SandboxError::ToolMissing("unshare".to_string()));
                    }
                    if !linux_namespaces_available(policy) {
                        return Err(SandboxError::Unsupported(
                            "Linux namespaces (unshare or user namespaces are unavailable)",
                        ));
                    }
                    Ok(Self {
                        profile,
                        #[cfg(windows)]
                        job: None,
                    })
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = policy;
                    Err(SandboxError::Unsupported("Linux namespaces"))
                }
            }
        }
    }

    pub fn profile(&self) -> &SandboxProfile {
        &self.profile
    }

    pub fn name(&self) -> &'static str {
        self.profile.name()
    }

    pub fn containment(&self) -> &'static str {
        self.profile.containment()
    }

    /// Build the shell command for `run_command`, applying a wrapper when the profile needs one.
    pub fn shell(&self, command: &str) -> Result<tokio::process::Command, SandboxError> {
        let (program, args) = shell_parts(command);
        match &self.profile {
            SandboxProfile::None | SandboxProfile::JobObject(_) => {
                let mut process = tokio::process::Command::new(program);
                process.args(args);
                Ok(process)
            }
            SandboxProfile::SandboxExec(policy) => {
                #[cfg(target_os = "macos")]
                {
                    let mut process = tokio::process::Command::new("/usr/bin/sandbox-exec");
                    process.arg("-p").arg(policy.profile()).arg(&program);
                    process.args(&args);
                    Ok(process)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = policy;
                    Err(SandboxError::Unsupported("macOS sandbox-exec"))
                }
            }
            SandboxProfile::LinuxNamespaces(policy) => {
                #[cfg(target_os = "linux")]
                {
                    let mut process = tokio::process::Command::new("unshare");
                    if policy.user {
                        process.args(["--user", "--map-root-user"]);
                    }
                    if policy.mount {
                        process.arg("--mount");
                    }
                    if policy.pid {
                        process.args(["--pid", "--fork"]);
                    }
                    if policy.network {
                        process.arg("--net");
                    }
                    if policy.mount && policy.pid {
                        process.arg("--mount-proc");
                    }
                    process.arg("--").arg(&program).args(&args);
                    Ok(process)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = policy;
                    Err(SandboxError::Unsupported("Linux namespaces"))
                }
            }
        }
    }

    /// Attach an already-spawned child to the platform boundary. On platforms whose boundary is
    /// applied by the wrapper this is a no-op.
    pub fn attach(&mut self, pid: u32) -> Result<(), SandboxError> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            return async_attach(job, pid);
        }
        let _ = pid;
        Ok(())
    }
}

fn shell_parts(command: &str) -> (&'static str, Vec<String>) {
    #[cfg(windows)]
    {
        ("cmd", vec!["/C".to_string(), command.to_string()])
    }
    #[cfg(not(windows))]
    {
        ("/bin/sh", vec!["-c".to_string(), command.to_string()])
    }
}

fn sbpl_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[cfg(unix)]
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(target_os = "linux")]
fn linux_namespaces_available(policy: &LinuxNamespacePolicy) -> bool {
    let mut probe = std::process::Command::new("unshare");
    if policy.user {
        probe.args(["--user", "--map-root-user"]);
    }
    if policy.mount {
        probe.arg("--mount");
    }
    if policy.pid {
        probe.args(["--pid", "--fork"]);
    }
    if policy.network {
        probe.arg("--net");
    }
    if policy.mount && policy.pid {
        probe.arg("--mount-proc");
    }
    probe
        .args(["--", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
struct JobHandle {
    raw: usize,
}

#[cfg(windows)]
impl JobHandle {
    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.raw as windows_sys::Win32::Foundation::HANDLE
    }
}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.raw());
        }
    }
}

#[cfg(windows)]
fn create_job(limits: &JobLimits) -> Result<JobHandle, SandboxError> {
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(SandboxError::JobObject(std::io::Error::last_os_error()));
        }
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(count) = limits.active_processes {
            information.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            information.BasicLimitInformation.ActiveProcessLimit = count;
        }
        if let Some(bytes) = limits.memory_bytes {
            information.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            information.JobMemoryLimit = bytes;
        }
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &information as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if configured == 0 {
            let error = std::io::Error::last_os_error();
            windows_sys::Win32::Foundation::CloseHandle(job);
            return Err(SandboxError::JobObject(error));
        }
        Ok(JobHandle { raw: job as usize })
    }
}

#[cfg(windows)]
fn async_attach(job: &JobHandle, pid: u32) -> Result<(), SandboxError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    unsafe {
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return Err(SandboxError::Assign(std::io::Error::last_os_error()));
        }
        let assigned =
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(job.raw(), process);
        CloseHandle(process);
        if assigned == 0 {
            return Err(SandboxError::Assign(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_has_a_containment_statement_for_both_sides() {
        let profiles = [
            SandboxProfile::None,
            SandboxProfile::job_object(JobLimits::default()),
            SandboxProfile::sandbox_exec(SandboxExecPolicy::for_project(".")),
            SandboxProfile::linux_namespaces(LinuxNamespacePolicy::default()),
        ];
        for profile in profiles {
            let statement = profile.containment();
            assert!(
                statement.contains("blocks") && statement.contains("allows"),
                "{} lacks a two-sided statement: {statement}",
                profile.name()
            );
            assert_ne!(profile.name(), "");
        }
    }

    #[tokio::test]
    async fn none_is_a_direct_shell() {
        let sandbox = Sandbox::new(SandboxProfile::None).unwrap();
        let command = sandbox.shell("echo ok").unwrap();
        #[cfg(windows)]
        assert_eq!(command.as_std().get_program(), "cmd");
        #[cfg(not(windows))]
        assert_eq!(command.as_std().get_program(), "/bin/sh");
        assert_eq!(sandbox.name(), "none");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandbox_exec_allows_a_project_write_and_blocks_an_outside_write() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let policy = SandboxExecPolicy::for_project(root.path());
        let sandbox = match Sandbox::new(SandboxProfile::sandbox_exec(policy)) {
            Ok(sandbox) => sandbox,
            Err(error) => {
                eprintln!("skipping macOS sandbox runtime test: {error}");
                return;
            }
        };
        let inside = root.path().join("inside.txt");
        let outside_file = outside.path().join("outside.txt");
        let script = format!(
            "printf ok > {}; printf no > {} || true",
            shell_quote(&inside),
            shell_quote(&outside_file)
        );
        let output = sandbox
            .shell(&script)
            .unwrap()
            .current_dir(root.path())
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(inside.is_file());
        assert!(!outside_file.is_file());
        assert_eq!(sandbox.name(), "macos-sandbox-exec");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_namespaces_allow_a_command_and_block_network() {
        let root = tempfile::tempdir().unwrap();
        let sandbox = match Sandbox::new(SandboxProfile::linux_namespaces(
            LinuxNamespacePolicy::default(),
        )) {
            Ok(sandbox) => sandbox,
            Err(error) => {
                eprintln!("skipping Linux namespace runtime test: {error}");
                return;
            }
        };
        let ok = sandbox
            .shell("printf ok")
            .unwrap()
            .current_dir(root.path())
            .output()
            .await;
        let ok = match ok {
            Ok(output) => output,
            Err(error) => {
                eprintln!("skipping Linux namespace runtime test: {error}");
                return;
            }
        };
        assert!(ok.status.success());
        assert_eq!(ok.stdout, b"ok");

        let python = std::process::Command::new("python3")
            .arg("--version")
            .output();
        if python.is_ok() {
            let probe = sandbox
                .shell("python3 -c \"import socket; socket.create_connection(('1.1.1.1', 80), 1)\"")
                .unwrap()
                .current_dir(root.path())
                .output()
                .await
                .unwrap();
            assert!(
                !probe.status.success(),
                "the network namespace still reached 1.1.1.1"
            );
        }
        assert_eq!(sandbox.name(), "linux-namespaces");
    }

    #[cfg(target_os = "macos")]
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    #[cfg(windows)]
    #[test]
    fn a_job_object_allows_a_command_to_finish() {
        let mut sandbox = Sandbox::new(SandboxProfile::job_object(JobLimits::default())).unwrap();
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 2 127.0.0.1 > NUL"])
            .spawn()
            .unwrap();
        sandbox.attach(child.id()).unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[cfg(windows)]
    #[test]
    fn a_job_object_blocks_a_child_from_outliving_the_handle() {
        let mut sandbox = Sandbox::new(SandboxProfile::job_object(JobLimits::default())).unwrap();
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 60 127.0.0.1 > NUL"])
            .spawn()
            .unwrap();
        sandbox.attach(child.id()).unwrap();
        assert!(
            child.try_wait().unwrap().is_none(),
            "the child exited before the containment handle was dropped"
        );
        drop(sandbox);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child survived after the job handle closed"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        // The child was still pinging when the handle closed; reaching a status before the
        // deadline is the containment assertion. Windows may report the killed process with
        // either a failure or a synthetic success code, so the code itself is not the contract.
        let _ = status;
    }
}
