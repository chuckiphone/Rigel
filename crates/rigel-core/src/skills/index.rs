//! `SkillIndex` — the one place that writes the index.
//!
//! Ordering inside an ingest is the storage rule from §6.4: **blob first, then row**. A crash
//! between the two leaves an orphan body that `rigel gc` collects; the other order leaves a
//! row pointing at a body that never existed, which is unrecoverable without a re-fetch.
//!
//! Everything here is offline-capable by construction: `search`, `get` and `status` read only
//! SQLite and the blob directory, and take no `Source`. Only `index_repo` and the marketplace
//! sync methods touch the network.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use super::blob::BlobStore;
use super::cluster;
use super::evidence::{Evidence, EvidenceSummary, claims_from_description};
use super::github::{GitHubSource, parse_repo_input};
use super::hub::{SkillHubSource, SkillsMpSource};
use super::scan::Capability;
use super::scanner::{
    CollectionScan, ScanVerdict, SkillScanner, SkillSpector, body_scan_dir, canonical_target,
};
use super::search::{SearchHit, SearchOrder, fts_query};
use super::source::{
    RepoFetcher, RepoSnapshot, SkillRecord, Source, SourceKind, SourceState, SourceStatus,
    SyncPolicy, load_status,
};
use crate::migrate::now_ms;
use crate::skills::{DEFAULT_TTL, SkillsError};
use crate::storage::Store;

pub struct SkillIndex {
    store: Store,
    blobs: BlobStore,
    scanner: Arc<dyn SkillScanner>,
    fetcher: Arc<dyn RepoFetcher>,
}

/// The production repository walker. It owns the same pool, blob store and freshness policy as
/// `SkillIndex`; tests replace the whole walker, not only its HTTP client.
struct GitHubRepoFetcher {
    pool: sqlx::SqlitePool,
    blobs: BlobStore,
    ttl: Duration,
}

