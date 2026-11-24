//! The security gate every skill passes before Rigel accepts or installs it.
//!
//! [NVIDIA SkillSpector](https://github.com/NVIDIA/SkillSpector) is a separate Apache-2.0
//! scanner. Rigel runs its static pass (`--no-llm`) so no skill body and no prompt is sent to a
//! model provider; the scanner reads a repository or a temporary `SKILL.md` and returns a JSON
//! risk assessment. The policy is deliberately fail-closed: a missing scanner, an unreadable
//! report, an error exit, a score above 50, or a `DO_NOT_INSTALL` recommendation stops the
//! operation. `CAUTION` is allowed but carried in the receipt.
//!
//! There are three points of use:
//!
//! - a single-skill URL is preflighted before Rigel fetches it;
//! - a repository's exact bodies are scanned together after the tree walk and before any row or
//!   blob is written, so one blocked skill does not hide the safe ones;
//! - an install scans the exact indexed `SKILL.md` that would be written, in a disposable
//!   directory, before the harness store is touched.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use super::github::parse_repo_input;
use crate::skills::SkillsError;

/// The command in the error a user sees when the scanner is not installed.
pub const INSTALL_HINT: &str = "uv tool install git+https://github.com/NVIDIA/SkillSpector.git";

const BIN_ENV: &str = "RIGEL_SKILLSPECTOR_BIN";
const BIN: &str = "skillspector";

/// What SkillSpector said about one target. The recommendation is the policy input; score and
/// findings are kept for the receipt and the shelf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanVerdict {
    /// SkillSpector's risk score, 0–100.
    pub score: i64,
    /// `LOW`, `MEDIUM`, `HIGH` or `CRITICAL`.
    pub severity: String,
    /// `SAFE`, `CAUTION` or `DO_NOT_INSTALL`, normalized to the documented spelling.
    pub recommendation: String,
    /// Number of issue records in the report, when the report carries them.
    pub findings: usize,
    /// The scanner and version that produced the report.
    pub scanner: String,
}

impl ScanVerdict {
    /// The one policy decision. A report that says safe but carries a blocking score or
    /// severity is still refused: a caller never has to combine the fields itself.
    pub fn blocked(&self) -> bool {
        self.recommendation == "DO_NOT_INSTALL"
            || self.score > 50
            || self.severity == "HIGH"
            || self.severity == "CRITICAL"
    }

    /// One line for a CLI or a Settings receipt.
    pub fn summary(&self) -> String {
        format!(
            "{} · {}/100 · {} · {} finding(s)",
            self.recommendation, self.score, self.severity, self.findings
        )
    }

    pub(crate) fn severity_rank(&self) -> u8 {
        match self.severity.as_str() {
            "CRITICAL" => 4,
            "HIGH" => 3,
            "MEDIUM" => 2,
            "LOW" => 1,
            _ => 0,
        }
    }

    /// Parse the single-skill JSON shape SkillSpector documents. Anything missing a score or
    /// recommendation is an error, not an assumed pass.
    pub fn from_report(report: &str) -> Result<Self, SkillsError> {
        Self::from_value(&parse_report(report)?)
    }

    fn from_value(value: &Value) -> Result<Self, SkillsError> {
        let (score, severity, recommendation) = if let Some(risk) = value.get("risk_assessment") {
            (
                risk.get("score").and_then(Value::as_i64),
                risk.get("severity").and_then(Value::as_str),
                risk.get("recommendation").and_then(Value::as_str),
            )
        } else if value
            .get("multi_skill")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            (
                value.get("max_risk_score").and_then(Value::as_i64),
                value.get("risk_severity").and_then(Value::as_str),
                value.get("risk_recommendation").and_then(Value::as_str),
            )
        } else {
            return Err(SkillsError::Scanner(
                "report has no risk_assessment (or multi_skill aggregate); no skill was touched"
                    .into(),
            ));
        };

        let score = score.ok_or_else(|| {
            SkillsError::Scanner("report has no numeric risk score; no skill was touched".into())
        })?;
        let severity = severity
            .map(|value| value.trim().to_ascii_uppercase())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                SkillsError::Scanner("report has no severity; no skill was touched".into())
            })?;
        let recommendation = recommendation
            .map(normalize_recommendation)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                SkillsError::Scanner("report has no recommendation; no skill was touched".into())
            })?;

        let findings = value
            .get("issues")
            .and_then(Value::as_array)
            .map(Vec::len)
            .or_else(|| {
                value
                    .get("public_finding_records")
                    .and_then(Value::as_u64)
                    .map(|count| count as usize)
            })
            .unwrap_or(0);

        Ok(Self {
            score,
            severity,
            recommendation,
            findings,
            scanner: scanner_name(value),
        })
    }
}

