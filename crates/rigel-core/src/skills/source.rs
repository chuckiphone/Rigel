//! The remote-source contract.
//!
//! A `Source` knows how to talk to one place skills come from, and nothing else: it does not
//! write the index and it does not decide what the UI shows. `github` implements repo →
//! skills; `hub` implements the two marketplaces, which can be searched but not walked.
//!
//! Every method has a default that reports "not supported by this source" rather than an
//! `unimplemented!()`, so adding a source means implementing the two or three methods it
//! actually has an answer for.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::skills::{Capability, SkillsError};

/// The finite set of places the skill index knows how to refresh. Persisted ids are kept for
/// compatibility, but dispatch never matches on an arbitrary string: an id is parsed into this
/// type first, and an unknown id is reported as having no rule instead of reaching a catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Github,
    SkillHub,
    SkillsMp,
}

impl SourceKind {
    pub const ALL: [SourceKind; 3] = [
        SourceKind::Github,
        SourceKind::SkillHub,
        SourceKind::SkillsMp,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SourceKind::Github => "github",
            SourceKind::SkillHub => "skillhub",
            SourceKind::SkillsMp => "skillsmp",
        }
    }

    pub fn base_url(self) -> &'static str {
        match self {
            SourceKind::Github => "https://api.github.com",
            SourceKind::SkillHub => "https://skills.browseract.com",
            SourceKind::SkillsMp => "https://skillsmp.com",
        }
    }

    /// Parse a persisted source id. `None` means another surface wrote the row; refresh reports
    /// that explicitly instead of guessing at a handler.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncPolicy {
    /// Ignore the freshness window and revalidate every cached URL, conditional where the
    /// server gave us an ETag. Never implies "ignore the cache": a `304` still saves the body.
    pub force: bool,
}

impl SyncPolicy {
    pub fn normal() -> Self {
        Self { force: false }
    }

    pub fn forced() -> Self {
        Self { force: true }
    }
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self::normal()
    }
}

/// One row of the index, before it is written.
#[derive(Debug, Clone)]
pub struct SkillRecord {
    /// `source:repo:path` for GitHub, `source:slug` for a marketplace.
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub url: Option<String>,
    pub body_sha: Option<String>,
    pub grade: Option<String>,
    pub grade_scope: Option<String>,
    pub license: Option<String>,
    pub stars: Option<i64>,
    pub installs: Option<i64>,
    pub tokens: Option<i64>,
    pub capabilities: Vec<Capability>,
    pub simhash: Option<i64>,
}

impl SkillRecord {
    pub fn source_id(&self) -> &str {
        &self.source
    }

    pub fn capabilities_json(&self) -> Option<String> {
        serde_json::to_string(&self.capabilities).ok()
    }

    /// Frontmatter plus body come to roughly four characters per token, which is the same
    /// estimate the prototype used and is only ever shown as "about".
    pub fn estimate_tokens(body: &str) -> i64 {
        ((body.len() as f64 / 4.0).round() as i64).max(120)
    }
}

#[derive(Debug, Clone)]
pub struct FetchedSkill {
    pub record: SkillRecord,
    pub body: String,
}

/// The result of walking one repository.
#[derive(Debug, Clone)]
pub struct RepoSnapshot {
    pub source: String,
    pub repo: String,
    pub branch: String,
    pub stars: i64,
    pub license: Option<String>,
    pub truncated: bool,
    pub found: usize,
    /// HTTP requests this walk actually made. `0` on a re-index inside the freshness window,
    /// which is exactly what P2.11 asserts.
    pub http_requests: usize,
    pub skills: Vec<FetchedSkill>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceState {
    Ok,
    Stale,
    Broken,
}

impl SourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceState::Ok => "ok",
            SourceState::Stale => "stale",
            SourceState::Broken => "broken",
        }
    }
}

/// What the shelf's status line shows. `last_sync_at` is kept even when the source is broken:
/// "last reached GitHub 3h ago" is more useful than "broken". `last_attempt_at` is when the
/// checker last tried, so Settings can show that it is still working; `failures` is the
/// consecutive-failure count that spaces out retries of a dead endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub id: String,
    pub base_url: Option<String>,
    pub state: SourceState,
    pub last_sync_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub failures: u32,
    pub error: Option<String>,
}