#[async_trait]
impl RepoFetcher for GitHubRepoFetcher {
    async fn fetch_repo(
        &self,
        target: &str,
        policy: SyncPolicy,
        slug: Option<&str>,
        limit: Option<usize>,
    ) -> Result<RepoSnapshot, SkillsError> {
        let mut source = GitHubSource::new(self.pool.clone(), self.blobs.clone(), self.ttl).await?;
        if let Some(slug) = slug {
            source = source.with_slug(slug);
        }
        if let Some(limit) = limit {
            source = source.with_limit(limit);
        }
        source.fetch_repo(target, policy).await
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestReport {
    pub source: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub found: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub with_body: usize,
    pub http_requests: usize,
    pub truncated: bool,
    pub clusters: usize,
    /// The worst accepted body verdict. `None` when a caller handed `ingest` an already-fetched
    /// snapshot; blocked bodies never reach this report because they are not ingested.
    pub scan: Option<ScanVerdict>,
    /// Skills whose exact body SkillSpector blocked. They are not written to the index.
    pub blocked: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedReport {
    /// Rows present in the reply.
    pub rows: usize,
    /// Rows written to this index.
    pub written: usize,
    /// Rows without an id, which the publisher should never have written.
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub invalid: usize,
}

/// What one install wrote. A skill is copied into every requested harness store so the same
/// index row works for Claude and for the shared `.agents` readers.
#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    pub name: String,
    pub directory: String,
    pub paths: Vec<PathBuf>,
    /// True when an existing `SKILL.md` was replaced with a different body.
    pub replaced: bool,
    /// The pre-install scan of the exact bytes written.
    pub scan: ScanVerdict,
}

/// One `rigel skills sweep` — both marketplace walks, their row counts, and what the
/// meta-only duplicate pass dropped. Errors are fields, not aborts: a half-swept index
/// is still an index.
#[derive(Debug, Clone, Serialize)]
pub struct SweepReport {
    pub skillhub_pages: usize,
    pub skillhub_rows: usize,
    pub skillhub_error: Option<String>,
    pub skillsmp_queries: usize,
    pub skillsmp_pages: usize,
    pub skillsmp_rows: usize,
    pub skillsmp_error: Option<String>,
    pub duplicates_dropped: i64,
    pub skills_before: i64,
    pub skills_after: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantRow {
    pub id: String,
    pub name: String,
    pub repo: Option<String>,
    pub grade: Option<String>,
    pub license: Option<String>,
    pub last_seen_at: Option<i64>,
    /// A human sentence for the disclosure: "identical content", "3 bits away", "no license".
    pub delta: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillDetail {
    pub skill: SearchHit,
    pub capabilities: Vec<Capability>,
    pub evidence: Vec<EvidenceSummary>,
    pub variants: Vec<VariantRow>,
    pub first_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexStats {
    pub skills: i64,
    pub with_body: i64,
    pub clusters: i64,
    pub blobs: usize,
    pub blob_bytes: u64,
    pub sources: Vec<SourceStatus>,
}

impl SkillIndex {
    pub async fn open(data_dir: &Path) -> Result<Self, SkillsError> {
        let store = Store::open_index(data_dir).await?;
        Ok(Self::from_store(store, data_dir))
    }

    pub fn from_store(store: Store, data_dir: &Path) -> Self {
        let blobs = BlobStore::new(data_dir);
        let fetcher = Arc::new(GitHubRepoFetcher {
            pool: store.pool().clone(),
            blobs: blobs.clone(),
            ttl: DEFAULT_TTL,
        });
        Self {
            store,
            blobs,
            scanner: Arc::new(SkillSpector::default()),
            fetcher,
        }
    }

    /// Replace the scanner. Production uses [`SkillSpector`]; tests inject a fake so the
    /// gate's ordering can be asserted without installing Python or reaching GitHub.
    pub fn with_scanner(mut self, scanner: Arc<dyn SkillScanner>) -> Self {
        self.scanner = scanner;
        self
    }

    /// Replace the repository walker. The gate, the write order and the index itself stay the
    /// production implementation; only the fetched snapshot is injected.
    pub fn with_fetcher(mut self, fetcher: Arc<dyn RepoFetcher>) -> Self {
        self.fetcher = fetcher;
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn data_dir(&self) -> PathBuf {
        self.store
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    pub async fn github(&self) -> Result<GitHubSource, SkillsError> {
        GitHubSource::new(self.store.pool().clone(), self.blobs.clone(), DEFAULT_TTL).await
    }

    pub async fn skillsmp(&self) -> Result<SkillsMpSource, SkillsError> {
        SkillsMpSource::new(self.store.pool().clone(), self.blobs.clone(), DEFAULT_TTL).await
    }

    pub async fn skillhub(&self) -> Result<SkillHubSource, SkillsError> {
        SkillHubSource::new(self.store.pool().clone(), self.blobs.clone(), DEFAULT_TTL).await
    }

    /// Walk a repository and write what it finds. Marks `github` broken on failure, so the
    /// shelf can say "last reached 3h ago" instead of showing a stale list as if it were live.
    pub async fn index_repo(
        &self,
        target: &str,
        policy: SyncPolicy,
    ) -> Result<IngestReport, SkillsError> {
        self.walk_repo(target, policy, None, None).await
    }

    /// The same walk with the CLI's per-repository body cap.
    pub async fn index_repo_with_limit(
        &self,
        target: &str,
        policy: SyncPolicy,
        limit: usize,
    ) -> Result<IngestReport, SkillsError> {
        self.walk_repo(target, policy, None, Some(limit)).await
    }

    /// Index one skill by its directory name. The skills.sh import uses this so a page costs
    /// one body, not a whole repository's worth of requests.
    pub async fn index_skill(
        &self,
        target: &str,
        policy: SyncPolicy,
        slug: &str,
    ) -> Result<IngestReport, SkillsError> {
        self.walk_repo(target, policy, Some(slug), None).await
    }

    async fn walk_repo(
        &self,
        target: &str,
        policy: SyncPolicy,
        slug: Option<&str>,
        limit: Option<usize>,
    ) -> Result<IngestReport, SkillsError> {
        // The target preflight runs before `github()` constructs a source and before any
        // metadata or body request. An explicit single-skill URL can be refused here. A base
        // repository's aggregate report is not used to reject every skill: after the walk, each
        // exact body is scanned independently and only blocked skills are dropped.
        let explicit_skill = parse_repo_input(target)
            .map(|repo| repo.git_ref.is_some() && repo.path.is_some())
            .unwrap_or(false);
        let preflight = self.scanner.scan(&canonical_target(target)).await?;
        if preflight.blocked() && explicit_skill {
            return Err(SkillsError::ScannerBlocked {
                target: target.to_string(),
                score: preflight.score,
                recommendation: preflight.recommendation,
            });
        }
        if preflight.blocked() {
            eprintln!(
                "rigel: repository preflight is {}; each exact SKILL.md body is scanned before indexing",
                preflight.summary()
            );
        }

        match self.fetcher.fetch_repo(target, policy, slug, limit).await {
            Ok(mut snapshot) => {
                let (scan, blocked) = self
                    .gate_snapshot(&mut snapshot, explicit_skill.then_some(&preflight))
                    .await?;
                let mut report = self.ingest(&snapshot).await?;
                report.scan = Some(scan);
                report.blocked = blocked;
                Ok(report)
            }
            Err(error) => {
                let _ = self
                    .set_source(
                        SourceKind::Github.id(),
                        SourceKind::Github.base_url(),
                        SourceState::Broken,
                        Some(&error.to_string()),
                    )
                    .await;
                Err(error)
            }
        }
    }

    pub async fn ingest(&self, snapshot: &RepoSnapshot) -> Result<IngestReport, SkillsError> {
        let mut added = 0;
        let mut updated = 0;
        let mut unchanged = 0;

        for skill in &snapshot.skills {
            // §6.4: the body is on disk before the row that names it.
            let sha = self.blobs.put(skill.body.as_bytes())?;
            let mut record = skill.record.clone();
            record.body_sha = Some(sha.clone());

            let previous: Option<String> =
                sqlx::query_scalar("SELECT body_sha FROM skills WHERE id = ?")
                    .bind(&record.id)
                    .fetch_optional(self.store.pool())
                    .await?;

            match previous.as_deref() {
                None => added += 1,
                Some(prev) if prev == sha => unchanged += 1,
                Some(_) => updated += 1,
            }

            self.write_skill(&record).await?;

            sqlx::query(
                "INSERT OR IGNORE INTO skill_versions (skill_id, body_sha, fetched_at)
                 VALUES (?, ?, ?)",
            )
            .bind(&record.id)
            .bind(&sha)
            .bind(now_ms())
            .execute(self.store.pool())
            .await?;

            self.record_claims(&record).await?;
        }

        let clusters = self.recluster().await?;
        self.set_source(
            SourceKind::Github.id(),
            SourceKind::Github.base_url(),
            SourceState::Ok,
            None,
        )
        .await?;

        Ok(IngestReport {
            source: snapshot.source.clone(),
            repo: Some(snapshot.repo.clone()),
            branch: Some(snapshot.branch.clone()),
            found: snapshot.found,
            added,
            updated,
            unchanged,
            with_body: snapshot.skills.len(),
            http_requests: snapshot.http_requests,
            truncated: snapshot.truncated,
            clusters,
            scan: None,
            blocked: 0,
        })
    }

    /// Scan the exact bodies a repository walk produced, drop the ones SkillSpector blocks,
    /// and return the worst verdict among those that remain. Writing the bodies to one temporary
    /// collection makes this a single scanner invocation even for a 500-skill repository.
    async fn gate_snapshot(
        &self,
        snapshot: &mut RepoSnapshot,
        single_preflight: Option<&ScanVerdict>,
    ) -> Result<(ScanVerdict, usize), SkillsError> {
        if snapshot.skills.is_empty() {
            return Err(SkillsError::NoSkillsFound(format!(
                "no skill body was read from {}",
                snapshot.repo
            )));
        }

        if snapshot.skills.len() == 1
            && let Some(preflight) = single_preflight
        {
            return Ok((preflight.clone(), 0));
        }

        let root = body_scan_dir();
        fs::create_dir_all(&root)?;
        let mut labels = Vec::with_capacity(snapshot.skills.len());
        for (index, skill) in snapshot.skills.iter().enumerate() {
            let label = format!("skill-{index:04}");
            let folder = root.join(&label);
            if let Err(error) = fs::create_dir_all(&folder)
                .and_then(|()| fs::write(folder.join("SKILL.md"), &skill.body))
            {
                let _ = fs::remove_dir_all(&root);
                return Err(error.into());
            }
            labels.push(label);
        }

        let target = root.to_string_lossy().into_owned();
        let scan = self
            .scanner
            .scan_collection(&target, snapshot.skills.len())
            .await;
        let _ = fs::remove_dir_all(&root);
        let scan = scan?;

        apply_collection(snapshot, &scan)
    }

    /// The shared, published catalogue. Metadata only: rows carry no body, so a seeded skill is
    /// browsable and searchable while staying explicitly unreviewed.
    pub const CATALOG_URL: &'static str =
        "https://rigel-25128-default-rtdb.firebaseio.com/catalog/v1/skills.json";

    /// Seed the shelf from the published catalogue. `write_skill` COALESCEs `body_sha` and
    /// `grade`, so seeding can never downgrade a row this machine already fetched and scanned.
    pub async fn seed_catalog(&self, url: &str, limit: usize) -> Result<SeedReport, SkillsError> {
        // Borrowed only for its cached HTTP client: the catalogue is one 5.7 MB GET, and the
        // blob cache means a second seed inside the TTL costs nothing.
        let source = self.github().await?;
        let body = source.http().get_json(url, true).await?;
        let nodes = body
            .as_object()
            .ok_or_else(|| SkillsError::Other("the catalogue reply was not an object".into()))?;

        let mut written = 0usize;
        let mut skipped = 0usize;
        for node in nodes.values() {
            if limit > 0 && written >= limit {
                break;
            }
            let field = |key: &str| node.get(key).cloned().unwrap_or(serde_json::Value::Null);
            let id = field("i").as_str().unwrap_or_default().to_string();
            if id.is_empty() {
                skipped += 1;
                continue;
            }
            let record = SkillRecord {
                id,
                name: field("n").as_str().unwrap_or_default().to_string(),
                description: field("d").as_str().unwrap_or_default().to_string(),
                source: field("s").as_str().unwrap_or("catalog").to_string(),
                repo: field("r")
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(ToOwned::to_owned),
                path: field("p")
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(ToOwned::to_owned),
                url: field("u")
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(ToOwned::to_owned),
                body_sha: None,
                grade: None,
                grade_scope: None,
                license: None,
                stars: field("st").as_i64(),
                installs: field("in").as_i64(),
                tokens: field("tk").as_i64(),
                capabilities: Vec::new(),
                simhash: None,
            };
            self.write_skill(&record).await?;
            written += 1;
        }

        Ok(SeedReport {
            rows: nodes.len(),
            written,
            skipped,
        })
    }

    /// Write one skill. Used for both scanned bodies and bare marketplace rows.
    pub async fn write_skill(&self, record: &SkillRecord) -> Result<(), SkillsError> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO skills
               (id, name, description, source, repo, path, url, body_sha, grade, grade_scope,
                license, stars, installs, tokens, cluster_id, first_seen_at, last_seen_at,
                capabilities_json, simhash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               description = excluded.description,
               repo = COALESCE(excluded.repo, skills.repo),
               path = COALESCE(excluded.path, skills.path),
               url = COALESCE(excluded.url, skills.url),
               body_sha = COALESCE(excluded.body_sha, skills.body_sha),
               grade = COALESCE(excluded.grade, skills.grade),
               grade_scope = COALESCE(excluded.grade_scope, skills.grade_scope),
               license = COALESCE(excluded.license, skills.license),
               stars = COALESCE(excluded.stars, skills.stars),
               installs = COALESCE(excluded.installs, skills.installs),
               tokens = COALESCE(excluded.tokens, skills.tokens),
               last_seen_at = excluded.last_seen_at,
               capabilities_json = COALESCE(excluded.capabilities_json, skills.capabilities_json),
               simhash = COALESCE(excluded.simhash, skills.simhash)",
        )
        .bind(&record.id)
        .bind(&record.name)
        .bind(&record.description)
        .bind(&record.source)
        .bind(&record.repo)
        .bind(&record.path)
        .bind(&record.url)
        .bind(&record.body_sha)
        .bind(&record.grade)
        .bind(&record.grade_scope)
        .bind(&record.license)
        .bind(record.stars)
        .bind(record.installs)
        .bind(record.tokens)
        .bind(now)
        .bind(now)
        .bind(record.capabilities_json())
        .bind(record.simhash.map(|value| format!("{:016x}", value as u64)))
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    async fn record_claims(&self, record: &SkillRecord) -> Result<(), SkillsError> {
        for evidence in
            claims_from_description(&record.id, &record.description, record.url.as_deref())
        {
            evidence.record(self.store.pool()).await?;
        }
        Ok(())
    }

    /// Upsert metadata rows that have no body yet — the marketplaces.
    pub async fn ingest_records(&self, records: &[SkillRecord]) -> Result<usize, SkillsError> {
        let written = self.ingest_records_quiet(records).await?;
        self.recluster().await?;
        Ok(written)
    }

    /// Same upsert, without the per-call recluster. The sweep calls this once per page and
    /// reclusters once at the end, so a 400-page walk does one clustering pass instead of 400.
    pub async fn ingest_records_quiet(
        &self,
        records: &[SkillRecord],
    ) -> Result<usize, SkillsError> {
        for record in records {
            self.write_skill(record).await?;
            self.record_claims(record).await?;
        }
        Ok(records.len())
    }

    async fn skill_count(&self) -> Result<i64, SkillsError> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM skills")
            .fetch_one(self.store.pool())
            .await?)
    }

    /// Drop duplicate metadata rows. Two rows are the same skill only when the trimmed,
    /// case-insensitive name matches **and** the first 160 characters of the description
    /// match, with a description specific enough (≥ 40 chars) to mean anything — a row
    /// with a scanned body is never deleted. Clusters of near-variants that differ in
    /// wording are left to the simhash mechanism, which needs bodies.
    pub async fn deduplicate(&self) -> Result<i64, SkillsError> {
        let result = sqlx::query(
            "DELETE FROM skills WHERE body_sha IS NULL AND id IN (
               SELECT id FROM (
                 SELECT id, ROW_NUMBER() OVER (
                   PARTITION BY lower(trim(name)), substr(trim(description), 1, 160)
                   ORDER BY first_seen_at, id
                 ) AS ranked
                 FROM skills
                 WHERE length(trim(description)) >= 40
                   AND trim(description) <> 'No description in frontmatter.'
               ) WHERE ranked > 1
             )",
        )
        .execute(self.store.pool())
        .await?;
        Ok(result.rows_affected() as i64)
    }

    /// Remove one row from the local shelf.
    ///
    /// The FTS triggers keep search in step, and the row's version history and evidence go
    /// with it so `rigel skills get` cannot find orphans. A body blob is left to `rigel skills
    /// gc`: another row may still reference the same bytes. Returns whether a row existed.
    ///
    /// This is the local half of cloud deletion: the next `rigel cloud push` sees the id in
    /// its manifest but not in the shelf, and writes a tombstone for the other machines.
    pub async fn remove(&self, id: &str) -> Result<bool, SkillsError> {
        let mut tx = self.store.pool().begin().await?;
        sqlx::query("DELETE FROM evidence WHERE skill_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM skill_versions WHERE skill_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query("DELETE FROM skills WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(deleted.rows_affected() > 0)
    }

    pub async fn sync_marketplace(
        &self,
        which: SourceKind,
        query: &str,
        limit: usize,
    ) -> Result<usize, SkillsError> {
        let (base, result) = match which {
            SourceKind::SkillsMp => {
                let source = self.skillsmp().await?;
                (which.base_url(), source.search_records(query, limit).await)
            }
            SourceKind::SkillHub => {
                let source = self.skillhub().await?;
                (which.base_url(), source.browse(1, limit as u32).await)
            }
            // Repository walking is not marketplace sync; refusing is the typed equivalent of
            // the old catch-all, without accepting an arbitrary source string.
            SourceKind::Github => {
                return Err(SkillsError::Unsupported("sync_marketplace", which.id()));
            }
        };

        match result {
            Ok(records) => {
                let count = self.ingest_records(&records).await?;
                self.set_source(which.id(), base, SourceState::Ok, None)
                    .await?;
                Ok(count)
            }
            Err(error) => {
                self.set_source(
                    which.id(),
                    base,
                    SourceState::Broken,
                    Some(&error.to_string()),
                )
                .await?;
                Err(error)
            }
        }
    }

    /// Re-sync every source that is not fresh, so a shelf nobody has touched for a day updates
    /// itself instead of staying stale.
    ///
    /// Bounded on purpose, because this runs at launch and in the background: the repositories
    /// whose skills are already in this index (at most [`AUTO_REFRESH_REPOS`]), and one page of
    /// each marketplace. The freshness policy still applies, so an unchanged tree costs a tree
    /// call and no bodies.
    ///
    /// A **broken** source is retried on a bounded backoff measured from its last attempt, not
    /// from its last success: five minutes, then fifteen, thirty, one hour, three hours, and
    /// finally every six hours. That recovers a short outage quickly without turning a dead URL
    /// into a request loop. A success resets the count.
    pub async fn refresh_stale(&self) -> Result<RefreshReport, SkillsError> {
        self.refresh_sources(false).await
    }

    /// Re-sync every source now, ignoring the freshness window. The launch pass uses it, so a
    /// GitHub shelf that was fresh when the window last closed is still compared with its remote
    /// on this launch instead of waiting out the stale window.
    pub async fn refresh_all(&self) -> Result<RefreshReport, SkillsError> {
        self.refresh_sources(true).await
    }

    async fn refresh_sources(&self, force: bool) -> Result<RefreshReport, SkillsError> {
        let statuses = self.status().await?;
        let mut report = RefreshReport {
            checked: statuses.len(),
            refreshed: 0,
            sources: Vec::with_capacity(statuses.len()),
        };

        for status in statuses {
            let due = force
                || match status.state {
                    SourceState::Ok => false,
                    SourceState::Stale => true,
                    SourceState::Broken => match status.last_attempt_at {
                        None => true,
                        Some(at) => now_ms() - at >= broken_retry_after(status.failures),
                    },
                };
            if !due {
                report.sources.push(SourceRefresh {
                    id: status.id,
                    state: status.state,
                    detail: match status.state {
                        SourceState::Ok => "fresh".to_string(),
                        SourceState::Broken => {
                            "failed recently; retrying after a short backoff".to_string()
                        }
                        SourceState::Stale => "left for the next pass".to_string(),
                    },
                });
                continue;
            }

            let Some(kind) = SourceKind::from_id(&status.id) else {
                // Another surface wrote this row. It has no refresh rule here, and the typed
                // dispatch below has no catch-all that could accidentally claim it.
                let detail = format!("no refresh rule for `{}`", status.id);
                report.sources.push(SourceRefresh {
                    id: status.id,
                    state: status.state,
                    detail,
                });
                continue;
            };

            let (state, detail) = match self.refresh_source(kind).await {
                Ok(detail) => {
                    report.refreshed += 1;
                    (SourceState::Ok, detail)
                }
                Err(error) => (SourceState::Broken, error.to_string()),
            };
            report.sources.push(SourceRefresh {
                id: status.id,
                state,
                detail,
            });
        }

        Ok(report)
    }

    /// What one typed source's refresh does. Unknown persisted ids never reach this function.
    async fn refresh_source(&self, kind: SourceKind) -> Result<String, SkillsError> {
        match kind {
            SourceKind::Github => {
                let repos = self.indexed_repos(AUTO_REFRESH_REPOS).await?;
                if repos.is_empty() {
                    return Ok("no repository indexed yet".to_string());
                }
                let mut rows = 0;
                let mut failure: Option<SkillsError> = None;
                for repo in &repos {
                    match self.index_repo(repo, SyncPolicy::normal()).await {
                        Ok(report) => rows += report.added + report.updated,
                        Err(error) => {
                            failure.get_or_insert(error);
                        }
                    }
                }
                // One failing repository must not hide a walk that worked; every repository
                // failing is a broken source, and `index_repo` has already recorded it as one.
                match failure {
                    Some(error) if rows == 0 => Err(error),
                    Some(error) => Ok(format!(
                        "{} repo(s), {rows} row(s) updated · one still failing: {error}",
                        repos.len()
                    )),
                    None => Ok(format!("{} repo(s), {rows} row(s) updated", repos.len())),
                }
            }
            SourceKind::SkillHub => {
                let rows = self
                    .sync_marketplace(kind, AUTO_REFRESH_QUERY, AUTO_REFRESH_LIMIT)
                    .await?;
                Ok(format!("{rows} row(s) from skillhub"))
            }
            // SkillsMP is a *search* source and its rows carry no record of the query that found
            // them, so the shelf's own default is re-run: it adds what that query finds and makes
            // the source fresh again. Rows from the other queries stay put; `rigel skills sweep`
            // is what walks those.
            SourceKind::SkillsMp => {
                let rows = self
                    .sync_marketplace(kind, AUTO_REFRESH_QUERY, AUTO_REFRESH_LIMIT)
                    .await?;
                Ok(format!(
                    "{rows} row(s) from the `{AUTO_REFRESH_QUERY}` query"
                ))
            }
        }
    }

    /// The repositories this index's skills came from, most recently seen first.
    pub async fn indexed_repos(&self, limit: usize) -> Result<Vec<String>, SkillsError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT repo FROM skills
             WHERE source = 'github' AND repo IS NOT NULL AND repo <> ''
             GROUP BY repo ORDER BY MAX(last_seen_at) DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(self.store.pool())
        .await?;
        Ok(rows.into_iter().map(|(repo,)| repo).collect())
    }

    /// Sweep the two marketplaces across many pages and turn the result into metadata rows —
    /// what a skill costs nothing to know: name, description, stars, where to fetch it.
    /// No body is downloaded. Both walks are independent: a failure in one leaves the other's
    /// rows in the index and reports itself in the table.
    pub async fn sweep(
        &self,
        skillhub_pages: u32,
        skillsmp_queries: &[String],
        skillsmp_pages: u32,
    ) -> Result<SweepReport, SkillsError> {
        let skills_before = self.skill_count().await?;

        let mut skillhub_rows = 0;

        // --- SkillHub: known pages of 100 cards, stop at the first empty page ---
        let mut skillhub_error: Option<String> = None;
        if skillhub_pages > 0 {
            let source = self.skillhub().await?;
            for page in 1..=skillhub_pages {
                match source.browse(page, 100).await {
                    Ok(records) => {
                        self.ingest_records_quiet(&records).await?;
                        skillhub_rows += records.len();
                        if records.is_empty() {
                            break;
                        }
                    }
                    Err(error) => {
                        skillhub_error = Some(error.to_string());
                        break;
                    }
                }
            }
            self.set_source(
                SourceKind::SkillHub.id(),
                SourceKind::SkillHub.base_url(),
                match skillhub_error {
                    Some(_) => SourceState::Broken,
                    None => SourceState::Ok,
                },
                skillhub_error.as_deref(),
            )
            .await?;
        }

        // --- SkillsMP: pages of 50 per broad query, stop at the first empty page ---
        let mut skillsmp_rows = 0;
        let mut skillsmp_pages_seen = 0;
        let mut skillsmp_error: Option<String> = None;
        let mut queries_seen = 0;
        if skillsmp_pages > 0 {
            let source = self.skillsmp().await?;
            for query in skillsmp_queries {
                let query = query.trim();
                if query.is_empty() {
                    continue;
                }
                queries_seen += 1;
                for page in 1..=skillsmp_pages {
                    match source.search_records_page(query, 50, page).await {
                        Ok(records) => {
                            self.ingest_records_quiet(&records).await?;
                            skillsmp_rows += records.len();
                            skillsmp_pages_seen += 1;
                            if records.is_empty() {
                                break;
                            }
                        }
                        Err(error) => {
                            skillsmp_error.get_or_insert_with(|| error.to_string());
                            break;
                        }
                    }
                }
            }
            self.set_source(
                SourceKind::SkillsMp.id(),
                SourceKind::SkillsMp.base_url(),
                match skillsmp_error {
                    Some(_) => SourceState::Broken,
                    None => SourceState::Ok,
                },
                skillsmp_error.as_deref(),
            )
            .await?;
        }

        // One clustering pass and one duplicate pass for the whole walk.
        self.recluster().await?;
        let duplicates_dropped = self.deduplicate().await?;

        Ok(SweepReport {
            skillhub_pages: skillhub_pages as usize,
            skillhub_rows,
            skillhub_error,
            skillsmp_queries: queries_seen,
            skillsmp_pages: skillsmp_pages_seen,
            skillsmp_rows,
            skillsmp_error,
            duplicates_dropped,
            skills_before,
            skills_after: self.skill_count().await?,
        })
    }

    pub async fn set_source(
        &self,
        id: &str,
        base_url: &str,
        state: SourceState,
        error: Option<&str>,
    ) -> Result<(), SkillsError> {
        let now = now_ms();
        let synced = matches!(state, SourceState::Ok).then_some(now);
        let failed = if matches!(state, SourceState::Ok) {
            0
        } else {
            1
        };

        sqlx::query(
            "INSERT INTO sources (id, base_url, last_sync_at, status, error, last_attempt_at, failures)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               base_url = excluded.base_url,
               status = excluded.status,
               error = excluded.error,
               last_sync_at = COALESCE(excluded.last_sync_at, sources.last_sync_at),
               last_attempt_at = excluded.last_attempt_at,
               failures = CASE WHEN excluded.status = 'ok' THEN 0 ELSE sources.failures + 1 END",
        )
        .bind(id)
        .bind(base_url)
        .bind(synced)
        .bind(state.as_str())
        .bind(error)
        .bind(now)
        .bind(failed)
        .execute(self.store.pool())
        .await?;
        Ok(())
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SkillsError> {
        let hits = self
            .search_ordered(query, limit, SearchOrder::DEFAULT)
            .await?;
        Ok(hits)
    }

    pub async fn search_ordered(
        &self,
        query: &str,
        limit: usize,
        order: SearchOrder,
    ) -> Result<Vec<SearchHit>, SkillsError> {
        self.search_page(query, limit, 0, order).await
    }

    /// One page of the same search. The window still caps a page at 500 rows; callers that
    /// want the whole catalogue page through it with `offset` (the Shelf does).
    pub async fn search_page(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        order: SearchOrder,
    ) -> Result<Vec<SearchHit>, SkillsError> {
        let limit = limit.clamp(1, 500) as i64;
        let offset = offset as i64;
        let order_by = order.order_by();
        let match_expr = fts_query(query);

        let sql = match &match_expr {
            Some(_) => format!(
                // FTS5 rejects an alias on the left of MATCH (`WHERE f MATCH ?` is
                // "no such column: f"), so the virtual table is named in full here.
                "SELECT {COLUMNS}, {EXTRA}
                   FROM skills_fts
                   JOIN skills s ON s.rowid = skills_fts.rowid
                   LEFT JOIN (SELECT cluster_id, COUNT(*) AS n FROM skills
                               WHERE cluster_id IS NOT NULL GROUP BY cluster_id) c
                          ON c.cluster_id = s.cluster_id
                  WHERE skills_fts MATCH ?
                  ORDER BY {order_by}
                  LIMIT ? OFFSET ?"
            ),
            None => format!(
                "SELECT {COLUMNS}, {EXTRA}
                   FROM skills s
                   LEFT JOIN (SELECT cluster_id, COUNT(*) AS n FROM skills
                               WHERE cluster_id IS NOT NULL GROUP BY cluster_id) c
                          ON c.cluster_id = s.cluster_id
                  ORDER BY {order_by}
                  LIMIT ? OFFSET ?"
            ),
        };

        let mut statement = sqlx::query(&sql);
        if let Some(expr) = &match_expr {
            statement = statement.bind(expr);
        }
        let rows = statement
            .bind(limit)
            .bind(offset)
            .fetch_all(self.store.pool())
            .await?;

        Ok(rows.iter().map(row_to_hit).collect())
    }

    /// How many rows a search would return. `query` empty counts the whole shelf.
    pub async fn count(&self, query: &str) -> Result<i64, SkillsError> {
        let match_expr = fts_query(query);
        let rows: i64 = match &match_expr {
            Some(expr) => {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM skills_fts
                       JOIN skills s ON s.rowid = skills_fts.rowid
                      WHERE skills_fts MATCH ?",
                )
                .bind(expr)
                .fetch_one(self.store.pool())
                .await?
            }
            None => {
                sqlx::query_scalar("SELECT COUNT(*) FROM skills")
                    .fetch_one(self.store.pool())
                    .await?
            }
        };
        Ok(rows)
    }

    pub async fn get(&self, id: &str) -> Result<Option<SkillDetail>, SkillsError> {
        let sql = format!(
            "SELECT {COLUMNS}, {EXTRA}, s.first_seen_at, s.capabilities_json
               FROM skills s
               LEFT JOIN (SELECT cluster_id, COUNT(*) AS n FROM skills
                           WHERE cluster_id IS NOT NULL GROUP BY cluster_id) c
                      ON c.cluster_id = s.cluster_id
              WHERE s.id = ?"
        );

        let Some(row) = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(self.store.pool())
            .await?
        else {
            return Ok(None);
        };

        let skill = row_to_hit(&row);
        let capabilities: Vec<Capability> = row
            .try_get::<Option<String>, _>("capabilities_json")
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let first_seen_at: Option<i64> = row.try_get("first_seen_at").ok();

        Ok(Some(SkillDetail {
            evidence: Evidence::for_skill(self.store.pool(), id).await?,
            variants: self.variants(id).await?,
            skill,
            capabilities,
            first_seen_at,
        }))
    }

    pub async fn variants(&self, id: &str) -> Result<Vec<VariantRow>, SkillsError> {
        // cluster_id, body_sha, simhash, license
        type OwnRow = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let row: Option<OwnRow> = sqlx::query_as(
            "SELECT cluster_id, body_sha, simhash, license FROM skills WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.store.pool())
        .await?;

        let Some((Some(cluster_id), own_sha, own_simhash, own_license)) = row else {
            return Ok(Vec::new());
        };

        // id, name, repo, grade, license, last_seen_at, simhash, body_sha
        type SiblingRow = (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let siblings: Vec<SiblingRow> = sqlx::query_as(
            "SELECT id, name, repo, grade, license, last_seen_at, simhash, body_sha
               FROM skills WHERE cluster_id = ? AND id <> ?
               ORDER BY name COLLATE NOCASE",
        )
        .bind(&cluster_id)
        .bind(id)
        .fetch_all(self.store.pool())
        .await?;

        Ok(siblings
            .into_iter()
            .map(
                |(sid, name, repo, grade, sibling_license, last_seen_at, simhash, sha)| {
                    let delta = describe_delta(
                        own_sha.as_deref(),
                        sha.as_deref(),
                        own_simhash.as_deref(),
                        simhash.as_deref(),
                        own_license.as_deref(),
                        sibling_license.as_deref(),
                    );

                    VariantRow {
                        id: sid,
                        name,
                        repo,
                        grade,
                        license: sibling_license,
                        last_seen_at,
                        delta,
                    }
                },
            )
            .collect())
    }

    /// Recompute cluster ids. Cheap enough to run after every ingest and honest enough to be
    /// the only way cluster ids change: they are derived, never assigned by a source.
    pub async fn recluster(&self) -> Result<usize, SkillsError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, simhash FROM skills WHERE simhash IS NOT NULL")
                .fetch_all(self.store.pool())
                .await?;

        let pairs: Vec<(String, u64)> = rows
            .iter()
            .filter_map(|(id, hex)| {
                u64::from_str_radix(hex, 16)
                    .ok()
                    .map(|value| (id.clone(), value))
            })
            .collect();

        let assigned = cluster::assign(&pairs);

        let mut tx = self.store.pool().begin().await?;
        for (id, cluster_id) in &assigned {
            sqlx::query(
                "UPDATE skills SET cluster_id = ?
                  WHERE id = ? AND (cluster_id IS NULL OR cluster_id <> ?)",
            )
            .bind(cluster_id)
            .bind(id)
            .bind(cluster_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE skills SET cluster_id = NULL WHERE simhash IS NULL AND cluster_id IS NOT NULL",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        let distinct: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT cluster_id) FROM skills WHERE cluster_id IS NOT NULL",
        )
        .fetch_one(self.store.pool())
        .await?;
        Ok(distinct as usize)
    }

    pub async fn body(&self, id: &str) -> Result<Option<(String, String)>, SkillsError> {
        let sha: Option<Option<String>> =
            sqlx::query_scalar::<_, Option<String>>("SELECT body_sha FROM skills WHERE id = ?")
                .bind(id)
                .fetch_optional(self.store.pool())
                .await?;

        let Some(Some(sha)) = sha else {
            return Ok(None);
        };
        let Some(text) = self.blobs.get_text(&sha)? else {
            return Ok(None);
        };
        Ok(Some((text, sha)))
    }

    /// The current hash even when its blob is missing. The pin review uses this to report a
    /// changed index without treating a collected body as "not indexed".
    pub async fn body_sha(&self, id: &str) -> Result<Option<String>, SkillsError> {
        let row: Option<Option<String>> =
            sqlx::query_scalar::<_, Option<String>>("SELECT body_sha FROM skills WHERE id = ?")
                .bind(id)
                .fetch_optional(self.store.pool())
                .await?;
        Ok(row.flatten())
    }

    /// Read any body ever written to the content-addressed store. A pinned hash is a version
    /// address, so it is enough to recover the body the session read.
    pub fn body_at(&self, sha: &str) -> Result<Option<String>, SkillsError> {
        // A template may carry a hand-edited hash. Do not let `../` turn the version address
        // into a file read outside the blob directory.
        if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        Ok(self.blobs.get_text(sha)?)
    }

    /// Copy one indexed skill into each harness store. The body is the exact indexed
    /// `SKILL.md`, never a regenerated summary, so an install cannot change what the review saw.
    pub async fn install(&self, id: &str, roots: &[PathBuf]) -> Result<InstallReport, SkillsError> {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM skills WHERE id = ?")
            .bind(id)
            .fetch_optional(self.store.pool())
            .await?;
        let Some(name) = name else {
            return Err(SkillsError::NotIndexed(id.to_string()));
        };
        let (body, _) = self
            .body(id)
            .await?
            .ok_or_else(|| SkillsError::NoBody(id.to_string()))?;
        let scan = self.scan_body(&body).await?;
        if scan.blocked() {
            return Err(SkillsError::ScannerBlocked {
                target: name.clone(),
                score: scan.score,
                recommendation: scan.recommendation,
            });
        }
        let directory = skill_dir_name(&name);
        let mut paths = Vec::with_capacity(roots.len());
        let mut replaced = false;

        for root in roots {
            let folder = root.join(&directory);
            fs::create_dir_all(&folder)?;
            let path = folder.join("SKILL.md");
            let existing = fs::read_to_string(&path).ok();
            if existing.as_deref() == Some(body.as_str()) {
                paths.push(path);
                continue;
            }
            if existing.is_some() {
                replaced = true;
                fs::write(&path, &body)?;
            } else {
                let temporary = folder.join(format!(".SKILL.md.{}.tmp", std::process::id()));
                fs::write(&temporary, &body)?;
                fs::rename(&temporary, &path)?;
            }
            paths.push(path);
        }

        Ok(InstallReport {
            name,
            directory,
            paths,
            replaced,
            scan,
        })
    }

    /// Scan the exact `SKILL.md` bytes an install would write. SkillSpector reads a directory,
    /// so the body gets a disposable one; it is removed before an install can write anything.
    async fn scan_body(&self, body: &str) -> Result<ScanVerdict, SkillsError> {
        let directory = body_scan_dir();
        fs::create_dir_all(&directory)?;
        let path = directory.join("SKILL.md");
        let target = path.to_string_lossy().into_owned();
        let outcome = match fs::write(&path, body) {
            Ok(()) => self.scanner.scan(&target).await,
            Err(error) => Err(error.into()),
        };
        let _ = fs::remove_dir_all(&directory);
        outcome
    }

    /// Whether this row already has a `SKILL.md` in every supplied store.
    pub async fn installed(&self, id: &str, roots: &[PathBuf]) -> Result<bool, SkillsError> {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM skills WHERE id = ?")
            .bind(id)
            .fetch_optional(self.store.pool())
            .await?;
        let Some(name) = name else {
            return Ok(false);
        };
        let directory = skill_dir_name(&name);
        Ok(roots
            .iter()
            .all(|root| root.join(&directory).join("SKILL.md").is_file()))
    }

    /// Seed the compiled-in bodies and install a catalogue entry's skill ids. Both the CLI and
    /// the window used to spell this order and the `builtin:` id convention themselves.
    pub async fn install_builtin_skills(
        &self,
        ids: &[String],
        roots: &[PathBuf],
    ) -> Result<Vec<InstallReport>, SkillsError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.install_builtin().await?;
        let mut reports = Vec::with_capacity(ids.len());
        for id in ids {
            reports.push(self.install(&format!("builtin:{id}"), roots).await?);
        }
        Ok(reports)
    }

    pub async fn status(&self) -> Result<Vec<SourceStatus>, SkillsError> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, base_url FROM sources ORDER BY id")
                .fetch_all(self.store.pool())
                .await?;

        let mut statuses = Vec::with_capacity(rows.len());
        for (id, base_url) in rows {
            statuses.push(load_status(self.store.pool(), &id, base_url.as_deref()).await?);
        }
        Ok(statuses)
    }

    pub async fn stats(&self) -> Result<IndexStats, SkillsError> {
        let skills: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
            .fetch_one(self.store.pool())
            .await?;
        let with_body: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE body_sha IS NOT NULL")
                .fetch_one(self.store.pool())
                .await?;
        let clusters: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT cluster_id) FROM skills WHERE cluster_id IS NOT NULL",
        )
        .fetch_one(self.store.pool())
        .await?;

        let mut files = 0_usize;
        let mut bytes = 0_u64;
        if let Ok(entries) = std::fs::read_dir(self.blobs.root()) {
            for entry in entries.flatten() {
                match entry.metadata() {
                    Ok(metadata) if metadata.is_file() => {
                        files += 1;
                        bytes += metadata.len();
                    }
                    _ => {}
                }
            }
        }

        Ok(IndexStats {
            skills,
            with_body,
            clusters,
            blobs: files,
            blob_bytes: bytes,
            sources: self.status().await?,
        })
    }

    /// Every metadata row, newest first, without bodies.
    ///
    /// This is the cloud mirror's read side: it needs the whole shelf rather than one page of
    /// search hits, and it must be able to send a row without its body. `body_sha` is deliberately
    /// absent — a pulled record never names a blob this machine holds, so `write_skill` keeps the
    /// local one instead of clearing it.
    pub async fn catalogue(&self, limit: usize) -> Result<Vec<SkillRecord>, SkillsError> {
        let rows = sqlx::query(
            "SELECT id, name, description, source, repo, path, url, grade, grade_scope,
                    license, stars, installs, tokens, capabilities_json, simhash
               FROM skills
              ORDER BY last_seen_at DESC, id
              LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(self.store.pool())
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let capabilities = row
                    .get::<Option<String>, _>("capabilities_json")
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default();
                let simhash = row
                    .get::<Option<String>, _>("simhash")
                    .and_then(|text| u64::from_str_radix(&text, 16).ok())
                    .map(|value| value as i64);
                SkillRecord {
                    id: row.get("id"),
                    name: row.get("name"),
                    description: row
                        .get::<Option<String>, _>("description")
                        .unwrap_or_default(),
                    source: row.get("source"),
                    repo: row.get("repo"),
                    path: row.get("path"),
                    url: row.get("url"),
                    body_sha: None,
                    grade: row.get("grade"),
                    grade_scope: row.get("grade_scope"),
                    license: row.get("license"),
                    stars: row.get("stars"),
                    installs: row.get("installs"),
                    tokens: row.get("tokens"),
                    capabilities,
                    simhash,
                }
            })
            .collect())
    }