fn scanner_name(value: &Value) -> String {
    let version = value
        .pointer("/metadata/skillspector_version")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    match version {
        Some(version) => format!("skillspector {version}"),
        None => "skillspector".to_string(),
    }
}

fn parse_report(report: &str) -> Result<Value, SkillsError> {
    serde_json::from_str(report.trim()).map_err(|error| {
        SkillsError::Scanner(format!(
            "returned output that is not JSON: {error}; no skill was touched"
        ))
    })
}

fn normalize_recommendation(raw: &str) -> String {
    raw.trim()
        .to_ascii_uppercase()
        .replace([' ', '-'], "_")
        .trim_matches('_')
        .to_string()
}

/// One skill inside a collection scan. `path` is the directory SkillSpector reported, which is
/// how Rigel maps a verdict back to the exact `SKILL.md` it wrote for scanning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScannedSkill {
    pub name: String,
    pub path: String,
    pub verdict: ScanVerdict,
}

/// A recursive scan of several bodies at once, plus its aggregate. The per-skill list is the
/// enforcement input; the aggregate is kept for the receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionScan {
    pub aggregate: ScanVerdict,
    pub skills: Vec<ScannedSkill>,
}

impl CollectionScan {
    fn from_report(report: &str) -> Result<Self, SkillsError> {
        let value = parse_report(report)?;
        if !value
            .get("multi_skill")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(SkillsError::Scanner(
                "recursive scan did not return a multi_skill report; no skill was touched".into(),
            ));
        }
        if value.get("execution_successful").and_then(Value::as_bool) == Some(false) {
            return Err(SkillsError::Scanner(
                "recursive scan did not execute successfully; no skill was touched".into(),
            ));
        }
        let omitted = value
            .get("skills_omitted")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + value
                .get("skills_output_omitted")
                .and_then(Value::as_u64)
                .unwrap_or(0);
        if omitted > 0 {
            return Err(SkillsError::Scanner(format!(
                "recursive scan omitted {omitted} skill(s); no skill was touched"
            )));
        }
        if value
            .pointer("/analysis_completeness/is_complete")
            .and_then(Value::as_bool)
            == Some(false)
        {
            return Err(SkillsError::Scanner(
                "recursive scan reported incomplete coverage; no skill was touched".into(),
            ));
        }

        let aggregate = ScanVerdict::from_value(&value)?;
        let raw_skills = value
            .get("skills")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                SkillsError::Scanner(
                    "recursive scan returned no per-skill results; no skill was touched".into(),
                )
            })?;
        let count = value
            .get("skill_count")
            .and_then(Value::as_u64)
            .unwrap_or(raw_skills.len() as u64);
        if count as usize != raw_skills.len() {
            return Err(SkillsError::Scanner(format!(
                "recursive scan reported {count} skill(s) but returned {} result(s); no skill was touched",
                raw_skills.len()
            )));
        }

        let mut skills = Vec::with_capacity(raw_skills.len());
        for item in raw_skills {
            if item.get("execution_successful").and_then(Value::as_bool) == Some(false) {
                return Err(SkillsError::Scanner(
                    "a recursive skill scan did not execute successfully; no skill was touched"
                        .into(),
                ));
            }
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    SkillsError::Scanner(
                        "a recursive skill result has no path; no skill was touched".into(),
                    )
                })?;
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(path)
                .to_string();
            let mut verdict = ScanVerdict::from_value(item)?;
            // Child records do not repeat the scanner version.
            verdict.scanner = aggregate.scanner.clone();
            skills.push(ScannedSkill {
                name,
                path: path.trim_end_matches('/').to_string(),
                verdict,
            });
        }

        Ok(Self { aggregate, skills })
    }
}

/// The gate's seam. Tests inject a fake; production uses [`SkillSpector`].
#[async_trait]
pub trait SkillScanner: Send + Sync {
    /// Scan a repository URL or a local directory/file SkillSpector can read.
    async fn scan(&self, target: &str) -> Result<ScanVerdict, SkillsError>;

    /// Scan immediate child directories as independent skills. The default keeps simple test
    /// fakes working; production overrides it with SkillSpector's recursive JSON report.
    async fn scan_collection(
        &self,
        target: &str,
        _max_skills: usize,
    ) -> Result<CollectionScan, SkillsError> {
        let verdict = self.scan(target).await?;
        Ok(CollectionScan {
            aggregate: verdict.clone(),
            skills: vec![ScannedSkill {
                name: "SKILL.md".to_string(),
                path: "SKILL.md".to_string(),
                verdict,
            }],
        })
    }
}

