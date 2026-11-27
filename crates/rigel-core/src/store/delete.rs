//! Permanent deletion of project sessions with durable tombstones.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::migrate::now_ms;

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("typed confirmation must exactly match session id {0}")]
    Confirmation(String),
    #[error("session {0} does not exist")]
    NotFound(String),

    #[error("session id {0} is not a single safe path component")]
    UnsafeSessionId(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("could not persist deletion tombstones at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not encode deletion tombstones: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tombstones {
    pub sessions: BTreeMap<String, i64>,
}

/// Keep the registry beside the normal app data directory, outside the tree that backups replace.
pub fn default_tombstone_path(data_dir: &Path) -> PathBuf {
    let name = data_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rigel");
    data_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}-session-tombstones.json"))
}

/// One ordinary path component: no separators, no drive prefix, no `.` or `..`, no NUL.
fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\', ':', '\0'])
        && Path::new(value).components().count() == 1
}

/// Persist a session tombstone before removing its rows, so later restores can filter it.
pub async fn delete_session(
    pool: &SqlitePool,
    project_root: &Path,
    session_id: &str,
    confirmation: &str,
    tombstone_file: &Path,
) -> Result<(), DeleteError> {
    if confirmation != session_id {
        return Err(DeleteError::Confirmation(session_id.to_owned()));
    }
    // The id later becomes one directory name under `.rigel/sessions`. Validate it as a single
    // component first, so a crafted id cannot remove a sibling directory or escape the project.
    if !safe_component(session_id) {
        return Err(DeleteError::UnsafeSessionId(session_id.to_owned()));
    }
    let exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)")
        .bind(session_id)
        .fetch_one(pool)
        .await?;
    if exists == 0 {
        return Err(DeleteError::NotFound(session_id.to_owned()));
    }

    let mut tombstones = read_tombstones(tombstone_file)?;
    let deleted_at = now_ms();
    tombstones
        .sessions
        .insert(session_id.to_owned(), deleted_at);
    write_tombstones(tombstone_file, &tombstones)?;

    let mut tx = pool.begin().await?;
    sqlx::query("INSERT OR REPLACE INTO session_tombstones(session_id, deleted_at) VALUES (?, ?)")
        .bind(session_id)
        .bind(deleted_at)
        .execute(&mut *tx)
        .await?;
    let agent_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM agents WHERE session_id = ?")
        .bind(session_id)
        .fetch_all(&mut *tx)
        .await?;
    for agent_id in agent_ids {
        sqlx::query("DELETE FROM approvals WHERE agent_id = ?")
            .bind(agent_id)
            .execute(&mut *tx)
            .await?;
    }
    for table in [
        "patches",
        "interventions",
        "checkpoints",
        "team_agents",
        "subagent_nodes",
        "turn_summaries",
        "session_skills",
        "bookmarks",
        "paused_turns",
        "turn_tombstones",
    ] {
        let query = format!("DELETE FROM {table} WHERE session_id = ?");
        sqlx::query(&query)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // Session-specific generated blobs live beside the database. Other project sessions are
    // never touched; shared content-addressed blobs remain owned by their global index.
    let owned = project_root
        .join(".rigel")
        .join("sessions")
        .join(session_id);
    if owned.is_dir() {
        fs::remove_dir_all(&owned).map_err(|source| DeleteError::Io {
            path: owned,
            source,
        })?;
    }
    Ok(())
}

pub fn read_tombstones(path: &Path) -> Result<Tombstones, DeleteError> {
    if !path.exists() {
        return Ok(Tombstones::default());
    }
    let bytes = fs::read(path).map_err(|source| DeleteError::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn write_tombstones(path: &Path, tombstones: &Tombstones) -> Result<(), DeleteError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DeleteError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(tombstones)?;
    fs::write(&temp, bytes).map_err(|source| DeleteError::Io {
        path: temp.clone(),
        source,
    })?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| DeleteError::Io {
            path: path.to_owned(),
            source,
        })?;
    }
    fs::rename(&temp, path).map_err(|source| DeleteError::Io {
        path: path.to_owned(),
        source,
    })
}