    /// Live blob hashes, for `rigel gc`.
    ///
    /// Cached HTTP bodies count as live: the tree and repo metadata blobs are what makes a
    /// re-index inside the freshness window free, and collecting them would turn every
    /// re-index into a full download.
    pub async fn live_blobs(&self) -> Result<HashSet<String>, SkillsError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT body_sha FROM skills WHERE body_sha IS NOT NULL
             UNION
             SELECT sha256 FROM http_cache WHERE sha256 IS NOT NULL",
        )
        .fetch_all(self.store.pool())
        .await?;
        Ok(rows.into_iter().map(|(sha,)| sha).collect())
    }

    /// P2.9: take the prototype's `skilldb:entries:v1` rows and put them in the index.
    ///
    /// They arrive with no body — the prototype only ever kept metadata — so they are written
    /// as `local:` rows with their original grade and savedAt, and the shelf shows them as
    /// unscanned-but-known. `skipped` counts rows already present (same URL), `invalid` rows
    /// without a name. Clearing the browser keys is the export snippet's job, not ours: the
    /// app cannot reach another origin's localStorage.
    pub async fn import_prototype_entries(&self, json: &str) -> Result<ImportReport, SkillsError> {
        let entries: Vec<PrototypeEntry> = serde_json::from_str(json)
            .map_err(|error| SkillsError::Other(format!("not a skilldb export: {error}")))?;

        let mut imported = 0;
        let mut skipped = 0;
        let mut invalid = 0;

        for entry in entries {
            let name = entry.name.trim().to_string();
            if name.is_empty() {
                invalid += 1;
                continue;
            }

            if let Some(url) = &entry.url {
                let exists: Option<String> =
                    sqlx::query_scalar("SELECT id FROM skills WHERE url = ? LIMIT 1")
                        .bind(url)
                        .fetch_optional(self.store.pool())
                        .await?;
                if exists.is_some() {
                    skipped += 1;
                    continue;
                }
            }

            let saved_at = entry.saved_at.unwrap_or_else(now_ms);
            let record = SkillRecord {
                id: format!("local:{}", entry.key),
                name,
                description: entry.description.chars().take(240).collect(),
                source: "local".to_string(),
                repo: entry.source.clone(),
                path: entry.path,
                url: entry.url,
                body_sha: None,
                grade: entry.grade,
                grade_scope: Some("prototype".to_string()),
                license: None,
                stars: entry.stars,
                installs: entry.installs.and_then(|value| value.as_i64()),
                tokens: None,
                capabilities: Vec::new(),
                simhash: None,
            };

            self.write_skill(&record).await?;
            if entry.saved_at.is_some() {
                // Keep the prototype's timestamp: "last seen" should not reset just because
                // the row was migrated.
                sqlx::query("UPDATE skills SET first_seen_at = ?, last_seen_at = ? WHERE id = ?")
                    .bind(saved_at)
                    .bind(saved_at)
                    .bind(&record.id)
                    .execute(self.store.pool())
                    .await?;
            }
            imported += 1;
        }

        Ok(ImportReport {
            imported,
            skipped,
            invalid,
        })
    }
}

