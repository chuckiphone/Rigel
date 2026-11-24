//! The security gate's ordering and no-write guarantee.
//!
//! These tests inject both the scanner and the repository walker. They never open a socket, and
//! every blocked case proves the same persisted state: no skill row, no body blob and no harness
//! install directory.

use std::fs;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rigel_core::skills::{
    CollectionScan, RepoFetcher, RepoSnapshot, ScanVerdict, ScannedSkill, SkillIndex, SkillRecord,
    SkillScanner, SkillSpector, SkillsError, SyncPolicy,
};
use tempfile::TempDir;

fn verdict(score: i64, severity: &str, recommendation: &str) -> ScanVerdict {
    ScanVerdict {
        score,
        severity: severity.into(),
        recommendation: recommendation.into(),
        findings: 1,
        scanner: "fake".into(),
    }
}

fn caution() -> ScanVerdict {
    verdict(45, "MEDIUM", "CAUTION")
}

fn safe() -> ScanVerdict {
    verdict(0, "LOW", "SAFE")
}

fn body(name: &str) -> String {
    format!("---\nname: {name}\ndescription: test fixture\n---\n\nUse the fixture.\n")
}

fn snapshot(name: &str) -> RepoSnapshot {
    RepoSnapshot {
        source: "github".to_string(),
        repo: "acme/skills".to_string(),
        branch: "main".to_string(),
        stars: 1,
        license: None,
        truncated: false,
        found: 1,
        http_requests: 3,
        skills: vec![rigel_core::skills::FetchedSkill {
            record: SkillRecord {
                id: format!("github:acme/skills:{name}/SKILL.md"),
                name: name.to_string(),
                description: "test fixture".to_string(),
                source: "github".to_string(),
                repo: Some("acme/skills".to_string()),
                path: Some(format!("{name}/SKILL.md")),
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
            body: body(name),
        }],
    }
}

#[derive(Clone)]
enum Answer {
    Verdict(ScanVerdict),
    Error(String),
}

#[derive(Clone)]
enum CollectionAnswer {
    Report(CollectionScan),
    Error(String),
}

#[derive(Clone)]
struct FakeScanner {
    preflight: Answer,
    collection: CollectionAnswer,
    targets: Arc<Mutex<Vec<String>>>,
}

impl FakeScanner {
    fn preflight(answer: Answer) -> Self {
        Self {
            preflight: answer,
            collection: CollectionAnswer::Error("collection was not expected".into()),
            targets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn collection(preflight: Answer, collection: CollectionAnswer) -> Self {
        Self {
            preflight,
            collection,
            targets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn scanning(verdict: ScanVerdict) -> Self {
        Self::blocking_collection(verdict.clone(), verdict)
    }

    fn blocking_collection(preflight: ScanVerdict, verdict: ScanVerdict) -> Self {
        let collection = CollectionScan {
            aggregate: verdict.clone(),
            skills: vec![ScannedSkill {
                name: "fixture".into(),
                path: "skill-0000".into(),
                verdict,
            }],
        };
        Self::collection(
            Answer::Verdict(preflight),
            CollectionAnswer::Report(collection),
        )
    }

    fn targets(&self) -> Vec<String> {
        self.targets.lock().unwrap().clone()
    }
}

#[async_trait]
impl SkillScanner for FakeScanner {
    async fn scan(&self, target: &str) -> Result<ScanVerdict, SkillsError> {
        self.targets.lock().unwrap().push(target.to_string());
        match &self.preflight {
            Answer::Verdict(verdict) => Ok(verdict.clone()),
            Answer::Error(error) => Err(SkillsError::Scanner(error.clone())),
        }
    }

    async fn scan_collection(
        &self,
        target: &str,
        _max_skills: usize,
    ) -> Result<CollectionScan, SkillsError> {
        self.targets
            .lock()
            .unwrap()
            .push(format!("collection:{target}"));
        match &self.collection {
            CollectionAnswer::Report(scan) => Ok(scan.clone()),
            CollectionAnswer::Error(error) => Err(SkillsError::Scanner(error.clone())),
        }
    }
}

#[derive(Clone)]
struct FakeFetcher {
    snapshot: RepoSnapshot,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeFetcher {
    fn new(name: &str) -> Self {
        Self {
            snapshot: snapshot(name),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl RepoFetcher for FakeFetcher {
    async fn fetch_repo(
        &self,
        target: &str,
        _policy: SyncPolicy,
        slug: Option<&str>,
        limit: Option<usize>,
    ) -> Result<RepoSnapshot, SkillsError> {
        self.calls.lock().unwrap().push(format!(
            "{target}|slug={}|limit={}",
            slug.unwrap_or("-"),
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into())
        ));
        Ok(self.snapshot.clone())
    }
}

async fn open(tmp: &TempDir, scanner: FakeScanner, fetcher: FakeFetcher) -> SkillIndex {
    SkillIndex::open(tmp.path())
        .await
        .unwrap()
        .with_scanner(Arc::new(scanner))
        .with_fetcher(Arc::new(fetcher))
}

fn blob_count(index: &SkillIndex) -> usize {
    fs::read_dir(index.blobs().root())
        .map(|entries| entries.count())
        .unwrap_or(0)
}

async fn assert_no_index_writes(index: &SkillIndex, context: &str) {
    let skills: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    assert_eq!(skills, 0, "{context}: no skill row may be written");
    assert_eq!(sources, 0, "{context}: no source row may be written");
    assert_eq!(
        blob_count(index),
        0,
        "{context}: no body blob may be written"
    );
}

#[tokio::test]
async fn every_blocked_preflight_refuses_before_fetch_and_writes_nothing() {
    for (case, answer) in [
        (
            "score above the threshold",
            Answer::Verdict(verdict(51, "LOW", "CAUTION")),
        ),
        (
            "HIGH severity",
            Answer::Verdict(verdict(1, "HIGH", "CAUTION")),
        ),
        (
            "CRITICAL severity",
            Answer::Verdict(verdict(1, "CRITICAL", "CAUTION")),
        ),
        (
            "DO_NOT_INSTALL",
            Answer::Verdict(verdict(0, "LOW", "DO_NOT_INSTALL")),
        ),
        (
            "missing binary",
            Answer::Error("SkillSpector was not found on this machine".into()),
        ),
        (
            "error exit",
            Answer::Error("SkillSpector exited with code 2".into()),
        ),
    ] {
        let data = TempDir::new().unwrap();
        let stores = TempDir::new().unwrap();
        let scanner = FakeScanner::preflight(answer);
        let fetcher = FakeFetcher::new("pdf");
        let index = open(&data, scanner.clone(), fetcher.clone()).await;

        let error = index
            .index_repo(
                "https://github.com/acme/skills/tree/main/pdf",
                SyncPolicy::normal(),
            )
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                SkillsError::ScannerBlocked { .. } | SkillsError::Scanner(_)
            ),
            "{case}: {error}"
        );
        assert_eq!(
            scanner.targets(),
            vec!["https://github.com/acme/skills/tree/main/pdf".to_string()],
            "{case}: the preflight must be the only scan"
        );
        assert!(
            fetcher.calls().is_empty(),
            "{case}: a single-skill URL must be refused before the fetch"
        );
        assert_no_index_writes(&index, case).await;
        assert_eq!(
            fs::read_dir(stores.path()).unwrap().count(),
            0,
            "{case}: no harness store may be touched"
        );
    }
}

#[tokio::test]
async fn every_blocked_collection_verdict_writes_nothing() {
    for (case, blocked) in [
        ("score above the threshold", verdict(51, "LOW", "CAUTION")),
        ("HIGH severity", verdict(1, "HIGH", "CAUTION")),
        ("CRITICAL severity", verdict(1, "CRITICAL", "CAUTION")),
        ("DO_NOT_INSTALL", verdict(0, "LOW", "DO_NOT_INSTALL")),
    ] {
        let data = TempDir::new().unwrap();
        let stores = TempDir::new().unwrap();
        let scanner = FakeScanner::blocking_collection(safe(), blocked);
        let fetcher = FakeFetcher::new("pdf");
        let index = open(&data, scanner.clone(), fetcher.clone()).await;

        let error = index
            .index_repo("acme/skills", SyncPolicy::normal())
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                SkillsError::ScannerBlocked { .. } | SkillsError::Scanner(_)
            ),
            "{case}: {error}"
        );
        assert_eq!(
            fetcher.calls().len(),
            1,
            "{case}: the fixture was fetched once"
        );
        assert_no_index_writes(&index, case).await;
        assert_eq!(
            fs::read_dir(stores.path()).unwrap().count(),
            0,
            "{case}: no harness store may be touched"
        );
    }
}

#[tokio::test]
async fn a_real_missing_scanner_binary_blocks_before_fetch_and_writes_nothing() {
    let data = TempDir::new().unwrap();
    let stores = TempDir::new().unwrap();
    let fetcher = FakeFetcher::new("pdf");
    let index = SkillIndex::open(data.path())
        .await
        .unwrap()
        .with_scanner(Arc::new(SkillSpector::with_binary(
            data.path().join("skillspector-does-not-exist"),
        )))
        .with_fetcher(Arc::new(fetcher.clone()));

    let error = index
        .index_repo("acme/skills", SyncPolicy::normal())
        .await
        .unwrap_err();

    assert!(matches!(error, SkillsError::Scanner(_)), "{error}");
    assert!(
        fetcher.calls().is_empty(),
        "the real scanner must fail before the repository walk"
    );
    assert_no_index_writes(&index, "real missing binary").await;
    assert_eq!(fs::read_dir(stores.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn malformed_incomplete_and_omitted_collection_reports_write_nothing() {
    for (case, reason) in [
        (
            "malformed report",
            "returned output that is not JSON: no skill was touched",
        ),
        (
            "incomplete report",
            "recursive scan reported incomplete coverage; no skill was touched",
        ),
        (
            "omitted skill",
            "recursive scan omitted 1 skill(s); no skill was touched",
        ),
        (
            "failed child scan",
            "a recursive skill scan did not execute successfully; no skill was touched",
        ),
    ] {
        let data = TempDir::new().unwrap();
        let stores = TempDir::new().unwrap();
        let scanner = FakeScanner::collection(
            Answer::Verdict(safe()),
            CollectionAnswer::Error(reason.into()),
        );
        let fetcher = FakeFetcher::new("pdf");
        let index = open(&data, scanner, fetcher.clone()).await;

        let error = index
            .index_repo("acme/skills", SyncPolicy::normal())
            .await
            .unwrap_err();

        assert!(matches!(error, SkillsError::Scanner(_)), "{case}: {error}");
        assert_eq!(
            fetcher.calls().len(),
            1,
            "{case}: the fixture was fetched once"
        );
        assert_no_index_writes(&index, case).await;
        assert_eq!(
            fs::read_dir(stores.path()).unwrap().count(),
            0,
            "{case}: no harness store may be touched"
        );
    }
}

#[tokio::test]
async fn caution_is_admitted_recorded_and_installable() {
    let data = TempDir::new().unwrap();
    let stores = TempDir::new().unwrap();
    let scanner = FakeScanner::scanning(caution());
    let fetcher = FakeFetcher::new("pdf");
    let index = open(&data, scanner, fetcher).await;

    let report = index
        .index_repo("acme/skills", SyncPolicy::normal())
        .await
        .expect("CAUTION is admitted");
    let scan = report.scan.expect("the index receipt carries the verdict");
    assert_eq!(scan.recommendation, "CAUTION");
    assert_eq!(scan.score, 45);
    assert_eq!(report.blocked, 0);
    assert_eq!(blob_count(&index), 1);
    assert!(
        index
            .body("github:acme/skills:pdf/SKILL.md")
            .await
            .unwrap()
            .is_some()
    );

    let roots = [stores.path().join("agents"), stores.path().join("claude")];
    let installed = index
        .install("github:acme/skills:pdf/SKILL.md", &roots)
        .await
        .expect("CAUTION is installable");
    assert_eq!(installed.scan.recommendation, "CAUTION");
    assert_eq!(installed.scan.score, 45);
    assert_eq!(installed.paths.len(), 2);
    for path in &installed.paths {
        assert_eq!(fs::read_to_string(path).unwrap(), body("pdf"));
    }
    assert_eq!(blob_count(&index), 1, "an install adds no body blob");
}

#[tokio::test]
async fn a_blocked_install_writes_no_row_blob_or_harness_file() {
    let data = TempDir::new().unwrap();
    let stores = TempDir::new().unwrap();
    let index = open(
        &data,
        FakeScanner::scanning(verdict(91, "CRITICAL", "DO_NOT_INSTALL")),
        FakeFetcher::new("pdf"),
    )
    .await;
    let text = body("pdf");
    let sha = index.blobs().put(text.as_bytes()).unwrap();
    index
        .write_skill(&SkillRecord {
            id: "github:acme/skills:pdf/SKILL.md".to_string(),
            name: "PDF Tools".to_string(),
            description: "test".to_string(),
            source: "github".to_string(),
            repo: Some("acme/skills".to_string()),
            path: Some("pdf/SKILL.md".to_string()),
            url: None,
            body_sha: Some(sha),
            grade: None,
            grade_scope: None,
            license: None,
            stars: None,
            installs: None,
            tokens: None,
            capabilities: Vec::new(),
            simhash: None,
        })
        .await
        .unwrap();
    let skills_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    let blobs_before = blob_count(&index);

    let roots = [stores.path().join("agents")];
    let error = index
        .install("github:acme/skills:pdf/SKILL.md", &roots)
        .await
        .expect_err("a blocked body must not be copied");

    assert!(matches!(error, SkillsError::ScannerBlocked { .. }));
    let skills_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    assert_eq!(skills_after, skills_before);
    assert_eq!(blob_count(&index), blobs_before, "no new body blob");
    assert_eq!(
        fs::read_dir(stores.path()).unwrap().count(),
        0,
        "the harness store must still be empty"
    );
    assert!(!roots[0].exists(), "no install directory may be created");
}

#[tokio::test]
async fn an_admitted_single_skill_url_is_fetched_after_the_preflight() {
    let data = TempDir::new().unwrap();
    let scanner = FakeScanner::scanning(safe());
    let fetcher = FakeFetcher::new("pdf");
    let index = open(&data, scanner.clone(), fetcher.clone()).await;

    index
        .index_repo(
            "https://github.com/acme/skills/tree/main/pdf",
            SyncPolicy::normal(),
        )
        .await
        .unwrap();

    assert_eq!(
        scanner.targets(),
        vec!["https://github.com/acme/skills/tree/main/pdf".to_string()]
    );
    assert_eq!(
        fetcher.calls(),
        vec!["https://github.com/acme/skills/tree/main/pdf|slug=-|limit=-".to_string()]
    );
}
