//! Attachment store with per-project quota, type validation and hash dedup (roadmap 101).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::patches::sha256_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub hash: String,
    pub size: u64,
    pub mime_type: String,
    pub project: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported content type: {0}")]
    UnsupportedType(String),
    #[error("project quota exceeded: {used} + {size} > {limit} bytes")]
    QuotaExceeded { used: u64, size: u64, limit: u64 },
    #[error("count quota exceeded: {used} + 1 > {limit}")]
    CountExceeded { used: usize, limit: usize },
    #[error("attachment {0} not found")]
    NotFound(String),
    #[error("attachment {0} is corrupted")]
    Corrupted(String),
}

#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
    quota_bytes: u64,
    quota_count: usize,
    allowed_types: Vec<String>,
}

impl AttachmentStore {
    pub fn new(
        root: impl AsRef<Path>,
        quota_bytes: u64,
        quota_count: usize,
        allowed_types: Vec<String>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            quota_bytes,
            quota_count,
            allowed_types,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    pub fn quota_count(&self) -> usize {
        self.quota_count
    }

    fn project_dir(&self, project: &str) -> PathBuf {
        self.root.join(project)
    }

    fn blob_path(&self, project: &str, hash: &str) -> PathBuf {
        self.project_dir(project).join(hash)
    }

    fn meta_path(&self, project: &str, hash: &str) -> PathBuf {
        self.project_dir(project).join(format!("{hash}.meta"))
    }

    fn ensure_project(&self, project: &str) -> Result<(), AttachmentError> {
        fs::create_dir_all(self.project_dir(project))?;
        Ok(())
    }

    /// Validate the content type against the allowlist.
    pub fn validate_type(&self, mime_type: &str) -> Result<(), AttachmentError> {
        if self.allowed_types.is_empty() || self.allowed_types.iter().any(|t| t == mime_type) {
            Ok(())
        } else {
            Err(AttachmentError::UnsupportedType(mime_type.to_string()))
        }
    }

    /// Current usage (bytes, count) for a project.
    pub fn usage(&self, project: &str) -> (u64, usize) {
        let dir = self.project_dir(project);
        if !dir.exists() {
            return (0, 0);
        }
        let mut bytes = 0u64;
        let mut count = 0usize;
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && !entry.file_name().to_string_lossy().ends_with(".meta")
                {
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    count += 1;
                }
            }
        }
        (bytes, count)
    }

    /// Store an attachment, deduping by hash and enforcing quota and type validation.
    pub fn put(
        &self,
        project: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<Attachment, AttachmentError> {
        self.validate_type(mime_type)?;
        self.ensure_project(project)?;
        let hash = sha256_hex(data);

        // Dedup: if the blob already exists, return it without re-charging quota.
        let blob_path = self.blob_path(project, &hash);
        if blob_path.exists() {
            return Ok(Attachment {
                hash,
                size: data.len() as u64,
                mime_type: mime_type.to_string(),
                project: project.to_string(),
            });
        }

        let (used_bytes, used_count) = self.usage(project);
        if used_bytes + data.len() as u64 > self.quota_bytes {
            return Err(AttachmentError::QuotaExceeded {
                used: used_bytes,
                size: data.len() as u64,
                limit: self.quota_bytes,
            });
        }
        if used_count + 1 > self.quota_count {
            return Err(AttachmentError::CountExceeded {
                used: used_count,
                limit: self.quota_count,
            });
        }

        fs::write(&blob_path, data)?;
        // write a small meta file recording the mime type
        let meta = format!("{mime_type}");
        let _ = fs::write(self.meta_path(project, &hash), meta);

        Ok(Attachment {
            hash,
            size: data.len() as u64,
            mime_type: mime_type.to_string(),
            project: project.to_string(),
        })
    }

    /// Read an attachment, verifying its hash.
    pub fn get(&self, project: &str, hash: &str) -> Result<Vec<u8>, AttachmentError> {
        let path = self.blob_path(project, hash);
        if !path.exists() {
            return Err(AttachmentError::NotFound(hash.to_string()));
        }
        let bytes = fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != hash {
            return Err(AttachmentError::Corrupted(hash.to_string()));
        }
        Ok(bytes)
    }

    /// Whether an attachment exists.
    pub fn contains(&self, project: &str, hash: &str) -> bool {
        self.blob_path(project, hash).exists()
    }

    /// Purge attachments not in `referenced`. Never removes a referenced blob.
    pub fn purge(
        &self,
        project: &str,
        referenced: &HashSet<String>,
    ) -> Result<Vec<String>, AttachmentError> {
        let dir = self.project_dir(project);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut removed = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".meta") {
                continue;
            }
            if referenced.contains(&name) {
                continue;
            }
            fs::remove_file(entry.path())?;
            // remove meta too
            let meta = dir.join(format!("{name}.meta"));
            let _ = fs::remove_file(meta);
            removed.push(name);
        }
        removed.sort();
        Ok(removed)
    }

    /// List attachment hashes for a project.
    pub fn list(&self, project: &str) -> Result<Vec<String>, AttachmentError> {
        let dir = self.project_dir(project);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".meta") {
                    out.push(name);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn store() -> (tempfile::TempDir, AttachmentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(
            dir.path(),
            1024,
            10,
            vec!["text/plain".to_string(), "image/png".to_string()],
        );
        (dir, store)
    }

    #[test]
    fn put_dedupes_by_hash() {
        let (_dir, store) = store();
        let a = store.put("p", b"hello", "text/plain").unwrap();
        let b = store.put("p", b"hello", "text/plain").unwrap();
        assert_eq!(a.hash, b.hash);
        assert_eq!(store.list("p").unwrap().len(), 1);
    }

    #[test]
    fn validates_content_type() {
        let (_dir, store) = store();
        assert!(store.put("p", b"hello", "application/x-evil").is_err());
        assert!(store.put("p", b"hello", "text/plain").is_ok());
    }

    #[test]
    fn quota_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path(), 5, 10, vec![]);
        store.put("p", b"hi", "text/plain").unwrap();
        // 2 + 4 = 6 > 5
        assert!(matches!(
            store.put("p", b"four!", "text/plain"),
            Err(AttachmentError::QuotaExceeded { .. })
        ));
    }

    #[test]
    fn count_quota_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path(), 1024, 1, vec![]);
        store.put("p", b"a", "text/plain").unwrap();
        assert!(matches!(
            store.put("p", b"b", "text/plain"),
            Err(AttachmentError::CountExceeded { .. })
        ));
    }

    #[test]
    fn get_verifies_hash() {
        let (_dir, store) = store();
        let att = store.put("p", b"hello", "text/plain").unwrap();
        let bytes = store.get("p", &att.hash).unwrap();
        assert_eq!(bytes, b"hello");
        // corrupt
        let path = store.root().join("p").join(&att.hash);
        std::fs::write(&path, b"tampered").unwrap();
        assert!(matches!(
            store.get("p", &att.hash),
            Err(AttachmentError::Corrupted(_))
        ));
    }

    #[test]
    fn purge_never_removes_a_referenced_blob() {
        let (_dir, store) = store();
        let a = store.put("p", b"one", "text/plain").unwrap();
        let b = store.put("p", b"two", "text/plain").unwrap();
        let referenced = HashSet::from([a.hash.clone()]);
        let removed = store.purge("p", &referenced).unwrap();
        assert_eq!(removed, vec![b.hash.clone()]);
        assert!(store.contains("p", &a.hash));
        assert!(!store.contains("p", &b.hash));
    }

    #[test]
    fn usage_reports_bytes_and_count() {
        let (_dir, store) = store();
        store.put("p", b"hello", "text/plain").unwrap();
        store.put("p", b"world", "text/plain").unwrap();
        let (bytes, count) = store.usage("p");
        assert_eq!(bytes, 10);
        assert_eq!(count, 2);
    }
}