/// The directory name a harness sees. Names are user-facing, so this keeps the readable
/// lowercase form and collapses everything that is not a letter, digit, dash or underscore.
pub fn skill_dir_name(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !slug.is_empty() {
            slug.push('-');
            separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "skill".to_string()
    } else {
        slug
    }
}

const COLUMNS: &str = "s.id, s.name, s.description, s.source, s.repo, s.path, s.url,
                       s.grade, s.grade_scope, s.stars, s.installs, s.tokens, s.cluster_id,
                       s.last_seen_at";

/// `unmeasured` is per metric, not per skill: a third-party benchmark can cover one claim
/// while the description's other numbers stay unverified.
const EXTRA: &str = "COALESCE(c.n, 1) AS variants,
                     EXISTS(SELECT 1 FROM evidence e
                             WHERE e.skill_id = s.id AND e.measured IS NULL
                               AND e.advertised IS NOT NULL
                               AND NOT EXISTS (SELECT 1 FROM evidence m
                                                WHERE m.skill_id = e.skill_id
                                                  AND m.metric = e.metric
                                                  AND m.measured IS NOT NULL)) AS unmeasured,
                     (s.body_sha IS NOT NULL) AS has_body";

/// Nullable columns are decoded as `Option<T>`, never as `T` with `.ok()`. Decoding a NULL
/// into `i64` succeeds with 0 in sqlx's SQLite backend, which would report a repository with
/// no star count as having zero stars instead of "unknown".
/// One source's state after a refresh pass, as the shelf and Settings show it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRefresh {
    pub id: String,
    pub state: SourceState,
    /// One line: what was fetched, or why the source was left alone.
    pub detail: String,
}

