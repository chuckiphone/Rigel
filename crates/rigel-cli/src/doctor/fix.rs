//! `rigel doctor --fix` — the repair side of the doctor.
//!
//! The plain `rigel doctor` command is report-only by default: it never mutates a project.
//! Mutations happen only behind `--fix`. Every repair is listed before it runs, and a second
//! run reports nothing to do, so the command is safe to re-run.

use std::path::Path;
use std::process::ExitCode;

/// One planned repair.
#[derive(Debug, Clone)]
pub struct Repair {
    pub id: String,
    pub detail: String,
}

/// Plan the repairs a project needs, without touching anything.
pub fn plan(project: &Path) -> Vec<Repair> {
    let mut repairs = Vec::new();
    let rigel = project.join(".rigel");

    // Missing .rigel/.gitignore.
    let gitignore = rigel.join(".gitignore");
    if !gitignore.exists() {
        repairs.push(Repair {
            id: "missing-gitignore".to_string(),
            detail: format!("create {}", gitignore.display()),
        });
    }

    // Uncheckpointed WAL.
    let wal = rigel.join("rigel.db-wal");
    if wal.exists() {
        repairs.push(Repair {
            id: "uncheckpointed-wal".to_string(),
            detail: format!("checkpoint {}", wal.display()),
        });
    }

    // Stale journal files.
    if let Ok(entries) = std::fs::read_dir(&rigel) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".db-journal") || name.ends_with(".journal") || name.ends_with(".tmp") {
                repairs.push(Repair {
                    id: "stale-journal".to_string(),
                    detail: format!("remove {}", entry.path().display()),
                });
            }
        }
    }

    // Orphan blobs: files in .rigel/blobs whose name is not a valid sha256 hash.
    let blobs = rigel.join("blobs");
    if blobs.exists() {
        if let Ok(entries) = std::fs::read_dir(&blobs) {
            for entry in entries.flatten() {
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if !is_sha256(&name) {
                    repairs.push(Repair {
                        id: "orphan-blob".to_string(),
                        detail: format!("remove {}", entry.path().display()),
                    });
                }
            }
        }
    }

    repairs
}

fn is_sha256(name: &str) -> bool {
    name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Apply the planned repairs. Returns the number applied.
pub async fn apply(project: &Path, repairs: &[Repair]) -> usize {
    let rigel = project.join(".rigel");
    let mut applied = 0usize;
    for repair in repairs {
        match repair.id.as_str() {
            "missing-gitignore" => {
                let gitignore = rigel.join(".gitignore");
                if let Some(parent) = gitignore.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&gitignore, "target/\n").is_ok() {
                    applied += 1;
                }
            }
            "uncheckpointed-wal" => {
                let db = rigel.join("rigel.db");
                if db.exists() {
                    // Run a real SQLite checkpoint instead of deleting -wal/-shm.
                    let url = format!("sqlite://{}", db.display());
                    if let Ok(pool) = sqlx::sqlite::SqlitePoolOptions::new()
                        .max_connections(1)
                        .connect(&url)
                        .await
                    {
                        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                            .execute(&pool)
                            .await;
                        pool.close().await;
                        applied += 1;
                    }
                } else {
                    // No database yet: the sidecar is stale and safe to remove.
                    let wal = rigel.join("rigel.db-wal");
                    let shm = rigel.join("rigel.db-shm");
                    let _ = std::fs::remove_file(&wal);
                    let _ = std::fs::remove_file(&shm);
                    applied += 1;
                }
            }
            "stale-journal" => {
                // detail contains the path after "remove "
                if let Some(path) = repair.detail.strip_prefix("remove ") {
                    if std::fs::remove_file(path).is_ok() {
                        applied += 1;
                    }
                }
            }
            "orphan-blob" => {
                if let Some(path) = repair.detail.strip_prefix("remove ") {
                    if std::fs::remove_file(path).is_ok() {
                        applied += 1;
                    }
                }
            }
            _ => {}
        }
    }
    applied
}

/// Run the fix pass: list repairs, apply them, report.
pub async fn run(project: &Path) -> ExitCode {
    let repairs = plan(project);
    if repairs.is_empty() {
        println!("nothing to do");
        return ExitCode::SUCCESS;
    }
    println!("repairs planned:");
    for repair in &repairs {
        println!("  - {} ({})", repair.id, repair.detail);
    }
    let applied = apply(project, &repairs).await;
    println!("applied {applied} of {}", repairs.len());
    if applied == repairs.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}