/// The installed `skillspector` executable. `RIGEL_SKILLSPECTOR_BIN` overrides discovery, which
/// is useful on Windows where uv's shim directory is not always on the app's PATH. A test can
/// also pin a binary directly, so the missing-binary path is exercised without global state.
#[derive(Debug, Clone, Default)]
pub struct SkillSpector {
    binary: Option<OsString>,
}

impl SkillSpector {
    pub fn with_binary(binary: impl Into<OsString>) -> Self {
        Self {
            binary: Some(binary.into()),
        }
    }

    fn binary(&self) -> OsString {
        self.binary
            .clone()
            .or_else(|| std::env::var_os(BIN_ENV))
            .unwrap_or_else(|| OsString::from(BIN))
    }

    async fn run(&self, target: &str, extra: &[&str]) -> Result<Output, SkillsError> {
        self.run_binary(&self.binary(), target, extra).await
    }

    /// The executable is a parameter so the missing-binary and error-exit paths are testable
    /// without changing process-wide environment variables.
    async fn run_binary(
        &self,
        binary: &std::ffi::OsStr,
        target: &str,
        extra: &[&str],
    ) -> Result<Output, SkillsError> {
        let target = target.trim();
        if target.is_empty() {
            return Err(SkillsError::Scanner(
                "refusing an empty scan target; no skill was touched".into(),
            ));
        }

        let mut command = Command::new(binary);
        command
            .arg("scan")
            .arg(target)
            .arg("--no-llm")
            .arg("--format")
            .arg("json");
        for argument in extra {
            command.arg(argument);
        }
        command
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => SkillsError::Scanner(format!(
                    "SkillSpector is required before a skill can be downloaded or installed, \
                     but `{}` was not found. Install it with `{INSTALL_HINT}`, add it to PATH, \
                     or set {BIN_ENV} to its executable",
                    Path::new(binary).display()
                )),
                _ => SkillsError::Scanner(format!(
                    "could not run SkillSpector (`{}`): {error}; no skill was touched",
                    Path::new(binary).display()
                )),
            })
    }
}

#[async_trait]
impl SkillScanner for SkillSpector {
    async fn scan(&self, target: &str) -> Result<ScanVerdict, SkillsError> {
        let output = self.run(target, &[]).await?;
        let status = output.status;
        let detail = failure_detail(&output.stderr);
        let parsed = ScanVerdict::from_report(&String::from_utf8_lossy(&output.stdout));
        finish_one(status, detail, parsed)
    }

    async fn scan_collection(
        &self,
        target: &str,
        max_skills: usize,
    ) -> Result<CollectionScan, SkillsError> {
        let max_skills = max_skills.max(1).to_string();
        let output = self
            .run(
                target,
                &["--recursive", "--max-skills", max_skills.as_str()],
            )
            .await?;
        let status = output.status;
        let detail = failure_detail(&output.stderr);
        let parsed = CollectionScan::from_report(&String::from_utf8_lossy(&output.stdout));
        finish_collection(status, detail, parsed)
    }
}

fn failure_detail(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim().replace(['\r', '\n'], " ");
    truncate(&detail, 500)
}

fn exit_code(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string())
}

fn finish_one(
    status: std::process::ExitStatus,
    detail: String,
    parsed: Result<ScanVerdict, SkillsError>,
) -> Result<ScanVerdict, SkillsError> {
    match parsed {
        Ok(verdict) if status.success() => Ok(verdict),
        // Exit 1 is SkillSpector's "scan completed and blocked" code.
        Ok(verdict) if status.code() == Some(1) && verdict.blocked() => Ok(verdict),
        Ok(_) => Err(SkillsError::Scanner(format!(
            "SkillSpector exited with code {}{}; no skill was touched",
            exit_code(&status),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))),
        Err(parse_error) => Err(SkillsError::Scanner(format!(
            "SkillSpector exited with code {} and {parse_error}{}; no skill was touched",
            exit_code(&status),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))),
    }
}