/// What one auto-refresh pass did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshReport {
    /// Sources this index knows about.
    pub checked: usize,
    /// Sources that were actually re-synced.
    pub refreshed: usize,
    pub sources: Vec<SourceRefresh>,
}

/// How many repositories one pass re-walks. The point is a fresh shelf, not a full re-index: the
/// freshness policy makes each walk a tree call at most, but a dozen repositories would still be
/// a dozen requests nobody asked for.
const AUTO_REFRESH_REPOS: usize = 3;

/// The query the marketplace refresh re-runs, and the page size it pulls. SkillsMP rows carry no
/// record of the query that found them, so the shelf's own default query is used.
const AUTO_REFRESH_QUERY: &str = "agent";
const AUTO_REFRESH_LIMIT: usize = 30;

/// How long a broken source waits before another attempt. The first retry is deliberately quick
/// (a dropped connection is often momentary); the rest back off so a dead endpoint is not polled
/// every time the window checks. The last value is the steady state.
const BROKEN_RETRY_MS: [i64; 6] = [
    5 * 60 * 1000,
    15 * 60 * 1000,
    30 * 60 * 1000,
    60 * 60 * 1000,
    3 * 60 * 60 * 1000,
    6 * 60 * 60 * 1000,
];

fn broken_retry_after(failures: u32) -> i64 {
    let step = (failures.max(1) as usize).saturating_sub(1);
    BROKEN_RETRY_MS[step.min(BROKEN_RETRY_MS.len() - 1)]
}