/// Read one source's health from the `sources` row and the clock.
///
/// The three states mean something specific to the user:
/// * `ok` — synced inside the last [`crate::skills::STALE_AFTER_MS`];
/// * `stale` — reachable last time, but the data is older than that, or has never been synced;
/// * `broken` — the last attempt failed; `error` says how, `last_sync_at` says when it worked.
///
/// It never makes a network call, because the status line must render offline.
pub async fn load_status(
    pool: &sqlx::SqlitePool,
    id: &str,
    base_url: Option<&str>,
) -> Result<SourceStatus, sqlx::Error> {
    type SourceRow = (
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<i64>,
        i64,
    );
    let row: Option<SourceRow> = sqlx::query_as(
        "SELECT last_sync_at, status, error, last_attempt_at, failures FROM sources WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let now = crate::migrate::now_ms();
    let (last_sync_at, state, error, last_attempt_at, failures) = match row {
        None => (None, SourceState::Stale, None, None, 0),
        Some((last_sync_at, status, error, last_attempt_at, failures)) => {
            let state = match status.as_deref() {
                Some("broken") => SourceState::Broken,
                Some("ok")
                    if last_sync_at.is_some_and(|at| now - at < crate::skills::STALE_AFTER_MS) =>
                {
                    SourceState::Ok
                }
                _ => SourceState::Stale,
            };
            (last_sync_at, state, error, last_attempt_at, failures)
        }
    };

    Ok(SourceStatus {
        id: id.to_string(),
        base_url: base_url.map(ToOwned::to_owned),
        state,
        last_sync_at,
        last_attempt_at,
        failures: failures.max(0) as u32,
        error,
    })
}

#[async_trait]
pub trait Source: Send + Sync {
    fn kind(&self) -> SourceKind;

    fn id(&self) -> &'static str {
        self.kind().id()
    }

    /// Source health, read from the `sources` row and the clock — never from a network call.
    async fn status(&self) -> Result<SourceStatus, SkillsError>;

    /// Full-text search over the source itself. Marketplaces have one; GitHub does not.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillRecord>, SkillsError> {
        let _ = (query, limit);
        Err(SkillsError::Unsupported("search", self.id()))
    }

    /// Walk a repository (or a directory inside it) and return every skill it contains.
    async fn fetch_repo(
        &self,
        target: &str,
        policy: SyncPolicy,
    ) -> Result<RepoSnapshot, SkillsError> {
        let _ = (target, policy);
        Err(SkillsError::Unsupported("fetch_repo", self.id()))
    }

    /// One skill's body, by index id.
    async fn fetch_skill(&self, id: &str) -> Result<FetchedSkill, SkillsError> {
        let _ = id;
        Err(SkillsError::Unsupported("fetch_skill", self.id()))
    }
}

/// The repository-walk seam used by [`crate::skills::SkillIndex`]. Production wraps
/// [`crate::skills::GitHubSource`]; a test can inject a snapshot without a socket. Keeping the
/// per-call `slug` and `limit` here preserves the same source configuration the GitHub walker
/// uses, instead of only swapping the client underneath it.
#[async_trait]
pub trait RepoFetcher: Send + Sync {
    async fn fetch_repo(
        &self,
        target: &str,
        policy: SyncPolicy,
        slug: Option<&str>,
        limit: Option<usize>,
    ) -> Result<RepoSnapshot, SkillsError>;
}

#[cfg(test)]
mod tests {
    use super::SourceKind;

    #[test]
    fn every_persisted_source_id_round_trips_and_unknown_ids_stay_unknown() {
        for kind in SourceKind::ALL {
            assert_eq!(SourceKind::from_id(kind.id()), Some(kind));
            assert!(!kind.base_url().is_empty());
        }
        assert_eq!(SourceKind::from_id(""), None);
        assert_eq!(SourceKind::from_id("local"), None);
        assert_eq!(SourceKind::from_id("GitHub"), None);
    }
}
