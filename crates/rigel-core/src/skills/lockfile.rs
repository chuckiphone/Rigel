//! Reproducible skill selection and offline restoration from `rigel.lock`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::blob::{BlobStore, sha256_hex};
use super::frontmatter;
use super::index::SkillIndex;
use super::verify::SkillTrust;
use super::{SkillsError, skill_dir_name};

pub const LOCK_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLockfile {
    pub format_version: u32,
    pub skills: Vec<LockedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSkill {
    pub id: String,
    pub name: String,
    pub version: String,
    pub hash: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreReport {
    pub restored: usize,
    pub paths: Vec<PathBuf>,
}

impl SkillLockfile {
    /// Build a stable lockfile from indexed entries and their content-addressed bodies.
    pub async fn generate(index: &SkillIndex, ids: &[String]) -> Result<Self, SkillsError> {
        let mut skills = Vec::with_capacity(ids.len());
        let mut unique_ids = HashSet::new();
        for id in ids {
            if !unique_ids.insert(id) {
                return Err(lock_error(format!("duplicate skill id {id}")));
            }
            let detail = index
                .get(id)
                .await?
                .ok_or_else(|| SkillsError::NotIndexed(id.clone()))?;
            let (body, hash) = index
                .body(id)
                .await?
                .ok_or_else(|| SkillsError::NoBody(id.clone()))?;
            let actual = sha256_hex(body.as_bytes());
            if actual != hash {
                return Err(lock_error(format!("indexed body hash mismatch for {id}")));
            }
            let source = index
                .source(id)
                .await?
                .ok_or_else(|| SkillsError::NotIndexed(id.clone()))?;
            let version = frontmatter::parse(&body)
                .get("version")
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .unwrap_or("unversioned")
                .to_string();
            skills.push(LockedSkill {
                id: id.clone(),
                name: detail.skill.name,
                version,
                hash,
                source,
            });
        }
        skills.sort_by(|left, right| left.id.cmp(&right.id));
        validate_entries(&skills)?;
        Ok(Self {
            format_version: LOCK_FORMAT_VERSION,
            skills,
        })
    }

    pub fn read(path: &Path) -> Result<Self, SkillsError> {
        let raw = fs::read_to_string(path)?;
        let lock: Self = serde_json::from_str(&raw)
            .map_err(|error| lock_error(format!("invalid {}: {error}", path.display())))?;
        if lock.format_version != LOCK_FORMAT_VERSION {
            return Err(lock_error(format!(
                "unsupported lockfile format version {}",
                lock.format_version
            )));
        }
        validate_entries(&lock.skills)?;
        Ok(lock)
    }

    /// Restore using only the lockfile and blob directory; no index database or network is read.
    /// All bodies are checked before any destination is changed.
    pub fn restore(
        &self,
        blob_dir: &Path,
        project_root: &Path,
        roots: &[PathBuf],
    ) -> Result<RestoreReport, SkillsError> {
        if self.format_version != LOCK_FORMAT_VERSION {
            return Err(lock_error(format!(
                "unsupported lockfile format version {}",
                self.format_version
            )));
        }
        validate_entries(&self.skills)?;
        let blobs = BlobStore::new(blob_dir);
        let trust = SkillTrust::load(project_root)
            .map_err(|error| lock_error(format!("skill trust verification failed: {error}")))?;
        let mut ready = Vec::with_capacity(self.skills.len());

        for skill in &self.skills {
            let bytes = blobs
                .get(&skill.hash)?
                .ok_or_else(|| lock_error(format!("blob {} is missing", skill.hash)))?;
            let actual = sha256_hex(&bytes);
            if actual != skill.hash {
                return Err(lock_error(format!(
                    "hash mismatch for {}: expected {}, got {actual}",
                    skill.id, skill.hash
                )));
            }
            let body = String::from_utf8(bytes)
                .map_err(|error| lock_error(format!("{} is not UTF-8: {error}", skill.id)))?;
            let metadata = frontmatter::parse(&body);
            let version = metadata
                .get("version")
                .map(str::trim)
                .filter(|version| !version.is_empty())
                .unwrap_or("unversioned")
                .to_string();
            if version != skill.version {
                return Err(lock_error(format!(
                    "version mismatch for {}: expected {}, got {version}",
                    skill.id, skill.version
                )));
            }
            trust
                .verify(&skill.id, &skill.source, &body)
                .map_err(|error| lock_error(format!("skill {} refused: {error}", skill.id)))?;
            // Keep the established static capability inventory in the restore path. It is
            // advisory; the trust and full-file hash checks are the refusal gates.
            let _capabilities = super::scan::scan(&body, "SKILL.md");
            ready.push((skill_dir_name(&skill.name), body));
        }

        let mut paths = Vec::new();
        for root in roots {
            for (directory, body) in &ready {
                let folder = root.join(directory);
                fs::create_dir_all(&folder)?;
                let path = folder.join("SKILL.md");
                fs::write(&path, body)?;
                paths.push(path);
            }
        }
        Ok(RestoreReport {
            restored: self.skills.len(),
            paths,
        })
    }
}

fn validate_entries(skills: &[LockedSkill]) -> Result<(), SkillsError> {
    let mut ids = HashSet::new();
    let mut directories = HashSet::new();
    for skill in skills {
        if skill.id.trim().is_empty()
            || skill.name.trim().is_empty()
            || skill.source.trim().is_empty()
        {
            return Err(lock_error("skill id, name, and source must be non-empty"));
        }
        if skill.hash.len() != 64 || !skill.hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(lock_error(format!("invalid SHA-256 hash for {}", skill.id)));
        }
        if !ids.insert(skill.id.as_str()) {
            return Err(lock_error(format!("duplicate skill id {}", skill.id)));
        }
        let directory = skill_dir_name(&skill.name);
        if !directories.insert(directory.clone()) {
            return Err(lock_error(format!(
                "skill names collide at directory {directory}"
            )));
        }
    }
    Ok(())
}

fn lock_error(detail: impl Into<String>) -> SkillsError {
    SkillsError::Lockfile(detail.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillRecord;
    use tempfile::TempDir;

    fn record(id: &str, body: &str) -> SkillRecord {
        SkillRecord {
            id: id.to_string(),
            name: "Locked Setup".to_string(),
            description: "A setup skill".to_string(),
            source: "builtin".to_string(),
            repo: None,
            path: None,
            url: None,
            body_sha: Some(sha256_hex(body.as_bytes())),
            grade: None,
            grade_scope: None,
            license: None,
            stars: None,
            installs: None,
            tokens: None,
            capabilities: Vec::new(),
            simhash: None,
        }
    }

    #[tokio::test]
    async fn generated_lock_restores_without_opening_the_index_database() {
        let data = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let index = SkillIndex::open(data.path()).await.unwrap();
        let body = "---\nname: Locked Setup\nversion: 2.1\n---\n# Setup\n";
        index.blobs().put(body.as_bytes()).unwrap();
        index
            .ingest_records(&[record("builtin:locked-setup", body)])
            .await
            .unwrap();
        let hash = sha256_hex(body.as_bytes());
        let offline = TempDir::new().unwrap();
        BlobStore::new(offline.path())
            .put(&index.blobs().get(&hash).unwrap().unwrap())
            .unwrap();

        let lock = SkillLockfile::generate(&index, &["builtin:locked-setup".to_string()])
            .await
            .unwrap();
        assert_eq!(lock.skills[0].version, "2.1");
        assert_eq!(lock.skills[0].source, "builtin");
        let path = project.path().join("rigel.lock");
        fs::write(&path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
        index.store().pool().close().await;
        drop(index);

        let loaded = SkillLockfile::read(&path).unwrap();
        let destination = project.path().join(".agents/skills");
        let report = loaded
            .restore(
                offline.path(),
                project.path(),
                std::slice::from_ref(&destination),
            )
            .unwrap();
        assert_eq!(report.restored, 1);
        assert_eq!(
            fs::read_to_string(destination.join("locked-setup/SKILL.md")).unwrap(),
            body
        );
        assert_eq!(fs::read_dir(offline.path()).unwrap().count(), 1);
    }

    #[test]
    fn restore_rejects_hash_mismatch_before_writing_any_skill() {
        let data = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let root = project.path().join("skills");
        let body = b"---\nname: Locked Setup\n---\n# Setup\n";
        let hash = sha256_hex(body);
        let blob_path = BlobStore::new(data.path()).path_for(&hash);
        fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
        fs::write(blob_path, b"modified after lock generation").unwrap();
        let lock = SkillLockfile {
            format_version: LOCK_FORMAT_VERSION,
            skills: vec![LockedSkill {
                id: "builtin:locked-setup".into(),
                name: "Locked Setup".into(),
                version: "unversioned".into(),
                hash: hash.clone(),
                source: "builtin".into(),
            }],
        };
        let error = lock
            .restore(data.path(), project.path(), std::slice::from_ref(&root))
            .unwrap_err();
        assert!(error.to_string().contains("missing") || error.to_string().contains("mismatch"));
        assert!(!root.exists());
    }
}