fn worse(candidate: &ScanVerdict, current: Option<&ScanVerdict>) -> bool {
    match current {
        None => true,
        Some(current) => {
            candidate.score > current.score
                || (candidate.score == current.score
                    && candidate.severity_rank() > current.severity_rank())
        }
    }
}

/// Apply a per-skill collection report to the snapshot: keep allowed skills, drop blocked ones,
/// and fail if the scanner did not answer for any body. Split from `gate_snapshot` so the
/// filtering decision can be tested without a running Python process.
fn apply_collection(
    snapshot: &mut RepoSnapshot,
    scan: &CollectionScan,
) -> Result<(ScanVerdict, usize), SkillsError> {
    let by_path: HashMap<&str, &super::scanner::ScannedSkill> = scan
        .skills
        .iter()
        .map(|skill| (skill.path.as_str(), skill))
        .collect();
    let mut accepted = Vec::with_capacity(snapshot.skills.len());
    let mut blocked = 0usize;
    let mut worst_blocked: Option<ScanVerdict> = None;
    let mut worst_accepted: Option<ScanVerdict> = None;

    for (index, skill) in std::mem::take(&mut snapshot.skills).into_iter().enumerate() {
        let label = format!("skill-{index:04}");
        let Some(report) = by_path.get(label.as_str()) else {
            return Err(SkillsError::Scanner(format!(
                "SkillSpector did not return a verdict for {label}; no skill was written"
            )));
        };
        if report.verdict.blocked() {
            blocked += 1;
            eprintln!(
                "rigel: SkillSpector blocked {} ({}); not indexed",
                skill.record.id,
                report.verdict.summary()
            );
            if worse(&report.verdict, worst_blocked.as_ref()) {
                worst_blocked = Some(report.verdict.clone());
            }
        } else {
            if worse(&report.verdict, worst_accepted.as_ref()) {
                worst_accepted = Some(report.verdict.clone());
            }
            accepted.push(skill);
        }
    }
    snapshot.skills = accepted;

    if let Some(verdict) = worst_accepted {
        Ok((verdict, blocked))
    } else {
        let verdict = worst_blocked.unwrap_or_else(|| scan.aggregate.clone());
        Err(SkillsError::ScannerBlocked {
            target: snapshot.repo.clone(),
            score: verdict.score,
            recommendation: verdict.recommendation,
        })
    }
}

