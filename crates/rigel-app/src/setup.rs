//! Roadmap 108: first-run setup probes and the step machine behind the wizard.
//!
//! Every step ends in a real finding from a Tauri command. A step can be skipped, but progress is
//! derived from *completed* or *skipped* steps only: an in-flight probe never moves the bar.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// The steps, in the order the wizard shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupStep {
    Os,
    Shell,
    Git,
    Providers,
    Harnesses,
}

impl SetupStep {
    pub const ALL: [Self; 5] = [
        Self::Os,
        Self::Shell,
        Self::Git,
        Self::Providers,
        Self::Harnesses,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Os => "os",
            Self::Shell => "shell",
            Self::Git => "git",
            Self::Providers => "providers",
            Self::Harnesses => "harnesses",
        }
    }
}

/// A finding is either observed or explicitly skipped; there is no third, optimistic state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupFinding {
    pub step: SetupStep,
    pub detail: String,
    pub skipped: bool,
}

/// Count only steps that actually finished. `pending` is the set still running and must not count.
pub fn progress(completed: &[SetupFinding], pending: &[SetupStep]) -> (usize, usize) {
    let settled = completed.len();
    (settled, settled + pending.len())
}

/// Whether a step may advance: a skip is a completed answer, not a missing one.
pub fn step_answered(finding: Option<&SetupFinding>) -> bool {
    finding.is_some()
}

/// Real OS finding. `current_exe` is not consulted; the compiler's target is the honest answer.
pub fn probe_os() -> SetupFinding {
    SetupFinding {
        step: SetupStep::Os,
        detail: format!(
            "{} / {} ({})",
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::consts::FAMILY
        ),
        skipped: false,
    }
}

/// Real shell finding: the configured shell, or the environment's `$SHELL`, or the platform's
/// default. The path is reported as configured; no fake readiness is implied.
pub fn probe_shell(configured: &str) -> SetupFinding {
    let shell = if !configured.trim().is_empty() {
        configured.trim().to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        })
    };
    SetupFinding {
        step: SetupStep::Shell,
        detail: shell,
        skipped: false,
    }
}

/// Real git finding: run `git --version` and report either its first line or the failure.
pub fn probe_git() -> SetupFinding {
    let detail = match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                stderr
            }
        }
        Err(error) => format!("git is not available: {error}"),
    };
    SetupFinding {
        step: SetupStep::Git,
        detail,
        skipped: false,
    }
}

pub fn skipped(step: SetupStep) -> SetupFinding {
    SetupFinding {
        step,
        detail: "skipped by the user".to_string(),
        skipped: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_never_moves_for_an_unfinished_probe() {
        let (done, total) = progress(&[], &[SetupStep::Os, SetupStep::Git]);
        assert_eq!((done, total), (0, 2));
        let findings = vec![probe_os()];
        assert_eq!(progress(&findings, &[SetupStep::Git]), (1, 2));
        assert_eq!(
            progress(&[probe_os(), skipped(SetupStep::Git)], &[]),
            (2, 2)
        );
    }

    #[test]
    fn a_skip_is_an_answer_and_counts_as_progress() {
        assert!(!step_answered(None));
        assert!(step_answered(Some(&skipped(SetupStep::Providers))));
        let finding = skipped(SetupStep::Providers);
        assert!(finding.skipped);
        assert_eq!(finding.step.as_str(), "providers");
        assert_eq!(SetupStep::ALL.len(), 5);
    }

    #[test]
    fn real_probes_report_observed_values() {
        let os = probe_os();
        assert!(os.detail.contains(std::env::consts::OS));
        assert!(!os.skipped);
        let shell = probe_shell("");
        assert!(!shell.detail.is_empty());
        assert_eq!(probe_shell("pwsh").detail, "pwsh");
        let git = probe_git();
        assert!(!git.detail.is_empty());
        assert!(!git.skipped);
    }
}
