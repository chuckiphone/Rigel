//! Committed memory, and the validator that keeps it safe to commit (P3.6, I7).
//!
//! Sessions are local because they contain prompts and machine paths; memory is committed
//! because it is the distilled thing that should travel. That split only holds if memory
//! cannot write a machine path or a secret: a committed file is forever, and a secret in git
//! is a secret leaked. So every write passes [`validate`], and a rejection names the pattern
//! and the line — never the matched content, which would put the secret in the error path.
//!
//! Ranking lives in [`rank`]: each non-heading line is a fact, and its score is explained from
//! recency, use and pin. The index here is the source of the pin and use counts, while the
//! committed files remain the source of the facts.

pub mod rank;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::migrate::now_ms;

/// §7.1's per-write cap.
pub const MAX_CONTENT: usize = 8_000;

pub const MEMORY_FILE: &str = "MEMORY.md";

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory would leak a machine path ({pattern}) on line {line}")]
    PathLeak { pattern: &'static str, line: usize },

    #[error("memory would leak a secret ({pattern}) on line {line}")]
    Secret { pattern: &'static str, line: usize },

    #[error("topic {0:?} must match ^[a-z0-9-]{{1,48}}$")]
    InvalidTopic(String),

    #[error("agent id {0:?} is not a safe directory name")]
    InvalidAgent(String),

    #[error("memory content is {size} bytes; the limit is {MAX_CONTENT}")]
    TooLarge { size: usize },

    #[error("the user did not approve this memory write; nothing was written")]
    NotApproved,

    #[error("memory fact {0:?} does not exist; refresh the ranking first")]
    UnknownFact(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl MemoryError {
    /// A leak is a policy refusal, not a bug in the caller's arguments.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::PathLeak { .. } | Self::Secret { .. } | Self::NotApproved => 3,
            Self::InvalidTopic(_) | Self::InvalidAgent(_) | Self::TooLarge { .. } => 1,
            Self::UnknownFact(_) => 1,
            Self::Io(_) | Self::Database(_) | Self::Json(_) => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryFile {
    pub agent_id: String,
    pub topic: String,
    /// Project-relative, so the record itself carries no machine path.
    pub path: String,
    pub sha256: String,
    pub words: usize,
    pub bytes: usize,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Append,
    Replace,
}

impl WriteMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "append" => Some(Self::Append),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Replace => "replace",
        }
    }
}

/// The exact bytes an approval is for. The write commits [`after`](Self::after), never a
/// recomputation, so a file that changes between the question and the click cannot make the
/// approved preview a lie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryPreview {
    pub before: String,
    pub after: String,
    /// A path-labelled line diff, enough for the card to show what will land.
    pub diff: String,
    pub sha256: String,
    pub bytes: usize,
}

/// Q251: one committed-memory write, exactly as the human sees it before it lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryRequest {
    pub agent_id: String,
    pub topic: String,
    pub content: String,
    pub mode: &'static str,
    /// The project-relative file the write targets.
    pub path: String,
    pub preview: MemoryPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDecision {
    Allow,
    Deny,
}

/// Who answers a [`MemoryRequest`]. Implementations must fail closed: an error, EOF or a
/// closed window denies.
#[async_trait]
pub trait MemoryApprover: Send + Sync {
    async fn approve(&self, request: &MemoryRequest) -> MemoryDecision;
}

/// Refuses every memory write. This is the default for a caller with no approval surface.
pub struct DenyMemory;

#[async_trait]
impl MemoryApprover for DenyMemory {
    async fn approve(&self, _request: &MemoryRequest) -> MemoryDecision {
        MemoryDecision::Deny
    }
}

/// Patterns from I7 and §7.1. The list is conservative: it is better to reject a sentence
/// that mentions `/home/` than to commit one that contains a real home directory.
const PATH_PATTERNS: &[(&str, &str)] = &[
    ("drive-letter", "C:\\"),
    ("unc", "\\\\"),
    ("users", "/Users/"),
    ("home", "/home/"),
    ("root-home", "/root/"),
    ("tilde-home", "~/"),
];

