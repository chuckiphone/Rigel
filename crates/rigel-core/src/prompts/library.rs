use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::migrate::now_ms;
use crate::storage::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub version: i64,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPin {
    pub turn_id: String,
    pub session_id: String,
    pub name: String,
    pub version: i64,
    pub snapshot: String,
    pub variables: BTreeMap<String, String>,
    pub pinned_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("prompt {name:?} version {version} was not found")]
    NotFound { name: String, version: i64 },
    #[error("prompt name must not be empty")]
    EmptyName,
    #[error("prompt variable {0:?} was not provided")]
    MissingVariable(String),
    #[error("turn {0:?} was not found")]
    TurnNotFound(String),
    #[error("turn {turn_id:?} does not belong to session {session_id:?}")]
    TurnSessionMismatch { turn_id: String, session_id: String },
}

/// Owns the project's prompt versions and the immutable copies pinned to turns.
#[derive(Clone)]
pub struct PromptLibrary {
    pool: SqlitePool,
}

impl PromptLibrary {
    pub async fn open(project_root: &Path) -> Result<Self, PromptError> {
        let store = Store::open_project(project_root).await?;
        Ok(Self::from_pool(store.pool().clone()))
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Save a new immutable version. Each edit increments the version for that name.
    pub async fn save(&self, name: &str, body: &str) -> Result<Prompt, PromptError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PromptError::EmptyName);
        }
        let mut tx = self.pool.begin().await?;
        let current: Option<i64> =
            sqlx::query_scalar("SELECT MAX(version) FROM prompt_versions WHERE name = ?")
                .bind(name)
                .fetch_one(&mut *tx)
                .await?;
        let version = current.unwrap_or(0) + 1;
        let created_at = now_ms();
        sqlx::query(
            "INSERT INTO prompt_versions (name, version, body, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(name)
        .bind(version)
        .bind(body)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Prompt {
            name: name.to_owned(),
            version,
            body: body.to_owned(),
            created_at,
        })
    }

    pub async fn list(&self) -> Result<Vec<Prompt>, PromptError> {
        let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
            "SELECT p.name, p.version, p.body, p.created_at
             FROM prompt_versions p
             JOIN (SELECT name, MAX(version) AS version FROM prompt_versions GROUP BY name) latest
               ON latest.name = p.name AND latest.version = p.version
             ORDER BY p.name COLLATE NOCASE, p.name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(prompt_from_row).collect())
    }

    pub async fn get(&self, name: &str, version: Option<i64>) -> Result<Prompt, PromptError> {
        let row: Option<(String, i64, String, i64)> = match version {
            Some(version) => sqlx::query_as(
                "SELECT name, version, body, created_at FROM prompt_versions WHERE name = ? AND version = ?",
            )
            .bind(name)
            .bind(version)
            .fetch_optional(&self.pool)
            .await?,
            None => sqlx::query_as(
                "SELECT name, version, body, created_at FROM prompt_versions WHERE name = ? ORDER BY version DESC LIMIT 1",
            )
            .bind(name)
            .fetch_optional(&self.pool)
            .await?,
        };
        row.map(prompt_from_row)
            .ok_or_else(|| PromptError::NotFound {
                name: name.to_owned(),
                version: version.unwrap_or(0),
            })
    }

    pub async fn render(
        &self,
        name: &str,
        version: Option<i64>,
        variables: &BTreeMap<String, String>,
    ) -> Result<String, PromptError> {
        interpolate(&self.get(name, version).await?.body, variables)
    }

    pub async fn diff(&self, name: &str, from: i64, to: i64) -> Result<String, PromptError> {
        let old = self.get(name, Some(from)).await?;
        let new = self.get(name, Some(to)).await?;
        let mut out = format!("--- {name}@{from}\n+++ {name}@{to}\n");
        let old_lines: Vec<_> = old.body.lines().collect();
        let new_lines: Vec<_> = new.body.lines().collect();
        for (index, line) in old_lines.iter().enumerate() {
            if new_lines.get(index) != Some(line) {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
        }
        for (index, line) in new_lines.iter().enumerate() {
            if old_lines.get(index) != Some(line) {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// Pin an expanded prompt to a turn. Transcript reads prefer this snapshot over the
    /// mutable turn prompt, so later library edits cannot rewrite history.
    pub async fn pin(
        &self,
        session_id: &str,
        turn_id: &str,
        name: &str,
        version: Option<i64>,
        variables: &BTreeMap<String, String>,
    ) -> Result<PromptPin, PromptError> {
        let prompt = self.get(name, version).await?;
        let snapshot = interpolate(&prompt.body, variables)?;
        let mut tx = self.pool.begin().await?;
        let owner: Option<String> = sqlx::query_scalar(
            "SELECT a.session_id FROM turns t JOIN agents a ON a.id = t.agent_id WHERE t.id = ?",
        )
        .bind(turn_id)
        .fetch_optional(&mut *tx)
        .await?;
        match owner {
            None => return Err(PromptError::TurnNotFound(turn_id.to_owned())),
            Some(owner) if owner != session_id => {
                return Err(PromptError::TurnSessionMismatch {
                    turn_id: turn_id.to_owned(),
                    session_id: session_id.to_owned(),
                });
            }
            Some(_) => {}
        }
        let pinned_at = now_ms();
        let variables_json = serde_json::to_string(variables)
            .map_err(|_| PromptError::Database(sqlx::Error::RowNotFound))?;
        sqlx::query(
            "INSERT INTO prompt_pins (turn_id, session_id, name, version, snapshot, variables_json, pinned_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(turn_id) DO UPDATE SET session_id=excluded.session_id, name=excluded.name,
               version=excluded.version, snapshot=excluded.snapshot, variables_json=excluded.variables_json,
               pinned_at=excluded.pinned_at",
        )
        .bind(turn_id)
        .bind(session_id)
        .bind(name)
        .bind(prompt.version)
        .bind(&snapshot)
        .bind(variables_json)
        .bind(pinned_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(PromptPin {
            turn_id: turn_id.to_owned(),
            session_id: session_id.to_owned(),
            name: name.to_owned(),
            version: prompt.version,
            snapshot,
            variables: variables.clone(),
            pinned_at,
        })
    }

    pub async fn pin_for_turn(&self, turn_id: &str) -> Result<Option<PromptPin>, PromptError> {
        let row: Option<(String, String, String, i64, String, String, i64)> = sqlx::query_as(
            "SELECT turn_id, session_id, name, version, snapshot, variables_json, pinned_at
             FROM prompt_pins WHERE turn_id = ?",
        )
        .bind(turn_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(
            |(turn_id, session_id, name, version, snapshot, variables_json, pinned_at)| {
                let variables = serde_json::from_str(&variables_json).map_err(|error| {
                    PromptError::Database(sqlx::Error::Protocol(error.to_string()))
                })?;
                Ok(PromptPin {
                    turn_id,
                    session_id,
                    name,
                    version,
                    snapshot,
                    variables,
                    pinned_at,
                })
            },
        )
        .transpose()
    }
}

fn prompt_from_row((name, version, body, created_at): (String, i64, String, i64)) -> Prompt {
    Prompt {
        name,
        version,
        body,
        created_at,
    }
}

fn interpolate(body: &str, variables: &BTreeMap<String, String>) -> Result<String, PromptError> {
    let mut output = String::with_capacity(body.len());
    let mut remaining = body;
    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let after_open = &remaining[start + 2..];
        let end = after_open
            .find("}}")
            .ok_or_else(|| PromptError::MissingVariable(after_open.to_owned()))?;
        let key = after_open[..end].trim();
        let value = variables
            .get(key)
            .ok_or_else(|| PromptError::MissingVariable(key.to_owned()))?;
        output.push_str(value);
        remaining = &after_open[end + 2..];
    }
    output.push_str(remaining);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionService;

    async fn library() -> (tempfile::TempDir, PromptLibrary) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_project(dir.path()).await.unwrap();
        let library = PromptLibrary::from_pool(store.pool().clone());
        (dir, library)
    }

    #[tokio::test]
    async fn versions_render_variables_and_diff_without_mutating_old_versions() {
        let (_dir, library) = library().await;
        let first = library
            .save("review", "Review {{path}}\nKeep safe")
            .await
            .unwrap();
        let second = library
            .save("review", "Review {{path}}\nKeep secure")
            .await
            .unwrap();
        assert_eq!((first.version, second.version), (1, 2));
        let vars = BTreeMap::from([("path".to_owned(), "src/lib.rs".to_owned())]);
        assert_eq!(
            library.render("review", Some(1), &vars).await.unwrap(),
            "Review src/lib.rs\nKeep safe"
        );
        let diff = library.diff("review", 1, 2).await.unwrap();
        assert!(diff.contains("-Keep safe\n+Keep secure\n"));
    }

    #[tokio::test]
    async fn pinned_prompt_snapshot_is_the_transcript_prompt_after_library_edit() {
        let (dir, library) = library().await;
        let prompt = library.save("welcome", "Hello {{name}}").await.unwrap();
        let service = SessionService::open(dir.path()).await.unwrap();
        let session = service.start(Some("prompt pin"), None).await.unwrap();
        let agent = service
            .register_agent(&session.id, "native", None)
            .await
            .unwrap();
        let turn = service
            .append_turn(&agent.id, Some("original"))
            .await
            .unwrap();
        let vars = BTreeMap::from([("name".to_owned(), "Ada".to_owned())]);
        let pinned = library
            .pin(
                &session.id,
                &turn.id,
                "welcome",
                Some(prompt.version),
                &vars,
            )
            .await
            .unwrap();
        library.save("welcome", "Goodbye {{name}}").await.unwrap();
        assert_eq!(
            library
                .pin_for_turn(&turn.id)
                .await
                .unwrap()
                .unwrap()
                .snapshot,
            "Hello Ada"
        );
        assert_eq!(
            service.transcript(&session.id).await.unwrap()[0]
                .turn
                .prompt
                .as_deref(),
            Some("Hello Ada")
        );
        assert_eq!(pinned.snapshot, "Hello Ada");
    }
}