fn finish_collection(
    status: std::process::ExitStatus,
    detail: String,
    parsed: Result<CollectionScan, SkillsError>,
) -> Result<CollectionScan, SkillsError> {
    match parsed {
        // Exit 1 is expected when one child is blocked; the per-skill list still decides what
        // may be ingested.
        Ok(scan) if status.success() || status.code() == Some(1) => Ok(scan),
        Ok(_) => Err(SkillsError::Scanner(format!(
            "SkillSpector exited with code {}{}; no skill was touched",
            exit_code(&status),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))),
        Err(parse_error) => Err(SkillsError::Scanner(format!(
            "SkillSpector exited with code {} and {parse_error}{}; no skill was touched",
            exit_code(&status),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))),
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut clipped: String = value.chars().take(limit).collect();
    clipped.push('…');
    clipped
}

/// Turn every accepted GitHub spelling into one repository or subdirectory URL. The scanner
/// must see the same target Rigel would fetch, not a bare `owner/repo` string it might not
/// understand.
pub(crate) fn canonical_target(raw: &str) -> String {
    let raw = raw.trim();
    let Some(repo) = parse_repo_input(raw) else {
        return raw.to_string();
    };
    let base = format!("https://github.com/{}/{}", repo.owner, repo.repo);
    match (repo.git_ref.as_deref(), repo.path.as_deref()) {
        (Some(git_ref), Some(path)) if !path.is_empty() => format!(
            "{base}/tree/{}/{}",
            encode_segment(git_ref),
            encode_segment(path)
        ),
        _ => base,
    }
}

/// Percent-encode a path's segments while keeping `/` as the separator. GitHub repository names
/// cannot contain most of the encoded characters, but a pasted directory path can.
fn encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            // A URL path is already percent-encoded; do not turn `%20` into `%2520`.
            encoded.push('%');
            encoded.push(bytes[index + 1] as char);
            encoded.push(bytes[index + 2] as char);
            index += 3;
            continue;
        }
        let plain = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || byte == b'/';
        if plain {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
        index += 1;
    }
    encoded
}