const SECRET_PATTERNS: &[(&str, &str)] = &[
    ("openai-key", "sk-"),
    ("stripe-key", "sk_live_"),
    ("github-token", "ghp_"),
    ("github-pat", "github_pat_"),
    ("gitlab-token", "glpat-"),
    ("slack-token", "xoxb-"),
    ("slack-token", "xoxp-"),
    ("aws-key", "AKIA"),
    ("ssh-public-key", "ssh-rsa"),
    ("ssh-public-key", "ssh-ed25519"),
    ("private-key", "BEGIN RSA PRIVATE KEY"),
    ("private-key", "BEGIN OPENSSH PRIVATE KEY"),
    ("private-key", "BEGIN EC PRIVATE KEY"),
];

/// Assignments catch the general case the prefixes miss: `OPENROUTER_API_KEY=…`.
const ASSIGNMENT_HINTS: &[&str] = &["api_key=", "apikey=", "token=", "secret=", "password="];

/// Reject anything that would put a machine path or a credential into a committed file.
pub fn validate(content: &str) -> Result<(), MemoryError> {
    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        let lower = line.to_ascii_lowercase();

        for (name, needle) in PATH_PATTERNS {
            // `C:\` only means a drive when a letter precedes it.
            let hit = if *name == "drive-letter" {
                line.as_bytes().windows(3).any(|w| {
                    w[0].is_ascii_alphabetic() && w[1] == b':' && (w[2] == b'\\' || w[2] == b'/')
                })
            } else {
                line.contains(needle)
            };
            if hit {
                return Err(MemoryError::PathLeak {
                    pattern: name,
                    line: line_no,
                });
            }
        }

        for (name, needle) in SECRET_PATTERNS {
            if line.contains(needle) {
                return Err(MemoryError::Secret {
                    pattern: name,
                    line: line_no,
                });
            }
        }
        if ASSIGNMENT_HINTS.iter().any(|hint| lower.contains(hint))
            && lower
                .split_once('=')
                .map(|(_, value)| value.trim().len() >= 8)
                .unwrap_or(false)
        {
            return Err(MemoryError::Secret {
                pattern: "credential-assignment",
                line: line_no,
            });
        }
    }
    Ok(())
}

pub struct MemoryService {
    root: PathBuf,
    pool: SqlitePool,
    approvals: Arc<dyn MemoryApprover>,
}

impl MemoryService {
    pub fn new(root: &Path, pool: SqlitePool) -> Self {
        Self {
            root: root.to_path_buf(),
            pool,
            approvals: Arc::new(DenyMemory),
        }
    }

    /// Install the Q251 approval surface. Without one every write is refused; validation still
    /// runs first, so a path leak or secret is reported as itself rather than as a refusal.
    pub fn with_approval(mut self, approver: Arc<dyn MemoryApprover>) -> Self {
        self.approvals = approver;
        self
    }

    fn agent_dir(&self, agent_id: &str) -> Result<PathBuf, MemoryError> {
        if !agent_ok(agent_id) {
            return Err(MemoryError::InvalidAgent(agent_id.to_string()));
        }
        Ok(self.root.join(".rigel").join("memory").join(agent_id))
    }

    fn path_for(&self, agent_id: &str, topic: &str) -> Result<(PathBuf, String), MemoryError> {
        if !topic_ok(topic) {
            return Err(MemoryError::InvalidTopic(topic.to_string()));
        }
        let dir = self.agent_dir(agent_id)?;
        let (file, relative) = if topic == "memory" {
            (dir.join(MEMORY_FILE), "MEMORY.md".to_string())
        } else {
            (
                dir.join("topics").join(format!("{topic}.md")),
                format!("topics/{topic}.md"),
            )
        };
        let relative = format!(".rigel/memory/{agent_id}/{}", relative.replace('\\', "/"));
        Ok((file, relative))
    }

