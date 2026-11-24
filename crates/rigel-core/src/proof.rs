//! Proof-carrying changes (roadmap #25): evidence that a specific working-tree state was
//! actually tested.
//!
//! A bundle records the base commit, every changed file with its content hash, and the exact
//! commands that ran with their exit codes, counts and bounded output. Re-recording the same
//! tree adds evidence instead of replacing it. `verify` checks the bundle's internal hashes,
//! then fingerprints the working tree again: a bundle whose files moved is stale, not proof.
//!
//! This is local evidence, not a signature. Anyone who can rewrite the bundle can rewrite the
//! hashes; the point is to make an honest claim checkable and to catch accidental drift between
//! the run and a review. Bundles live under `.rigel/proofs/` and can be copied into a review.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::testimpact::{self, ChangedFile};
use crate::testrun::TestRun;

/// Versioned so a future format can be refused rather than silently misread.
pub const SCHEMA: &str = "rigel.proof/1";
/// Project-relative directory that holds bundles.
pub const PROOFS_DIR: &str = ".rigel/proofs";
/// Evidence output is bounded; the full text is not needed to check that the run happened.
pub const MAX_EVIDENCE_OUTPUT: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofFile {
    pub path: String,
    /// Git's status letter, or `?` for an untracked file.
    pub status: String,
    /// SHA-256 of the file at recording time, or `deleted` / `missing`.
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub command: String,
    pub exit_code: Option<i32>,
    pub ok: bool,
    pub passed: usize,
    pub failed: usize,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// The first [`MAX_EVIDENCE_OUTPUT`] bytes, as text.
    pub output: String,
    pub output_sha256: String,
    pub output_truncated: bool,
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofBundle {
    pub schema: String,
    pub id: String,
    pub created_at_ms: i64,
    /// The commit the change is measured against, resolved at recording time.
    pub base: String,
    /// Stable digest of `files`; see [`tree_digest`].
    pub tree: String,
    pub files: Vec<ProofFile>,
    pub evidence: Vec<Evidence>,
    /// Every recorded command passed, and a source change carries at least one command.
    pub ok: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProofSummary {
    pub id: String,
    pub created_at_ms: i64,
    pub base: String,
    pub tree: String,
    pub files: usize,
    pub evidence: usize,
    pub ok: bool,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProofVerification {
    pub id: String,
    pub path: PathBuf,
    /// The bundle's own hashes and schema are consistent.
    pub valid: bool,
    /// The working tree still matches the bundle's `tree` digest.
    pub fresh: bool,
    /// `fresh`, `stale`, or `unknown` when the base commit can no longer be read.
    pub freshness: String,
    /// Every recorded command passed, with evidence when a source file changed.
    pub ok: bool,
    pub evidence: usize,
    /// Files that differ from the bundle when it is stale.
    pub changed: Vec<String>,
    /// Why the bundle is not valid, fresh or successful; empty when it is all three.
    pub reasons: Vec<String>,
}

impl ProofVerification {
    /// The one predicate a merge gate should use.
    pub fn verified(&self) -> bool {
        self.valid && self.fresh && self.ok
    }
}

#[derive(Debug, Error)]
pub enum ProofError {
    #[error("proofs need a git repository with at least one commit: {0}")]
    Git(String),
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no proof bundle matches {0}")]
    NotFound(String),
    #[error("more than one proof bundle matches {0}")]
    Ambiguous(String),
    #[error("proof bundle {path} is not valid: {reason}")]
    Invalid { path: PathBuf, reason: String },
    #[error("a proof command is required")]
    EmptyCommand,
}

/// A stable digest for the changed-file set. Sorted and content-aware; Git status is displayed
/// but not hashed, so committing a recorded change does not invalidate its evidence.
pub fn tree_digest(files: &[ProofFile]) -> String {
    let mut sorted: Vec<&ProofFile> = files.iter().collect();
    sorted.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = Sha256::new();
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn proofs_dir(root: &Path) -> PathBuf {
    root.join(PROOFS_DIR)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn git(root: &Path, args: &[&str]) -> Result<String, ProofError> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(root).args(args);
    crate::process::hide(&mut command);
    let output = command
        .output()
        .map_err(|error| ProofError::Git(format!("git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProofError::Git(format!(
            "git {}: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve the base revision to a commit. `HEAD` is the default; a symbolic name is accepted
/// and stored as the commit it named, so a later commit does not silently move the comparison.
pub fn resolve_base(root: &Path, since: &str) -> Result<String, ProofError> {
    if !root.is_dir() {
        return Err(ProofError::Git(format!(
            "the project folder is not available: {}",
            root.display()
        )));
    }
    let since = if since.trim().is_empty() {
        "HEAD"
    } else {
        since.trim()
    };
    let commit = git(
        root,
        &["rev-parse", "--verify", &format!("{since}^{{commit}}")],
    )?;
    if commit.is_empty() {
        return Err(ProofError::Git(format!(
            "{since} did not resolve to a commit"
        )));
    }
    Ok(commit)
}

fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok((hex(&hasher.finalize()), bytes))
}

fn file_entry(root: &Path, change: &ChangedFile) -> ProofFile {
    let path = root.join(&change.path);
    if change.status.starts_with('D') {
        return ProofFile {
            path: change.path.clone(),
            status: change.status.clone(),
            sha256: "deleted".into(),
            bytes: 0,
        };
    }
    match hash_file(&path) {
        Ok((sha256, bytes)) => ProofFile {
            path: change.path.clone(),
            status: change.status.clone(),
            sha256,
            bytes,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => ProofFile {
            path: change.path.clone(),
            status: change.status.clone(),
            sha256: "missing".into(),
            bytes: 0,
        },
        Err(_) => ProofFile {
            path: change.path.clone(),
            status: change.status.clone(),
            sha256: "unreadable".into(),
            bytes: 0,
        },
    }
}

/// Fingerprint the current changed-file set against `base`. The base is resolved first and the
/// returned bundle carries the commit, not the caller's symbolic name.
pub fn tree_state(
    root: &Path,
    since: &str,
) -> Result<(String, Vec<ProofFile>, String), ProofError> {
    let base = resolve_base(root, since)?;
    let changed = testimpact::changed_files(root, &base)
        .map_err(|error| ProofError::Git(error.to_string()))?;
    let files: Vec<ProofFile> = changed
        .iter()
        .map(|change| file_entry(root, change))
        .collect();
    let tree = tree_digest(&files);
    Ok((base, files, tree))
}

fn cap_output(output: &str) -> (String, bool) {
    if output.len() <= MAX_EVIDENCE_OUTPUT {
        return (output.to_string(), false);
    }
    let mut end = MAX_EVIDENCE_OUTPUT;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    (output[..end].to_string(), true)
}

/// Turn one real test run into evidence. Output is bounded and hashed as stored.
pub fn evidence_from_run(run: &TestRun) -> Evidence {
    let (output, capped) = cap_output(&run.output);
    Evidence {
        command: run.command.clone(),
        exit_code: run.exit_code,
        ok: run.ok,
        passed: run.passed,
        failed: run.failed,
        timed_out: run.timed_out,
        duration_ms: run.duration_ms,
        output_sha256: hex(&Sha256::digest(output.as_bytes())),
        output,
        output_truncated: capped || run.truncated,
        recorded_at_ms: now_ms(),
    }
}

/// True when the changed set contains a source file, using the same extension list the test
/// selector uses. A documentation-only change may legitimately carry no command.
fn needs_evidence(files: &[ProofFile]) -> bool {
    files
        .iter()
        .any(|file| testimpact::is_source_path(&file.path))
}

fn write_bundle(path: &Path, bundle: &ProofBundle) -> Result<(), ProofError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ProofError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let text = serde_json::to_string_pretty(bundle).map_err(|error| ProofError::Invalid {
        path: path.to_path_buf(),
        reason: format!("could not encode the bundle: {error}"),
    })?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text).map_err(|source| ProofError::Io {
        path: temp.clone(),
        source,
    })?;
    if path.exists() {
        fs::remove_file(path).map_err(|source| ProofError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    fs::rename(&temp, path).map_err(|source| ProofError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Record evidence for the current tree. If a bundle for the same tree already exists, the new
/// evidence is appended and the bundle is rewritten; otherwise a new bundle is created.
pub fn record(
    root: &Path,
    since: &str,
    evidence: Vec<Evidence>,
    notes: Vec<String>,
) -> Result<ProofBundle, ProofError> {
    let (base, files, tree) = tree_state(root, since)?;
    let id = tree.chars().take(16).collect::<String>();
    let path = proofs_dir(root).join(format!("{id}.json"));
    let mut bundle = if path.is_file() {
        read_bundle(&path)?
    } else {
        ProofBundle {
            schema: SCHEMA.into(),
            id: id.clone(),
            created_at_ms: now_ms(),
            base: base.clone(),
            tree: tree.clone(),
            files: files.clone(),
            evidence: Vec::new(),
            ok: false,
            notes: Vec::new(),
        }
    };
    if bundle.schema != SCHEMA || bundle.tree != tree {
        // A collision or a changed tree under the same id must never merge two changes.
        return Err(ProofError::Invalid {
            path,
            reason: "the existing bundle is for a different tree".into(),
        });
    }
    bundle.base = base;
    bundle.files = files;
    bundle.evidence.extend(evidence);
    for note in notes {
        if !bundle.notes.contains(&note) {
            bundle.notes.push(note);
        }
    }
    bundle.ok = bundle.evidence.iter().all(|entry| entry.ok)
        && (!bundle.evidence.is_empty() || !needs_evidence(&bundle.files));
    bundle.notes.retain(|note| {
        note != "a command failed" && note != "a source file changed without recorded evidence"
    });
    if !bundle.ok
        && bundle.evidence.iter().any(|entry| !entry.ok)
        && !bundle.notes.iter().any(|note| note == "a command failed")
    {
        bundle.notes.push("a command failed".into());
    } else if !bundle.ok
        && bundle.evidence.is_empty()
        && needs_evidence(&bundle.files)
        && !bundle
            .notes
            .iter()
            .any(|note| note == "a source file changed without recorded evidence")
    {
        bundle
            .notes
            .push("a source file changed without recorded evidence".into());
    }
    write_bundle(&path, &bundle)?;
    Ok(bundle)
}

fn read_bundle(path: &Path) -> Result<ProofBundle, ProofError> {
    let text = fs::read_to_string(path).map_err(|source| ProofError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|error| ProofError::Invalid {
        path: path.to_path_buf(),
        reason: format!("not a valid proof bundle: {error}"),
    })
}

/// Load one bundle by full id or an unambiguous prefix.
pub fn load(root: &Path, id_or_prefix: &str) -> Result<ProofBundle, ProofError> {
    let id = id_or_prefix.trim();
    let mut matches = Vec::new();
    for (path, bundle) in all_bundles(root)? {
        if bundle.id == id || bundle.id.starts_with(id) {
            matches.push((path, bundle));
        }
    }
    match matches.len() {
        0 => Err(ProofError::NotFound(id.to_string())),
        1 => Ok(matches.remove(0).1),
        _ => Err(ProofError::Ambiguous(id.to_string())),
    }
}

fn all_bundles(root: &Path) -> Result<Vec<(PathBuf, ProofBundle)>, ProofError> {
    let dir = proofs_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ProofError::Io { path: dir, source });
        }
    };
    let mut bundles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ProofError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        match read_bundle(&path) {
            Ok(bundle) => bundles.push((path, bundle)),
            Err(ProofError::Invalid { .. }) => continue,
            Err(error) => return Err(error),
        }
    }
    bundles.sort_by_key(|(_, bundle)| std::cmp::Reverse(bundle.created_at_ms));
    Ok(bundles)
}

/// Every readable bundle, newest first.
pub fn list(root: &Path) -> Result<Vec<ProofSummary>, ProofError> {
    Ok(all_bundles(root)?
        .into_iter()
        .map(|(path, bundle)| ProofSummary {
            id: bundle.id,
            created_at_ms: bundle.created_at_ms,
            base: bundle.base,
            tree: bundle.tree,
            files: bundle.files.len(),
            evidence: bundle.evidence.len(),
            ok: bundle.ok,
            path,
        })
        .collect())
}

fn changed_between(recorded: &[ProofFile], current: &[ProofFile]) -> Vec<String> {
    let mut paths: Vec<String> = recorded
        .iter()
        .map(|file| file.path.clone())
        .chain(current.iter().map(|file| file.path.clone()))
        .collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| {
            let left = recorded.iter().find(|file| &file.path == path);
            let right = current.iter().find(|file| &file.path == path);
            left != right
        })
        .collect()
}

/// Verify one bundle: internal consistency first, then freshness against the working tree.
pub fn verify(root: &Path, id_or_prefix: &str) -> Result<ProofVerification, ProofError> {
    let bundle = load(root, id_or_prefix)?;
    let path = proofs_dir(root).join(format!("{}.json", bundle.id));
    let mut reasons = Vec::new();
    let mut valid = true;
    if bundle.schema != SCHEMA {
        valid = false;
        reasons.push(format!(
            "schema is {:?}; expected {SCHEMA:?}",
            bundle.schema
        ));
    }
    if tree_digest(&bundle.files) != bundle.tree {
        valid = false;
        reasons.push("the stored file hashes do not match the tree digest".into());
    }
    for evidence in &bundle.evidence {
        if hex(&Sha256::digest(evidence.output.as_bytes())) != evidence.output_sha256 {
            valid = false;
            reasons.push(format!("evidence output was changed: {}", evidence.command));
        }
        if evidence.ok != (evidence.exit_code == Some(0) && !evidence.timed_out) {
            valid = false;
            reasons.push(format!(
                "evidence outcome does not match its exit code: {}",
                evidence.command
            ));
        }
    }
    if bundle.ok && bundle.evidence.is_empty() && needs_evidence(&bundle.files) {
        valid = false;
        reasons.push("a source change claims success with no evidence".into());
    }
    let current = tree_state(root, &bundle.base);
    let (fresh, freshness, changed) = match current {
        Ok((_, files, tree)) => {
            if tree == bundle.tree {
                (true, "fresh".to_string(), Vec::new())
            } else {
                let changed = changed_between(&bundle.files, &files);
                (false, "stale".to_string(), changed)
            }
        }
        Err(error) => {
            reasons.push(format!("the base commit could not be read: {error}"));
            (false, "unknown".to_string(), Vec::new())
        }
    };
    if !bundle.ok {
        reasons.push("the recorded evidence did not pass".into());
    }
    Ok(ProofVerification {
        id: bundle.id,
        path,
        valid,
        fresh,
        freshness,
        ok: bundle.ok,
        evidence: bundle.evidence.len(),
        changed,
        reasons,
    })
}

/// Verify the newest bundle, or every bundle when `all` is true.
pub fn verify_newest_or_all(root: &Path, all: bool) -> Result<Vec<ProofVerification>, ProofError> {
    let bundles = all_bundles(root)?;
    if bundles.is_empty() {
        return Err(ProofError::NotFound("the newest bundle".into()));
    }
    let chosen: Vec<&(PathBuf, ProofBundle)> = if all {
        bundles.iter().collect()
    } else {
        bundles.iter().take(1).collect()
    };
    chosen
        .into_iter()
        .map(|(_, bundle)| verify(root, &bundle.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        command.arg("-C").arg(root).args(args);
        crate::process::hide(&mut command);
        assert!(
            command.status().unwrap().success(),
            "git {args:?} in {}",
            root.display()
        );
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn one() {}\n").unwrap();
        fs::write(dir.path().join("README.md"), "demo\n").unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "proof@example.com"]);
        git(dir.path(), &["config", "user.name", "Proof"]);
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "base"]);
        dir
    }

    fn passing() -> Evidence {
        Evidence {
            command: "cargo test".into(),
            exit_code: Some(0),
            ok: true,
            passed: 3,
            failed: 0,
            timed_out: false,
            duration_ms: 12,
            output: "3 passed; 0 failed\n".into(),
            output_sha256: hex(&Sha256::digest(b"3 passed; 0 failed\n")),
            output_truncated: false,
            recorded_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn a_recorded_proof_verifies_and_goes_stale_when_the_tree_moves() {
        let repo = repo();
        fs::write(repo.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        let bundle = record(repo.path(), "HEAD", vec![passing()], Vec::new()).unwrap();
        assert!(bundle.ok, "{bundle:?}");
        assert_eq!(bundle.files.len(), 1);

        let verified = verify(repo.path(), &bundle.id).unwrap();
        assert!(verified.verified(), "{verified:?}");
        assert_eq!(verified.freshness, "fresh");

        fs::write(repo.path().join("src/lib.rs"), "pub fn three() {}\n").unwrap();
        let stale = verify(repo.path(), &bundle.id).unwrap();
        assert!(stale.valid);
        assert!(!stale.fresh);
        assert_eq!(stale.freshness, "stale");
        assert_eq!(stale.changed, vec!["src/lib.rs".to_string()]);
        assert!(!stale.verified());
    }

    #[test]
    fn recording_the_same_tree_appends_evidence_instead_of_replacing_it() {
        let repo = repo();
        fs::write(repo.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        let first = record(repo.path(), "HEAD", vec![passing()], Vec::new()).unwrap();
        let second = record(repo.path(), "HEAD", vec![passing()], Vec::new()).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.evidence.len(), 2);
    }

    #[test]
    fn tampering_with_evidence_output_is_detected() {
        let repo = repo();
        fs::write(repo.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        let bundle = record(repo.path(), "HEAD", vec![passing()], Vec::new()).unwrap();
        let path = proofs_dir(repo.path()).join(format!("{}.json", bundle.id));
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("3 passed", "4 passed")).unwrap();

        let verified = verify(repo.path(), &bundle.id).unwrap();
        assert!(!verified.valid, "{verified:?}");
        assert!(
            verified
                .reasons
                .iter()
                .any(|reason| reason.contains("evidence output was changed")),
            "{verified:?}"
        );
        assert!(!verified.verified());
    }

    #[test]
    fn a_docs_only_change_needs_no_evidence_but_a_source_change_does() {
        let docs = repo();
        fs::write(docs.path().join("README.md"), "changed\n").unwrap();
        let bundle = record(docs.path(), "HEAD", Vec::new(), Vec::new()).unwrap();
        assert!(bundle.ok);
        assert!(verify(docs.path(), &bundle.id).unwrap().verified());

        let source = repo();
        fs::write(source.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        let bundle = record(source.path(), "HEAD", Vec::new(), Vec::new()).unwrap();
        assert!(!bundle.ok, "{bundle:?}");
        assert!(!verify(source.path(), &bundle.id).unwrap().verified());
    }

    #[test]
    fn a_failed_command_is_recorded_as_failed_evidence() {
        let repo = repo();
        fs::write(repo.path().join("src/lib.rs"), "pub fn two() {}\n").unwrap();
        let mut failed = passing();
        failed.exit_code = Some(1);
        failed.ok = false;
        failed.failed = 1;
        let bundle = record(repo.path(), "HEAD", vec![failed], Vec::new()).unwrap();
        assert!(!bundle.ok);
        assert!(
            bundle
                .notes
                .iter()
                .any(|note| note.contains("command failed"))
        );
        let verified = verify(repo.path(), &bundle.id).unwrap();
        assert!(verified.valid);
        assert!(!verified.verified());
    }

    #[test]
    fn bundles_are_listed_newest_first_and_resolve_by_prefix() {
        let repo = repo();
        fs::write(repo.path().join("README.md"), "one\n").unwrap();
        let first = record(repo.path(), "HEAD", Vec::new(), Vec::new()).unwrap();
        fs::write(repo.path().join("README.md"), "two\n").unwrap();
        let second = record(repo.path(), "HEAD", Vec::new(), Vec::new()).unwrap();
        assert_ne!(first.id, second.id);

        let listed = list(repo.path()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id);
        let found = load(repo.path(), &first.id[..4]).unwrap();
        assert_eq!(found.id, first.id);
    }

    #[test]
    fn committing_the_recorded_change_keeps_the_proof_fresh() {
        let repo = repo();
        fs::write(repo.path().join("src/new.rs"), "pub fn new() {}\n").unwrap();
        let bundle = record(repo.path(), "HEAD", vec![passing()], Vec::new()).unwrap();
        assert!(verify(repo.path(), &bundle.id).unwrap().fresh);

        git(repo.path(), &["add", "."]);
        git(repo.path(), &["commit", "-qm", "recorded change"]);
        let after = verify(repo.path(), &bundle.id).unwrap();
        assert!(after.verified(), "{after:?}");
        assert_eq!(after.freshness, "fresh");
    }
}
