//! Per-project retention policy and preview-bound cleanup.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::attachments::{AttachmentStore, store::default_root};
use crate::snapshot::Manifest;
use crate::storage::{StorageError, Store};
use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("attachment error: {0}")]
    Attachment(#[from] crate::attachments::AttachmentError),
    #[error("retention policy is invalid: {0}")]
    InvalidPolicy(String),
    #[error(
        "purge confirmation does not match current preview (expected {count} rows and {bytes} bytes)"
    )]
    Confirmation { count: u64, bytes: u64 },
}

/// A zero-day value disables that cleanup category. Other values are bounded to ten years.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionPolicy {
    pub turns_days: u32,
    pub blobs_days: u32,
    pub logs_days: u32,
    pub snapshots_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            turns_days: 90,
            blobs_days: 90,
            logs_days: 90,
            snapshots_days: 90,
        }
    }
}

impl RetentionPolicy {
    pub fn validate(self) -> Result<Self, RetentionError> {
        for (name, value) in [
            ("turns_days", self.turns_days),
            ("blobs_days", self.blobs_days),
            ("logs_days", self.logs_days),
            ("snapshots_days", self.snapshots_days),
        ] {
            if value > 3650 {
                return Err(RetentionError::InvalidPolicy(format!(
                    "{name} must be 0..=3650"
                )));
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RetentionCategory {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionPreview {
    pub project: String,
    pub policy: RetentionPolicy,
    pub turns: RetentionCategory,
    pub blobs: RetentionCategory,
    pub logs: RetentionCategory,
    pub snapshots: RetentionCategory,
    pub total_count: u64,
    pub total_bytes: u64,
}

impl RetentionPreview {
    fn new(project: &Path, policy: RetentionPolicy) -> Self {
        Self {
            project: project.display().to_string(),
            policy,
            turns: RetentionCategory::default(),
            blobs: RetentionCategory::default(),
            logs: RetentionCategory::default(),
            snapshots: RetentionCategory::default(),
            total_count: 0,
            total_bytes: 0,
        }
    }

    fn totalize(&mut self) {
        for category in [&self.turns, &self.blobs, &self.logs, &self.snapshots] {
            self.total_count += category.count;
            self.total_bytes += category.bytes;
        }
    }
}

pub fn policy_path(project: &Path) -> PathBuf {
    project.join(".rigel").join("retention.json")
}

pub fn load_policy(project: &Path) -> Result<RetentionPolicy, RetentionError> {
    let path = policy_path(project);
    if !path.exists() {
        return Ok(RetentionPolicy::default());
    }
    let policy: RetentionPolicy = serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| RetentionError::InvalidPolicy(error.to_string()))?;
    policy.validate()
}

pub fn save_policy(project: &Path, policy: RetentionPolicy) -> Result<(), RetentionError> {
    let policy = policy.validate()?;
    let path = policy_path(project);
    fs::create_dir_all(path.parent().unwrap_or(project))?;
    fs::write(
        path,
        serde_json::to_vec_pretty(&policy)
            .map_err(|error| RetentionError::InvalidPolicy(error.to_string()))?,
    )?;
    Ok(())
}

/// Inspect the project without deleting anything. Preview totals are the values required by apply.
pub async fn preview(
    store: &Store,
    project: &Path,
    data_dir: &Path,
    now: i64,
) -> Result<RetentionPreview, RetentionError> {
    let policy = load_policy(project)?;
    let mut report = RetentionPreview::new(project, policy);
    let project_key = project.to_string_lossy().into_owned();
    if policy.turns_days > 0 {
        let cutoff = now.saturating_sub(days_ms(policy.turns_days));
        let row: (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(length(CAST(COALESCE(t.prompt, '') AS BLOB)) + COALESCE((SELECT SUM(length(CAST(i.body_json AS BLOB)) + length(CAST(i.raw_json AS BLOB))) FROM items i WHERE i.turn_id = t.id), 0)), 0) FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id WHERE s.project_root = ? AND t.completed_at IS NOT NULL AND t.completed_at < ?"
        ).bind(&project_key).bind(cutoff).fetch_one(store.pool()).await?;
        report.turns = RetentionCategory {
            count: row.0.max(0) as u64,
            bytes: row.1.max(0) as u64,
        };
    }

    if policy.blobs_days > 0 {
        let attachment_store =
            AttachmentStore::new(default_root(project), u64::MAX, usize::MAX, Vec::new());
        let cutoff = system_time_cutoff(now, policy.blobs_days);
        for hash in attachment_store.list("default")? {
            if !attachment_store.sessions_for("default", &hash).is_empty() {
                continue;
            }
            let path = default_root(project).join("default").join(&hash);
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata
                .modified()
                .ok()
                .is_some_and(|modified| modified < cutoff)
            {
                report.blobs.count += 1;
                report.blobs.bytes += metadata.len();
            }
        }
    }

    if policy.logs_days > 0 {
        report.logs = scan_old_files(
            &project.join(".rigel").join("logs"),
            system_time_cutoff(now, policy.logs_days),
        );
    }

    if policy.snapshots_days > 0 {
        let snapshots = data_dir.join("snapshots");
        let cutoff_ms = now.saturating_sub(days_ms(policy.snapshots_days));
        for directory in directories(&snapshots) {
            let manifest_path = directory.join("manifest.json");
            let Ok(bytes) = fs::read(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
                continue;
            };
            if !same_project_root(&manifest.root, project) || manifest.created_at >= cutoff_ms {
                continue;
            }
            report.snapshots.count += 1;
            report.snapshots.bytes += tree_bytes(&directory);
        }
    }
    report.totalize();
    Ok(report)
}

/// Delete only after the caller repeats the exact preview totals; totals are recomputed first.
pub async fn apply(
    store: &Store,
    project: &Path,
    data_dir: &Path,
    now: i64,
    confirm_count: u64,
    confirm_bytes: u64,
) -> Result<RetentionPreview, RetentionError> {
    let report = preview(store, project, data_dir, now).await?;
    if report.total_count != confirm_count || report.total_bytes != confirm_bytes {
        return Err(RetentionError::Confirmation {
            count: report.total_count,
            bytes: report.total_bytes,
        });
    }
    let cutoff = now.saturating_sub(days_ms(report.policy.turns_days));
    if report.policy.turns_days > 0 {
        let mut transaction = store.pool().begin().await?;
        sqlx::query("DELETE FROM turns WHERE completed_at IS NOT NULL AND completed_at < ? AND agent_id IN (SELECT a.id FROM agents a JOIN sessions s ON s.id = a.session_id WHERE s.project_root = ?)")
            .bind(cutoff).bind(project.to_string_lossy().as_ref()).execute(&mut *transaction).await?;
        transaction.commit().await?;
    }

    let attachment_store =
        AttachmentStore::new(default_root(project), u64::MAX, usize::MAX, Vec::new());
    if report.policy.blobs_days > 0 {
        let cutoff_time = system_time_cutoff(now, report.policy.blobs_days);
        let mut keep = HashSet::new();
        for hash in attachment_store.list("default")? {
            let path = default_root(project).join("default").join(&hash);
            let is_expired = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified < cutoff_time);
            if !is_expired || !attachment_store.sessions_for("default", &hash).is_empty() {
                keep.insert(hash);
            }
        }
        attachment_store.purge("default", &keep)?;
    }
    if report.policy.logs_days > 0 {
        remove_old_files(
            &project.join(".rigel").join("logs"),
            system_time_cutoff(now, report.policy.logs_days),
        )?;
    }
    if report.policy.snapshots_days > 0 {
        let cutoff_ms = now.saturating_sub(days_ms(report.policy.snapshots_days));
        for directory in directories(&data_dir.join("snapshots")) {
            let Ok(bytes) = fs::read(directory.join("manifest.json")) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
                continue;
            };
            if same_project_root(&manifest.root, project) && manifest.created_at < cutoff_ms {
                fs::remove_dir_all(directory)?;
            }
        }
    }
    Ok(report)
}

fn days_ms(days: u32) -> i64 {
    i64::from(days).saturating_mul(DAY_MS)
}

fn system_time_cutoff(now: i64, days: u32) -> std::time::SystemTime {
    let age = now.saturating_sub(days_ms(days)).max(0) as u64;
    std::time::UNIX_EPOCH + std::time::Duration::from_millis(age)
}

fn same_project_root(manifest_root: &str, project: &Path) -> bool {
    let expected = fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let actual = fs::canonicalize(manifest_root).unwrap_or_else(|_| PathBuf::from(manifest_root));
    actual == expected
}

fn directories(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect();
    paths.sort();
    paths
}

fn scan_old_files(path: &Path, cutoff: std::time::SystemTime) -> RetentionCategory {
    let mut category = RetentionCategory::default();
    let Ok(entries) = fs::read_dir(path) else {
        return category;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file()
            && metadata
                .modified()
                .ok()
                .is_some_and(|modified| modified < cutoff)
        {
            category.count += 1;
            category.bytes += metadata.len();
        }
    }
    category
}

fn remove_old_files(path: &Path, cutoff: std::time::SystemTime) -> Result<(), RetentionError> {
    let Ok(entries) = fs::read_dir(path) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let metadata = entry.metadata()?;
        if metadata.is_file() && metadata.modified().is_ok_and(|modified| modified < cutoff) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            entry
                .metadata()
                .map(|metadata| {
                    if metadata.is_dir() {
                        tree_bytes(&entry.path())
                    } else {
                        metadata.len()
                    }
                })
                .unwrap_or(0)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_and_bounds_are_explicit() {
        assert_eq!(RetentionPolicy::default().turns_days, 90);
        assert!(
            RetentionPolicy {
                turns_days: 3651,
                ..RetentionPolicy::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RetentionPolicy {
                turns_days: 0,
                ..RetentionPolicy::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[tokio::test]
    async fn preview_and_apply_require_the_same_counts_and_bytes() {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open_project(project.path()).await.unwrap();
        let sessions = crate::session::SessionService::from_store(&store, project.path());
        let session = sessions.start(Some("old"), None).await.unwrap();
        let agent = sessions
            .register_agent(&session.id, "native", None)
            .await
            .unwrap();
        let turn = sessions
            .append_turn(&agent.id, Some("old prompt"))
            .await
            .unwrap();
        sessions.finish_turn(&turn.id, "completed").await.unwrap();
        sqlx::query("UPDATE turns SET completed_at = 1 WHERE id = ?")
            .bind(&turn.id)
            .execute(store.pool())
            .await
            .unwrap();
        save_policy(
            project.path(),
            RetentionPolicy {
                turns_days: 1,
                blobs_days: 0,
                logs_days: 0,
                snapshots_days: 0,
            },
        )
        .unwrap();
        let now = crate::now_ms() + DAY_MS * 2;
        let report = preview(&store, project.path(), data.path(), now)
            .await
            .unwrap();
        assert_eq!(report.turns.count, 1);
        assert!(report.turns.bytes >= 10);
        assert!(matches!(
            apply(&store, project.path(), data.path(), now, 0, 0).await,
            Err(RetentionError::Confirmation { .. })
        ));
        apply(
            &store,
            project.path(),
            data.path(),
            now,
            report.total_count,
            report.total_bytes,
        )
        .await
        .unwrap();
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM turns WHERE id = ?")
            .bind(&turn.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn project_log_scan_ignores_recent_logs() {
        let project = tempfile::tempdir().unwrap();
        let logs = project.path().join(".rigel/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("current.log"), b"current").unwrap();
        assert_eq!(
            scan_old_files(
                &logs,
                std::time::SystemTime::now() - std::time::Duration::from_secs(60)
            ),
            RetentionCategory::default()
        );
    }

    #[tokio::test]
    async fn preview_and_apply_cover_unreferenced_blobs_logs_and_snapshots() {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = Store::open_project(project.path()).await.unwrap();
        let attachments = AttachmentStore::new(
            default_root(project.path()),
            u64::MAX,
            usize::MAX,
            Vec::new(),
        );
        let blob = attachments
            .put("default", b"old blob", "text/plain")
            .unwrap();

        let logs = project.path().join(".rigel/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("old.log"), b"old log").unwrap();

        let snapshot = data.path().join("snapshots/old");
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(snapshot.join("payload"), b"snapshot bytes").unwrap();
        let manifest = Manifest {
            id: "old".into(),
            created_at: 1,
            root: project.path().to_string_lossy().into_owned(),
            kind: crate::snapshot::SnapshotKind::Tree,
            stash: None,
            git_ref: None,
            deleted: Vec::new(),
            files: Vec::new(),
            total_bytes: 0,
            note: String::new(),
        };
        fs::write(
            snapshot.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        save_policy(
            project.path(),
            RetentionPolicy {
                turns_days: 0,
                blobs_days: 1,
                logs_days: 1,
                snapshots_days: 1,
            },
        )
        .unwrap();

        let now = crate::now_ms() + DAY_MS * 2;
        let report = preview(&store, project.path(), data.path(), now)
            .await
            .unwrap();
        assert_eq!(report.blobs.count, 1);
        assert_eq!(report.logs.count, 1);
        assert_eq!(report.snapshots.count, 1);
        assert_eq!(report.total_count, 3);
        assert!(matches!(
            apply(
                &store,
                project.path(),
                data.path(),
                now,
                2,
                report.total_bytes
            )
            .await,
            Err(RetentionError::Confirmation { count: 3, .. })
        ));
        apply(
            &store,
            project.path(),
            data.path(),
            now,
            report.total_count,
            report.total_bytes,
        )
        .await
        .unwrap();
        assert!(
            !default_root(project.path())
                .join("default")
                .join(blob.hash)
                .exists()
        );
        assert!(!logs.join("old.log").exists());
        assert!(!snapshot.exists());
    }
}