    pub async fn write(
        &self,
        agent_id: &str,
        topic: &str,
        content: &str,
        mode: WriteMode,
    ) -> Result<MemoryFile, MemoryError> {
        if content.len() > MAX_CONTENT {
            return Err(MemoryError::TooLarge {
                size: content.len(),
            });
        }
        validate(content)?;

        let (path, relative) = self.path_for(agent_id, topic)?;
        let existing = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(MemoryError::Io(error)),
        };
        let after = composed(&existing, content, mode);
        let preview = MemoryPreview {
            before: existing.clone(),
            diff: preview_diff(&relative, &existing, &after),
            sha256: sha256_hex(after.as_bytes()),
            bytes: after.len(),
            after: after.clone(),
        };
        let request = MemoryRequest {
            agent_id: agent_id.to_string(),
            topic: topic.to_string(),
            content: content.to_string(),
            mode: mode.as_str(),
            path: relative.clone(),
            preview,
        };
        if self.approvals.approve(&request).await == MemoryDecision::Deny {
            return Err(MemoryError::NotApproved);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // The approved bytes, not a second composition: the preview is the contract.
        let body = request.preview.after;
        {
            let mut file = fs::File::create(&path)?;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
        }

        let updated_at = now_ms();
        let sha256 = sha256_hex(body.as_bytes());
        let words = body.split_whitespace().count();

        sqlx::query(
            "INSERT INTO memory_index (id, path, sha256, updated_at, words)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               path = excluded.path, sha256 = excluded.sha256,
               updated_at = excluded.updated_at, words = excluded.words",
        )
        .bind(format!("{agent_id}:{topic}"))
        .bind(&relative)
        .bind(&sha256)
        .bind(updated_at)
        .bind(words as i64)
        .execute(&self.pool)
        .await?;

        Ok(MemoryFile {
            agent_id: agent_id.to_string(),
            topic: topic.to_string(),
            path: relative,
            sha256,
            words,
            bytes: body.len(),
            updated_at,
        })
    }

