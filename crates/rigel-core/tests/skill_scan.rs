//! The security gate's ordering: nothing is fetched or written before SkillSpector answers.
//!
//! These tests inject a scanner because the real one is an optional Python tool. What matters
//! here is not the scanner's own analysis, but that Rigel invokes it first and fails closed.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rigel_core::skills::{
    ScanVerdict, SkillIndex, SkillRecord, SkillScanner, SkillsError, SyncPolicy,
};
use tempfile::TempDir;

#[derive(Clone)]
enum Answer {
    Verdict(ScanVerdict),
    Error(String),
}

#[derive(Clone)]
struct FakeScanner {
    answer: Answer,
    targets: Arc<Mutex<Vec<String>>>,
}

impl FakeScanner {
    fn blocked() -> Self {
        Self {
            answer: Answer::Verdict(ScanVerdict {
                score: 91,
                severity: "CRITICAL".into(),
                recommendation: "DO_NOT_INSTALL".into(),
                findings: 3,
                scanner: "fake".into(),
            }),
            targets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn missing() -> Self {
        Self {
            answer: Answer::Error("SkillSpector is required".into()),
            targets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn targets(&self) -> Vec<String> {
        self.targets.lock().unwrap().clone()
    }
}

#[async_trait]
impl SkillScanner for FakeScanner {
    async fn scan(&self, target: &str) -> Result<ScanVerdict, SkillsError> {
        self.targets.lock().unwrap().push(target.to_string());
        match &self.answer {
            Answer::Verdict(verdict) => Ok(verdict.clone()),
            Answer::Error(error) => Err(SkillsError::Scanner(error.clone())),
        }
    }
}

async fn index(tmp: &TempDir, scanner: FakeScanner) -> SkillIndex {
    SkillIndex::open(tmp.path())
        .await
        .unwrap()
        .with_scanner(Arc::new(scanner))
}

#[tokio::test]
async fn a_blocked_repository_is_refused_before_download() {
    let tmp = TempDir::new().unwrap();
    let scanner = FakeScanner::blocked();
    let index = index(&tmp, scanner.clone()).await;

    let error = index
        .index_repo(
            "https://github.com/acme/skills/tree/main/pdf",
            SyncPolicy::normal(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, SkillsError::ScannerBlocked { .. }));
    assert_eq!(
        scanner.targets(),
        vec!["https://github.com/acme/skills/tree/main/pdf".to_string()]
    );
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    let skills: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    assert_eq!(sources, 0, "a blocked scan must not create a source row");
    assert_eq!(skills, 0, "a blocked scan must not download a body");
}

#[tokio::test]
async fn a_missing_scanner_fails_closed_before_download() {
    let tmp = TempDir::new().unwrap();
    let scanner = FakeScanner::missing();
    let index = index(&tmp, scanner.clone()).await;

    let error = index
        .index_repo("acme/skills", SyncPolicy::normal())
        .await
        .unwrap_err();

    assert!(matches!(error, SkillsError::Scanner(_)));
    assert_eq!(scanner.targets().len(), 1);
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
        .fetch_one(index.store().pool())
        .await
        .unwrap();
    assert_eq!(sources, 0);
}

#[tokio::test]
async fn a_blocked_install_writes_no_skill_file() {
    let data = TempDir::new().unwrap();
    let stores = TempDir::new().unwrap();
    let index = index(&data, FakeScanner::blocked()).await;
    let body = "---\nname: PDF Tools\ndescription: test\n---\n\nRead the PDF.\n";
    let sha = index.blobs().put(body.as_bytes()).unwrap();
    index
        .write_skill(&SkillRecord {
            id: "test:pdf".to_string(),
            name: "PDF Tools".to_string(),
            description: "test".to_string(),
            source: "test".to_string(),
            repo: None,
            path: None,
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

    let roots = [stores.path().join("agents")];
    let error = index
        .install("test:pdf", &roots)
        .await
        .expect_err("a blocked body must not be copied");

    assert!(matches!(error, SkillsError::ScannerBlocked { .. }));
    assert_eq!(
        std::fs::read_dir(stores.path()).unwrap().count(),
        0,
        "the harness store must still be empty"
    );
}
