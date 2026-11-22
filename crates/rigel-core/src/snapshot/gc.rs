//! Roadmap 74: snapshot orphan collection.
//!
//! A snapshot is only deleted when it has no readable manifest or when the project it belongs to
//! no longer exists, and then only after a grace period. A readable snapshot whose recorded root
//! still exists is never removed here; retention controls are a separate surface. This keeps a
//! cleanup pass from deleting the only copy of uncommitted work.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{MANIFEST, Manifest, SnapshotError, SnapshotService};

/// The default grace period for a snapshot with no readable manifest or a missing project root.
pub const DEFAULT_ORPHAN_GRACE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcPolicy {
    pub orphan_grace: Duration,
    pub dry_run: bool,
}

impl Default for GcPolicy {
    fn default() -> Self {
        Self {
            orphan_grace: DEFAULT_ORPHAN_GRACE,
            dry_run: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcRemoved {
    pub id: String,
    pub reason: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub scanned: usize,
    pub removed: Vec<GcRemoved>,
    pub kept: Vec<String>,
    pub errors: Vec<String>,
    pub bytes_reclaimed: u64,
}

impl SnapshotService {
    /// Remove old orphan snapshots under the app data directory. Released snapshots are not
    /// touched: their manifest is readable and their project root still exists.
    pub fn gc(&self, policy: &GcPolicy) -> Result<GcReport, SnapshotError> {
        self.gc_at(now_ms(), policy)
    }

    /// Deterministic form of [`Self::gc`], used by the tests and by callers that already know the
    /// clock. It never touches a live snapshot.
    pub fn gc_at(&self, now: i64, policy: &GcPolicy) -> Result<GcReport, SnapshotError> {
        let mut report = GcReport {
            dry_run: policy.dry_run,
            scanned: 0,
            removed: Vec::new(),
            kept: Vec::new(),
            errors: Vec::new(),
            bytes_reclaimed: 0,
        };
        let entries = match fs::read_dir(&self.base) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(source) => {
                return Err(SnapshotError::Io {
                    path: self.base.clone(),
                    source,
                });
            }
        };
        let grace_ms = policy.orphan_grace.as_millis().min(i64::MAX as u128) as i64;
        let mut directories = entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        directories.sort();

        for directory in directories {
            report.scanned += 1;
            let id = directory
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| directory.display().to_string());
            let manifest = read_manifest(&directory);
            let (orphan, reason) = match &manifest {
                Some(manifest) if PathBuf::from(&manifest.root).exists() => (false, String::new()),
                Some(manifest) => {
                    let age = now.saturating_sub(manifest.created_at);
                    (
                        age >= grace_ms,
                        format!(
                            "project root {} no longer exists (snapshot is {:.1} days old)",
                            manifest.root,
                            age as f64 / 86_400_000.0
                        ),
                    )
                }
                None => {
                    let modified = modified_ms(&directory).unwrap_or(0);
                    let age = now.saturating_sub(modified);
                    (
                        age >= grace_ms,
                        format!(
                            "no readable manifest (directory is {:.1} days old)",
                            age as f64 / 86_400_000.0
                        ),
                    )
                }
            };
            if !orphan {
                report.kept.push(id);
                continue;
            }

            let bytes = directory_size(&directory);
            if policy.dry_run {
                report.bytes_reclaimed += bytes;
                report.removed.push(GcRemoved { id, reason, bytes });
                continue;
            }
            match fs::remove_dir_all(&directory) {
                Ok(()) => {
                    report.bytes_reclaimed += bytes;
                    report.removed.push(GcRemoved { id, reason, bytes });
                }
                Err(error) => {
                    report
                        .errors
                        .push(format!("{}: {error}", directory.display()));
                }
            }
        }
        Ok(report)
    }
}

fn read_manifest(directory: &std::path::Path) -> Option<Manifest> {
    let text = fs::read_to_string(directory.join(MANIFEST)).ok()?;
    serde_json::from_str(&text).ok()
}

fn modified_ms(path: &std::path::Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0),
    )
}

fn directory_size(directory: &std::path::Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(directory) else {
        return total;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += directory_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