    pub fn read(&self, agent_id: &str, topic: &str) -> Result<Option<String>, MemoryError> {
        let (path, _) = self.path_for(agent_id, topic)?;
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(MemoryError::Io(error)),
        }
    }

    /// Re-read the committed files and rank every fact. The files are the source of the text;
    /// this database keeps the pin and use state across a rewrite. A fact that left its file is
    /// removed from the index rather than ranked forever.
    pub async fn facts(&self, agent: Option<&str>) -> Result<Vec<rank::RankedFact>, MemoryError> {
        self.reindex(agent).await?;
        let sql = format!("SELECT {FACT_COLUMNS} FROM memory_facts");
        let rows = match agent {
            Some(agent) => {
                let filtered = format!("{sql} WHERE agent_id = ?");
                sqlx::query_as::<_, FactRow>(&filtered)
                    .bind(agent)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query_as::<_, FactRow>(&sql)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rank::rank(
            rows.into_iter().map(fact_from_row).collect(),
            now_ms(),
        ))
    }

    /// Pin or unpin one indexed fact. A pinned fact's recency value is held at full, so its
    /// score no longer decays.
    pub async fn pin_fact(&self, id: &str, pinned: bool) -> Result<rank::RankedFact, MemoryError> {
        let result = sqlx::query("UPDATE memory_facts SET pinned = ? WHERE id = ?")
            .bind(if pinned { 1i64 } else { 0 })
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(MemoryError::UnknownFact(id.to_string()));
        }
        let sql = format!("SELECT {FACT_COLUMNS} FROM memory_facts WHERE id = ?");
        let row = sqlx::query_as::<_, FactRow>(&sql)
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(rank::ranked(fact_from_row(row), now_ms()))
    }

    /// Return the facts a query matches, highest score first, and count each one as used.
    /// An empty query is a plain recall of the top `limit`, which is what the inspector's
    /// first page is.
    pub async fn recall(
        &self,
        agent: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<rank::RankedFact>, MemoryError> {
        let needle = query.trim().to_lowercase();
        let mut hits: Vec<rank::Fact> = self
            .facts(agent)
            .await?
            .into_iter()
            .map(|row| row.fact)
            .filter(|fact| needle.is_empty() || fact.text.to_lowercase().contains(&needle))
            .take(limit.clamp(1, 50))
            .collect();
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let used_at = now_ms();
        let mut tx = self.pool.begin().await?;
        for fact in &hits {
            sqlx::query("UPDATE memory_facts SET uses = uses + 1, last_used_at = ? WHERE id = ?")
                .bind(used_at)
                .bind(&fact.id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        for fact in &mut hits {
            fact.uses += 1;
            fact.last_used_at = Some(used_at);
        }
        Ok(rank::rank(hits, used_at))
    }

    /// Rebuild the fact index for one agent, or every agent on disk.
    async fn reindex(&self, only: Option<&str>) -> Result<(), MemoryError> {
        if let Some(agent) = only {
            if !agent_ok(agent) {
                return Err(MemoryError::InvalidAgent(agent.to_string()));
            }
            return self.reindex_agent(agent).await;
        }
        let base = self.root.join(".rigel").join("memory");
        let entries = match fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(agent) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if agent_ok(&agent) {
                self.reindex_agent(&agent).await?;
            }
        }
        Ok(())
    }

    async fn reindex_agent(&self, agent: &str) -> Result<(), MemoryError> {
        let dir = self.root.join(".rigel").join("memory").join(agent);
        let mut topics: Vec<(String, PathBuf)> = Vec::new();
        let main = dir.join(MEMORY_FILE);
        if main.is_file() {
            topics.push(("memory".to_string(), main));
        }
        match fs::read_dir(dir.join("topics")) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if path.extension().and_then(|value| value.to_str()) != Some("md") {
                        continue;
                    }
                    let Some(topic) = path.file_stem().and_then(|value| value.to_str()) else {
                        continue;
                    };
                    if topic_ok(topic) {
                        topics.push((topic.to_string(), path));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        for (topic, path) in topics {
            let body = fs::read_to_string(&path)?;
            let updated_at = fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_millis() as i64)
                .unwrap_or_else(now_ms);
            let facts = rank::facts_in(&body);
            let mut tx = self.pool.begin().await?;
            for (index, text) in facts.iter().enumerate() {
                let hash = rank::fact_hash(text);
                let id = format!("{agent}:{topic}:{hash}");
                sqlx::query(
                    "INSERT INTO memory_facts
                         (id, agent_id, topic, fact_hash, text, ordinal, pinned, uses, last_used_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, 0, 0, NULL, ?)
                     ON CONFLICT(id) DO UPDATE SET
                         text = excluded.text,
                         ordinal = excluded.ordinal,
                         updated_at = excluded.updated_at",
                )
                .bind(&id)
                .bind(agent)
                .bind(&topic)
                .bind(&hash)
                .bind(text)
                .bind((index + 1) as i64)
                .bind(updated_at)
                .execute(&mut *tx)
                .await?;
            }
            // A fact removed from the file leaves the index; its pin and use count go with it.
            if facts.is_empty() {
                sqlx::query("DELETE FROM memory_facts WHERE agent_id = ? AND topic = ?")
                    .bind(agent)
                    .bind(&topic)
                    .execute(&mut *tx)
                    .await?;
            } else {
                let placeholders = vec!["?"; facts.len()].join(",");
                let sql = format!(
                    "DELETE FROM memory_facts
                     WHERE agent_id = ? AND topic = ? AND fact_hash NOT IN ({placeholders})"
                );
                let mut query = sqlx::query(&sql).bind(agent).bind(&topic);
                for text in &facts {
                    query = query.bind(rank::fact_hash(text));
                }
                query.execute(&mut *tx).await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }
}

const FACT_COLUMNS: &str = "id, agent_id, topic, text, pinned, uses, last_used_at, updated_at";

/// The row shape `FactRow` needs. `sqlx` is built without its derive feature here, so the
/// mapping is an ordinary tuple and a plain function.
#[allow(clippy::type_complexity)]
type FactRow = (String, String, String, String, i64, i64, Option<i64>, i64);

fn fact_from_row(row: FactRow) -> rank::Fact {
    rank::Fact {
        id: row.0,
        agent_id: row.1,
        topic: row.2,
        text: row.3,
        pinned: row.4 != 0,
        uses: row.5.max(0) as u64,
        last_used_at: row.6,
        updated_at: row.7,
    }
}

fn agent_ok(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= 64
        && agent_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn topic_ok(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= 48
        && topic.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn composed(existing: &str, content: &str, mode: WriteMode) -> String {
    match mode {
        WriteMode::Replace => content.to_string(),
        WriteMode::Append if existing.is_empty() => content.to_string(),
        WriteMode::Append if existing.ends_with('\n') => format!("{existing}\n{content}"),
        WriteMode::Append => format!("{existing}\n\n{content}"),
    }
}

fn preview_diff(path: &str, before: &str, after: &str) -> String {
    if before == after {
        return format!("--- a/{path}\n+++ b/{path}\n(no change)\n");
    }
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if !before.is_empty() {
        for line in before.lines() {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
    }
    if !after.is_empty() {
        for line in after.lines() {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::session::SessionService;

    struct Recorder {
        decision: MemoryDecision,
        seen: Mutex<Vec<MemoryRequest>>,
    }

    #[async_trait]
    impl MemoryApprover for Recorder {
        async fn approve(&self, request: &MemoryRequest) -> MemoryDecision {
            self.seen.lock().unwrap().push(request.clone());
            self.decision
        }
    }

    async fn service(dir: &tempfile::TempDir) -> MemoryService {
        let sessions = SessionService::open(dir.path()).await.unwrap();
        MemoryService::new(dir.path(), sessions.pool().clone())
    }

    #[tokio::test]
    async fn the_default_is_deny_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        let error = service
            .write("agent", "notes", "a safe line", WriteMode::Append)
            .await
            .unwrap_err();
        assert!(matches!(error, MemoryError::NotApproved));
        assert!(
            !dir.path()
                .join(".rigel/memory/agent/topics/notes.md")
                .exists()
        );
    }

    #[tokio::test]
    async fn the_approved_bytes_are_exactly_the_previewed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".rigel/memory/agent/topics/notes.md");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "old line\n").unwrap();

        let recorder = Arc::new(Recorder {
            decision: MemoryDecision::Allow,
            seen: Mutex::new(Vec::new()),
        });
        let service = service(&dir).await.with_approval(recorder.clone());
        let written = service
            .write("agent", "notes", "new line", WriteMode::Append)
            .await
            .unwrap();

        let request = recorder.seen.lock().unwrap()[0].clone();
        assert_eq!(request.preview.before, "old line\n");
        assert_eq!(request.preview.after, "old line\n\nnew line");
        assert!(request.preview.diff.contains("-old line"));
        assert!(request.preview.diff.contains("+new line"));
        assert_eq!(request.preview.bytes, request.preview.after.len());

        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            on_disk, request.preview.after,
            "the approval is the contract"
        );
        assert_eq!(written.sha256, sha256_hex(on_disk.as_bytes()));
        assert_eq!(written.sha256, request.preview.sha256);
    }

    #[test]
    fn ordinary_prose_passes() {
        validate("The parser lives in crates/rigel-core and uses sqlx.\n").unwrap();
        validate("Use `rigel skills search pdf` before adding a dependency.\n").unwrap();
    }

    #[test]
    fn a_windows_path_is_named_but_not_echoed() {
        let error = validate("root: C:\\Users\\someone\\project").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("drive-letter"));
        assert!(
            !message.contains("someone"),
            "the error must not repeat the path"
        );
    }

    #[test]
    fn home_directories_are_rejected() {
        assert!(validate("see /Users/someone/x").is_err());
        assert!(validate("see /home/someone/x").is_err());
        assert!(validate("see ~/x").is_err());
    }

    #[test]
    fn secrets_and_assignments_are_rejected() {
        assert!(validate("key sk-live-abcdef").is_err());
        assert!(validate("aws AKIAIOSFODNN7EXAMPLE").is_err());
        assert!(validate("OPENROUTER_API_KEY=abcdefgh12345678").is_err());
        assert!(validate("ssh-rsa AAAAB3NzaC1yc2E").is_err());
    }

    fn seed(dir: &tempfile::TempDir, agent: &str, topic: &str, body: &str) {
        let root = dir.path().join(".rigel/memory").join(agent);
        let path = if topic == "memory" {
            root.join(MEMORY_FILE)
        } else {
            root.join("topics").join(format!("{topic}.md"))
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn facts_are_ranked_from_the_files_and_a_pin_survives_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        seed(
            &dir,
            "worker",
            "memory",
            "# Notes\n- Uses sqlx.\n- Never force-push.\n",
        );
        seed(&dir, "worker", "build", "Build with cargo.\n");

        let facts = service.facts(None).await.unwrap();
        assert_eq!(facts.len(), 3, "{facts:?}");
        assert!(facts.iter().all(|row| !row.fact.pinned));
        assert!(facts.iter().all(|row| row.fact.uses == 0));
        assert!(
            facts.iter().all(|row| row.score.factors.len() == 3),
            "every fact names recency, use and pin"
        );

        let pinned = facts
            .iter()
            .find(|row| row.fact.text == "Never force-push.")
            .expect("the fact");
        let after = service.pin_fact(&pinned.fact.id, true).await.unwrap();
        assert!(after.fact.pinned);
        assert_eq!(after.score.factors[2].value, 1.0);
        assert!(after.score.total > pinned.score.total);

        // A rewrite keeps the same fact id, so the pin is still on it.
        seed(
            &dir,
            "worker",
            "memory",
            "# Notes\n- Uses sqlx.\n- Never force-push.\n- Run cargo fmt.\n",
        );
        let again = service.facts(None).await.unwrap();
        let pinned = again
            .iter()
            .find(|row| row.fact.text == "Never force-push.")
            .expect("the rewritten fact");
        assert!(pinned.fact.pinned, "the pin outlives the file rewrite");
        assert_eq!(again.len(), 4);
    }

    #[tokio::test]
    async fn recall_counts_only_the_facts_it_returns_and_raises_their_use_factor() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        seed(
            &dir,
            "worker",
            "memory",
            "- Uses sqlx.\n- Build with cargo.\n",
        );

        let first = service.recall(None, "sqlx", 5).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].fact.uses, 1);
        let second = service.recall(None, "SQLX", 5).await.unwrap();
        assert_eq!(second[0].fact.uses, 2, "the query is case-insensitive");
        assert!(second[0].score.factors[1].value > first[0].score.factors[1].value);

        let cold = service.facts(None).await.unwrap();
        let cargo = cold
            .iter()
            .find(|row| row.fact.text.contains("cargo"))
            .expect("the other fact");
        assert_eq!(cargo.fact.uses, 0, "a fact not returned was not used");
    }

    #[tokio::test]
    async fn a_fact_removed_from_the_file_leaves_the_ranking() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        seed(&dir, "worker", "memory", "- One.\n- Two.\n");
        assert_eq!(service.facts(None).await.unwrap().len(), 2);

        seed(&dir, "worker", "memory", "- One.\n");
        let facts = service.facts(None).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact.text, "One.");

        assert!(matches!(
            service.pin_fact("worker:memory:missing", true).await,
            Err(MemoryError::UnknownFact(_))
        ));
    }

    #[tokio::test]
    async fn an_empty_project_ranks_no_facts() {
        let dir = tempfile::tempdir().unwrap();
        let service = service(&dir).await;
        assert!(service.facts(None).await.unwrap().is_empty());
        assert!(service.recall(None, "", 10).await.unwrap().is_empty());
    }
}