fn row_to_hit(row: &sqlx::sqlite::SqliteRow) -> SearchHit {
    SearchHit {
        id: row.get("id"),
        name: row.get("name"),
        description: row
            .try_get::<Option<String>, _>("description")
            .unwrap_or(None)
            .unwrap_or_default(),
        source: row.get("source"),
        repo: row.try_get::<Option<String>, _>("repo").unwrap_or(None),
        path: row.try_get::<Option<String>, _>("path").unwrap_or(None),
        url: row.try_get::<Option<String>, _>("url").unwrap_or(None),
        grade: row.try_get::<Option<String>, _>("grade").unwrap_or(None),
        grade_scope: row
            .try_get::<Option<String>, _>("grade_scope")
            .unwrap_or(None),
        stars: row.try_get::<Option<i64>, _>("stars").unwrap_or(None),
        installs: row.try_get::<Option<i64>, _>("installs").unwrap_or(None),
        tokens: row.try_get::<Option<i64>, _>("tokens").unwrap_or(None),
        cluster_id: row
            .try_get::<Option<String>, _>("cluster_id")
            .unwrap_or(None),
        variants: row.try_get::<i64, _>("variants").unwrap_or(1),
        last_seen_at: row
            .try_get::<Option<i64>, _>("last_seen_at")
            .unwrap_or(None),
        unmeasured: row.try_get::<i64, _>("unmeasured").unwrap_or(0) != 0,
        has_body: row.try_get::<i64, _>("has_body").unwrap_or(0) != 0,
    }
}

