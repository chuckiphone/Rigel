//! Roadmap 142: shell completions generated from the clap command tree.
//!
//! There is no hand-maintained list of flags here. `Cli` is the single source of truth, so a new
//! subcommand or flag appears in every shell as soon as it exists. An unknown flag is a clap
//! error: generation refuses a bogus `--shell`, and the shell's own parse of the command tree
//! reports an unexpected argument rather than completing nothing in silence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Subcommand};
use clap_complete::aot::{Shell, generate};
use serde::Serialize;

use crate::Cli;

/// The exact bytes one shell receives. Kept pure so tests can compare generated and installed
/// scripts byte for byte.
pub fn script(shell: Shell) -> String {
    let mut command = Cli::command();
    let mut out = Vec::new();
    generate(shell, &mut command, "rigel", &mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// The file name each shell expects for a binary called `rigel`.
pub fn file_name(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "rigel",
        Shell::Zsh => "_rigel",
        Shell::Fish => "rigel.fish",
        Shell::PowerShell => "rigel.ps1",
        Shell::Elvish => "rigel.elv",
        // `Shell` is non-exhaustive; a future shell gets the generic file name.
        _ => "rigel",
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "no HOME or USERPROFILE to install a completion into".to_string())
}

fn config_dir() -> Result<PathBuf, String> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().ok().map(|home| home.join(".config")))
        .ok_or_else(|| "no config directory to install a completion into".to_string())
}

/// The conventional per-user install location. `--dir` overrides it for a shared or portable
/// install, and for a test that must not touch the real home directory.
pub fn install_path(shell: Shell) -> Result<PathBuf, String> {
    let home = home_dir()?;
    Ok(match shell {
        Shell::Bash => home
            .join(".local/share/bash-completion/completions")
            .join(file_name(shell)),
        Shell::Zsh => home.join(".zfunc").join(file_name(shell)),
        Shell::Fish => config_dir()?
            .join("fish/completions")
            .join(file_name(shell)),
        Shell::PowerShell => home
            .join("Documents")
            .join("PowerShell")
            .join("Completions")
            .join(file_name(shell)),
        Shell::Elvish => config_dir()?.join("elvish/lib").join(file_name(shell)),
        // A shell added by a future clap_complete gets a neutral per-user directory.
        _ => home.join("completions").join(file_name(shell)),
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InstallReport {
    pub shell: String,
    pub path: String,
    pub bytes: usize,
}

/// Write one script to its install path, creating the parent directory when needed.
pub fn install(shell: Shell, dir: Option<&Path>) -> Result<InstallReport, String> {
    let path = match dir {
        Some(dir) => dir.join(file_name(shell)),
        None => install_path(shell)?,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let text = script(shell);
    fs::write(&path, text.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(InstallReport {
        shell: shell.to_string(),
        path: path.to_string_lossy().into_owned(),
        bytes: text.len(),
    })
}

#[derive(Debug, Subcommand)]
pub enum CompletionCommand {
    /// Print the script for one shell to stdout; nothing is installed.
    Generate {
        #[arg(long)]
        shell: Shell,
    },
    /// Write the script to the shell's install path, or into `--dir`.
    Install {
        #[arg(long)]
        shell: Shell,
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print the install path for one shell without writing anything.
    Path {
        #[arg(long)]
        shell: Shell,
    },
}

fn failure(as_json: bool, error: String) -> ExitCode {
    if as_json {
        println!(
            "{}",
            serde_json::json!({ "installed": false, "error": error })
        );
    } else {
        eprintln!("rigel completions: {error}");
    }
    ExitCode::from(2)
}

pub fn run(command: CompletionCommand) -> ExitCode {
    match command {
        CompletionCommand::Generate { shell } => {
            print!("{}", script(shell));
            ExitCode::SUCCESS
        }
        CompletionCommand::Install { shell, dir, json } => match install(shell, dir.as_deref()) {
            Ok(report) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                } else {
                    println!("installed {} completion → {}", report.shell, report.path);
                }
                ExitCode::SUCCESS
            }
            Err(error) => failure(json, error),
        },
        CompletionCommand::Path { shell } => match install_path(shell) {
            Ok(path) => {
                println!("{}", path.display());
                ExitCode::SUCCESS
            }
            Err(error) => failure(false, error),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn the_command_tree_is_valid_and_every_shell_gets_a_distinct_script() {
        Cli::command().debug_assert();
        let bash = script(Shell::Bash);
        let zsh = script(Shell::Zsh);
        let fish = script(Shell::Fish);
        let powershell = script(Shell::PowerShell);
        for text in [&bash, &zsh, &fish, &powershell] {
            assert!(!text.is_empty());
            assert!(
                text.contains("completions"),
                "the new subcommand is generated"
            );
            assert!(text.contains("hooks"), "every subcommand comes from Cli");
        }
        assert!(bash.contains("_rigel()"));
        assert!(zsh.contains("#compdef rigel"));
        assert!(fish.contains("complete -c rigel"));
        assert!(powershell.contains("Register-ArgumentCompleter"));
        let mut all = vec![bash, zsh, fish, powershell];
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 4, "each shell gets its own script");
    }

    #[test]
    fn an_unknown_flag_is_an_error_not_silence() {
        let unknown = match Cli::try_parse_from(["rigel", "--not-a-real-flag"]) {
            Ok(_) => panic!("an unknown flag must not parse"),
            Err(error) => error.to_string(),
        };
        assert!(unknown.contains("--not-a-real-flag"), "{unknown}");

        let bad_shell =
            match Cli::try_parse_from(["rigel", "completions", "generate", "--shell", "nope"]) {
                Ok(_) => panic!("an unknown shell must not parse"),
                Err(error) => error.to_string(),
            };
        assert!(bad_shell.contains("invalid value"), "{bad_shell}");
    }

    #[test]
    fn install_writes_the_exact_generated_bytes_to_each_shells_file_name() {
        let dir = tempfile::tempdir().unwrap();
        for (shell, name) in [
            (Shell::Bash, "rigel"),
            (Shell::Zsh, "_rigel"),
            (Shell::Fish, "rigel.fish"),
            (Shell::PowerShell, "rigel.ps1"),
        ] {
            let report = install(shell, Some(dir.path())).unwrap();
            assert_eq!(report.shell, shell.to_string());
            assert_eq!(Path::new(&report.path), dir.path().join(name));
            assert_eq!(report.bytes, script(shell).len());
            assert_eq!(
                fs::read(dir.path().join(name)).unwrap(),
                script(shell).as_bytes(),
                "{shell} installed bytes match generation"
            );
        }
        // Installing again is idempotent: the same bytes land in the same file.
        install(Shell::Bash, Some(dir.path())).unwrap();
        assert_eq!(
            fs::read(dir.path().join("rigel")).unwrap(),
            script(Shell::Bash).as_bytes()
        );
    }

    #[test]
    fn a_report_is_json_ready() {
        let report = InstallReport {
            shell: "bash".to_string(),
            path: "/tmp/rigel".to_string(),
            bytes: 12,
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["shell"], "bash");
        assert_eq!(value["bytes"], 12);
    }

    #[test]
    fn a_failed_run_has_a_nonzero_exit_code() {
        assert_eq!(failure(false, "nope".to_string()), ExitCode::from(2));
    }
}
