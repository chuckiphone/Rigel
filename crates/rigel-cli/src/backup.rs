//! `rigel backup` — encrypted local data snapshots (roadmap #28).
//!
//! The passphrase is never an argument: it comes from a file or
//! `RIGEL_BACKUP_PASSPHRASE` (`RIGEL_BACKUP_NEW_PASSPHRASE` for rotation), so it does not land in
//! shell history or a process listing. The archive itself is an age file; the manifest and every
//! data entry inside are authenticated before they are used.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::backup::{self, ARCHIVE_EXTENSION, DrillReport};
use serde_json::json;

const PASSPHRASE_ENV: &str = "RIGEL_BACKUP_PASSPHRASE";
const NEW_PASSPHRASE_ENV: &str = "RIGEL_BACKUP_NEW_PASSPHRASE";
const DRILL_RECEIPT: &str = "last-drill.json";

#[derive(Subcommand)]
pub enum BackupCommand {
    /// Encrypt a copy of the global data directory.
    Create {
        /// Archive path. Defaults to <data-dir>/backups/rigel-<timestamp>.rigel-backup.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Read the passphrase from this file instead of RIGEL_BACKUP_PASSPHRASE.
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Create or verify today's encrypted backup and retain the newest N daily archives.
    Daily {
        /// Number of verified daily archives to retain.
        #[arg(long, default_value_t = 7, value_name = "COUNT")]
        keep: usize,
        /// Read the passphrase from this file instead of RIGEL_BACKUP_PASSPHRASE.
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Decrypt and check every file and the manifest without writing anything.
    Verify {
        archive: PathBuf,
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Restore into a directory that does not exist or is empty.
    Restore {
        archive: PathBuf,
        #[arg(long, value_name = "DIR")]
        into: PathBuf,
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Re-encrypt an archive under a new passphrase.
    Rotate {
        archive: PathBuf,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// The current passphrase; defaults to RIGEL_BACKUP_PASSPHRASE.
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        /// The new passphrase; defaults to RIGEL_BACKUP_NEW_PASSPHRASE.
        #[arg(long, value_name = "FILE")]
        new_passphrase_file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Restore into a temporary directory and run SQLite's integrity check.
    ///
    /// Use --max-age-days from Task Scheduler or cron: a recent successful receipt makes the
    /// command a no-op, while a failure or a stale receipt runs the drill again.
    Drill {
        archive: PathBuf,
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long, default_value_t = 0, value_name = "DAYS")]
        max_age_days: u64,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: BackupCommand, data_dir: PathBuf) -> ExitCode {
    match command {
        BackupCommand::Create {
            out,
            passphrase_file,
            json: as_json,
        } => {
            let passphrase = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            let out = out.unwrap_or_else(|| default_archive(&data_dir));
            match backup::create(&data_dir, &out, &passphrase).await {
                Ok(report) => {
                    if as_json {
                        print_json(&report);
                    } else {
                        println!(
                            "backup {}  {} files, {} bytes  ({})",
                            report.path.display(),
                            report.files,
                            report.bytes,
                            source_age(report.created_at)
                        );
                        for omitted in &report.excluded {
                            println!("  excluded {omitted}");
                        }
                        println!(
                            "  passphrase was read from a file or environment; it was not stored"
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error.to_string(), as_json),
            }
        }

        BackupCommand::Daily {
            keep,
            passphrase_file,
            json: as_json,
        } => {
            let passphrase = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            let backups_dir = data_dir.join("backups");
            match backup::create_daily(&data_dir, &backups_dir, &passphrase, keep).await {
                Ok(report) => {
                    if as_json {
                        print_json(&report);
                    } else {
                        let action = if report.created {
                            "created"
                        } else {
                            "verified"
                        };
                        println!(
                            "{action} daily backup {}  {} files, {} bytes  ({} retained)",
                            report.path.display(),
                            report.files,
                            report.bytes,
                            report.kept
                        );
                        for path in &report.pruned {
                            println!("  pruned {}", path.display());
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error.to_string(), as_json),
            }
        }

        BackupCommand::Verify {
            archive,
            passphrase_file,
            json: as_json,
        } => {
            let passphrase = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            match backup::verify(&archive, &passphrase) {
                Ok(report) => {
                    if as_json {
                        print_json(&report);
                    } else {
                        println!(
                            "verified {}  {} files, {} bytes  ({})",
                            report.path.display(),
                            report.files,
                            report.bytes,
                            source_age(report.created_at)
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error.to_string(), as_json),
            }
        }

        BackupCommand::Restore {
            archive,
            into,
            passphrase_file,
            json: as_json,
        } => {
            let passphrase = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            match backup::restore(&archive, &into, &passphrase).await {
                Ok(report) => {
                    if as_json {
                        print_json(&report);
                    } else {
                        println!(
                            "rehearsed and restored {} files ({} bytes) into {}",
                            report.files,
                            report.bytes,
                            report.into.display()
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error.to_string(), as_json),
            }
        }

        BackupCommand::Rotate {
            archive,
            out,
            passphrase_file,
            new_passphrase_file,
            json: as_json,
        } => {
            let old = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            let new = match read_passphrase(new_passphrase_file.as_deref(), NEW_PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            if old == new {
                return fail("the new passphrase must be different", as_json);
            }
            match backup::rotate(&archive, &out, &old, &new) {
                Ok(report) => {
                    if as_json {
                        print_json(&report);
                    } else {
                        println!(
                            "rotated {}  {} files, {} bytes",
                            report.path.display(),
                            report.files,
                            report.bytes
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error.to_string(), as_json),
            }
        }

        BackupCommand::Drill {
            archive,
            passphrase_file,
            max_age_days,
            json: as_json,
        } => {
            let receipt_path = data_dir.join("backups").join(DRILL_RECEIPT);
            if let Some(receipt) = fresh_drill(&receipt_path, &archive, max_age_days) {
                if as_json {
                    print_json(&json!({ "ok": true, "skipped": true, "receipt": receipt }));
                } else {
                    println!(
                        "drill not due: last successful check was {}",
                        source_age(receipt.checked_at)
                    );
                }
                return ExitCode::SUCCESS;
            }
            let passphrase = match read_passphrase(passphrase_file.as_deref(), PASSPHRASE_ENV) {
                Ok(passphrase) => passphrase,
                Err(error) => return fail(&error, as_json),
            };
            match backup::drill(&archive, &passphrase).await {
                Ok(report) => {
                    write_drill_receipt(&receipt_path, &report);
                    if as_json {
                        print_json(&report);
                    } else {
                        println!(
                            "drill ok  {} files, {} bytes, database {}  ({})",
                            report.files,
                            report.bytes,
                            report.database,
                            source_age(report.checked_at)
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    // A stale receipt must not make the next scheduled run skip after a failure.
                    let _ = fs::remove_file(&receipt_path);
                    fail(&error.to_string(), as_json)
                }
            }
        }
    }
}

fn read_passphrase(file: Option<&Path>, variable: &str) -> Result<String, String> {
    let value = match file {
        Some(path) => fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?,
        None => std::env::var(variable)
            .map_err(|_| format!("set {variable} or pass a passphrase file"))?,
    };
    let value = value.trim_end_matches(&['\r', '\n'][..]).to_string();
    if value.trim().is_empty() {
        Err("the passphrase is empty".to_string())
    } else {
        Ok(value)
    }
}

fn default_archive(data_dir: &Path) -> PathBuf {
    data_dir.join("backups").join(format!(
        "rigel-{}.{ARCHIVE_EXTENSION}",
        rigel_core::now_ms()
    ))
}

fn fresh_drill(path: &Path, archive: &Path, max_age_days: u64) -> Option<DrillReport> {
    if max_age_days == 0 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let receipt: DrillReport = serde_json::from_str(&text).ok()?;
    if !receipt.ok || receipt.archive != archive.display().to_string() {
        return None;
    }
    let age = rigel_core::now_ms().saturating_sub(receipt.checked_at);
    if age < 0 || age > max_age_days.saturating_mul(24 * 60 * 60 * 1000) as i64 {
        None
    } else {
        Some(receipt)
    }
}

fn write_drill_receipt(path: &Path, report: &DrillReport) {
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(report) {
        let _ = fs::write(path, bytes);
    }
}

fn source_age(timestamp: i64) -> String {
    let age = rigel_core::now_ms().saturating_sub(timestamp);
    if age < 0 {
        return "in the future".to_string();
    }
    let minutes = age / 60_000;
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 24 * 60 {
        format!("{}h ago", minutes / 60)
    } else {
        format!("{}d ago", minutes / (24 * 60))
    }
}

fn print_json(value: &impl serde::Serialize) {
    crate::json::print(value);
}

fn fail(error: &str, as_json: bool) -> ExitCode {
    if as_json {
        crate::json::print(json!({ "ok": false, "error": error }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(1)
}