fn describe_delta(
    own_sha: Option<&str>,
    sibling_sha: Option<&str>,
    own_simhash: Option<&str>,
    sibling_simhash: Option<&str>,
    own_license: Option<&str>,
    sibling_license: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if own_sha.is_some() && own_sha == sibling_sha {
        parts.push("identical content".to_string());
    } else {
        match (
            own_simhash.and_then(parse_hex),
            sibling_simhash.and_then(parse_hex),
        ) {
            (Some(a), Some(b)) => {
                parts.push(format!("{} bits from this one", cluster::hamming(a, b)))
            }
            _ => parts.push("not fingerprinted".to_string()),
        }
    }

    match (own_license.is_some(), sibling_license.is_some()) {
        (true, false) => parts.push("no license".to_string()),
        (false, true) => parts.push("no license here".to_string()),
        _ => {}
    }

    parts.join(" · ")
}

fn parse_hex(value: &str) -> Option<u64> {
    u64::from_str_radix(value, 16).ok()
}

#[derive(Debug, Deserialize)]
struct PrototypeEntry {
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "source", default)]
    source: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    grade: Option<String>,
    #[serde(default)]
    stars: Option<i64>,
    #[serde(default)]
    installs: Option<serde_json::Value>,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "savedAt", default)]
    saved_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::super::scanner::ScannedSkill;
    use super::*;
    use crate::skills::FetchedSkill;

    fn verdict(score: i64, recommendation: &str, severity: &str) -> ScanVerdict {
        ScanVerdict {
            score,
            severity: severity.to_string(),
            recommendation: recommendation.to_string(),
            findings: 1,
            scanner: "fake".to_string(),
        }
    }

    fn fetched(id: &str) -> FetchedSkill {
        FetchedSkill {
            record: SkillRecord {
                id: id.to_string(),
                name: id.to_string(),
                description: "test".to_string(),
                source: "github".to_string(),
                repo: Some("acme/skills".to_string()),
                path: Some(format!("{id}/SKILL.md")),
                url: None,
                body_sha: None,
                grade: None,
                grade_scope: None,
                license: None,
                stars: None,
                installs: None,
                tokens: None,
                capabilities: Vec::new(),
                simhash: None,
            },
            body: format!("---\nname: {id}\n---\n"),
        }
    }

    fn snapshot() -> RepoSnapshot {
        RepoSnapshot {
            source: "github".to_string(),
            repo: "acme/skills".to_string(),
            branch: "main".to_string(),
            stars: 1,
            license: None,
            truncated: false,
            found: 3,
            http_requests: 5,
            skills: vec![fetched("safe"), fetched("bad"), fetched("caution")],
        }
    }

    fn collection() -> CollectionScan {
        CollectionScan {
            aggregate: verdict(56, "DO_NOT_INSTALL", "HIGH"),
            skills: vec![
                ScannedSkill {
                    name: "safe".into(),
                    path: "skill-0000".into(),
                    verdict: verdict(0, "SAFE", "LOW"),
                },
                ScannedSkill {
                    name: "bad".into(),
                    path: "skill-0001".into(),
                    verdict: verdict(56, "DO_NOT_INSTALL", "HIGH"),
                },
                ScannedSkill {
                    name: "caution".into(),
                    path: "skill-0002".into(),
                    verdict: verdict(45, "CAUTION", "MEDIUM"),
                },
            ],
        }
    }

    #[test]
    fn collection_gate_drops_only_the_blocked_skill() {
        let mut snapshot = snapshot();
        let (worst, blocked) = apply_collection(&mut snapshot, &collection()).unwrap();

        assert_eq!(blocked, 1);
        assert_eq!(worst.recommendation, "CAUTION");
        assert_eq!(worst.score, 45);
        let ids: Vec<&str> = snapshot
            .skills
            .iter()
            .map(|skill| skill.record.id.as_str())
            .collect();
        assert_eq!(ids, vec!["safe", "caution"]);
    }

    #[test]
    fn an_all_blocked_collection_returns_the_worst_refusal() {
        let mut snapshot = snapshot();
        let mut scan = collection();
        for skill in &mut scan.skills {
            skill.verdict = verdict(80, "DO_NOT_INSTALL", "CRITICAL");
        }
        let error = apply_collection(&mut snapshot, &scan).unwrap_err();
        assert!(matches!(
            error,
            SkillsError::ScannerBlocked { score: 80, .. }
        ));
        assert!(snapshot.skills.is_empty());
    }

    #[test]
    fn a_missing_per_skill_verdict_fails_closed() {
        let mut snapshot = snapshot();
        let mut scan = collection();
        scan.skills.pop();
        let error = apply_collection(&mut snapshot, &scan).unwrap_err();
        assert!(matches!(error, SkillsError::Scanner(_)));
    }
}