/// A stable local directory for scanning one body or a temporary collection. The caller removes
/// it after the scan.
pub(crate) fn body_scan_dir() -> PathBuf {
    std::env::temp_dir().join(format!("rigel-skill-scan-{}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_single_skill_report() {
        let verdict = ScanVerdict::from_report(
            r#"{
                "skill": {"name": "pdf", "source": "github"},
                "risk_assessment": {"score": 0, "severity": "LOW", "recommendation": "SAFE"},
                "issues": [],
                "metadata": {"skillspector_version": "2.13.6"}
            }"#,
        )
        .unwrap();
        assert_eq!(
            verdict,
            ScanVerdict {
                score: 0,
                severity: "LOW".into(),
                recommendation: "SAFE".into(),
                findings: 0,
                scanner: "skillspector 2.13.6".into(),
            }
        );
        assert!(!verdict.blocked());
    }

    #[test]
    fn parses_the_multi_skill_aggregate() {
        let verdict = ScanVerdict::from_report(
            r#"{
                "multi_skill": true,
                "max_risk_score": 72,
                "risk_severity": "HIGH",
                "risk_recommendation": "DO_NOT_INSTALL",
                "skills_scanned": 4,
                "public_finding_records": 9
            }"#,
        )
        .unwrap();
        assert_eq!(verdict.score, 72);
        assert_eq!(verdict.findings, 9);
        assert!(verdict.blocked());
    }

    #[test]
    fn parses_per_skill_results_for_filtering() {
        let scan = CollectionScan::from_report(
            r#"{
                "multi_skill": true,
                "skill_count": 2,
                "skills_scanned": 2,
                "skills_omitted": 0,
                "skills_output_omitted": 0,
                "max_risk_score": 56,
                "risk_severity": "HIGH",
                "risk_recommendation": "DO_NOT_INSTALL",
                "execution_successful": true,
                "analysis_completeness": {"is_complete": true},
                "metadata": {"skillspector_version": "2.13.7"},
                "skills": [
                    {
                        "name": "safe",
                        "path": "skill-0000",
                        "risk_assessment": {"score": 0, "severity": "LOW", "recommendation": "SAFE"},
                        "issues": []
                    },
                    {
                        "name": "bad",
                        "path": "skill-0001",
                        "risk_assessment": {"score": 56, "severity": "HIGH", "recommendation": "DO_NOT_INSTALL"},
                        "issues": [{"id": "x"}, {"id": "y"}]
                    }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(scan.skills.len(), 2);
        assert_eq!(scan.skills[0].path, "skill-0000");
        assert!(!scan.skills[0].verdict.blocked());
        assert!(scan.skills[1].verdict.blocked());
        assert_eq!(scan.skills[1].verdict.findings, 2);
        assert_eq!(scan.skills[1].verdict.scanner, "skillspector 2.13.7");
    }

    #[test]
    fn an_incomplete_collection_fails_closed() {
        for report in [
            r#"{"multi_skill": true, "skill_count": 2, "skills_omitted": 1, "skills": []}"#,
            r#"{"multi_skill": true, "skill_count": 1, "skills_output_omitted": 1, "skills": []}"#,
            r#"{"multi_skill": true, "skill_count": 1, "analysis_completeness": {"is_complete": false}, "skills": []}"#,
            r#"{"multi_skill": true, "skill_count": 2, "skills": []}"#,
        ] {
            assert!(CollectionScan::from_report(report).is_err(), "{report}");
        }
    }

    #[test]
    fn a_high_score_blocks_even_when_the_label_is_wrong() {
        let mut verdict = ScanVerdict {
            score: 51,
            severity: "MEDIUM".into(),
            recommendation: "CAUTION".into(),
            findings: 1,
            scanner: "skillspector".into(),
        };
        assert!(verdict.blocked());
        verdict.score = 50;
        assert!(!verdict.blocked());
        verdict.recommendation = normalize_recommendation("do not install");
        assert!(verdict.blocked());
    }

    #[test]
    fn a_report_without_a_decision_fails_closed() {
        assert!(ScanVerdict::from_report("{\"skill\": {}}").is_err());
        assert!(
            ScanVerdict::from_report(
                "{\"risk_assessment\": {\"score\": 0, \"severity\": \"LOW\"}}"
            )
            .is_err()
        );
        assert!(ScanVerdict::from_report("not json").is_err());
    }

    #[test]
    fn malformed_reports_and_incomplete_child_execution_fail_closed() {
        for report in [
            "not json",
            "{}",
            r#"{"skill": {}}"#,
            r#"{"risk_assessment": {"score": 0}}"#,
        ] {
            assert!(ScanVerdict::from_report(report).is_err(), "{report}");
        }

        for report in [
            r#"{"multi_skill": true, "execution_successful": false, "skill_count": 1, "skills": []}"#,
            r#"{
                "multi_skill": true,
                "skill_count": 1,
                "max_risk_score": 0,
                "risk_severity": "LOW",
                "risk_recommendation": "SAFE",
                "skills": [{
                    "name": "pdf",
                    "path": "skill-0000",
                    "execution_successful": false,
                    "risk_assessment": {"score": 0, "severity": "LOW", "recommendation": "SAFE"}
                }]
            }"#,
        ] {
            assert!(CollectionScan::from_report(report).is_err(), "{report}");
        }
    }

    #[test]
    fn a_nonzero_scanner_exit_fails_closed() {
        let status = exit_status(2);
        let safe = ScanVerdict {
            score: 0,
            severity: "LOW".into(),
            recommendation: "SAFE".into(),
            findings: 0,
            scanner: "fake".into(),
        };
        assert!(finish_one(status, String::new(), Ok(safe.clone())).is_err());
        let collection = CollectionScan {
            aggregate: safe,
            skills: Vec::new(),
        };
        assert!(finish_collection(exit_status(2), String::new(), Ok(collection)).is_err());
    }

    #[tokio::test]
    async fn a_missing_scanner_binary_fails_closed() {
        let binary = std::env::temp_dir().join(format!(
            "rigel-no-skillspector-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let error = SkillSpector::with_binary(binary)
            .scan("https://github.com/acme/skills")
            .await
            .expect_err("a missing binary must not scan");
        let message = error.to_string();
        assert!(message.contains("was not found"), "{message}");
        assert!(message.contains("Install it with"), "{message}");
    }

    fn exit_status(code: i32) -> std::process::ExitStatus {
        #[cfg(windows)]
        let status = std::process::Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status();
        #[cfg(not(windows))]
        let status = std::process::Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .status();
        status.unwrap()
    }

    #[test]
    fn accepted_github_spellings_become_one_url() {
        assert_eq!(
            canonical_target("acme/skills"),
            "https://github.com/acme/skills"
        );
        assert_eq!(
            canonical_target("https://github.com/acme/skills.git"),
            "https://github.com/acme/skills"
        );
        assert_eq!(
            canonical_target("https://github.com/acme/skills/tree/main/pdf%20tools"),
            "https://github.com/acme/skills/tree/main/pdf%20tools"
        );
        assert_eq!(
            canonical_target("acme/skills/tree/main/pdf tools"),
            "https://github.com/acme/skills/tree/main/pdf%20tools"
        );
        assert_eq!(canonical_target("not a repo"), "not a repo");
    }
}
