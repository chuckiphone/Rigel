//! Content-addressed blob store (roadmap 184).
//!
//! - `put` dedupes by SHA-256: identical bytes are stored once and the existing
//!   hash is returned.
//! - `get` re-hashes the bytes on disk and refuses a corrupted blob.
//! - `collect_orphans` removes blobs whose hash is not in the referenced set.
//! - `bytes_per_project` reports stored bytes grouped by project.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::patches::sha256_hex;

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blob {0} is corrupted: hash mismatch")]
    Corrupted(String),
    #[error("blob {0} not found")]
    NotFound(String),
    #[error("invalid hash: {0}")]
    InvalidHash(String),
}

#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.root.join(hash)
    }

    fn ensure_root(&self) -> Result<(), BlobError> {
        fs::create_dir_all(&self.root)?;
        Ok(())
    }

    /// Store `data`, deduping by SHA-256. Returns the hash.
    pub fn put(&self, data: &[u8]) -> Result<String, BlobError> {
        self.ensure_root()?;
        let hash = sha256_hex(data);
        let path = self.blob_path(&hash);
        if !path.exists() {
            fs::write(&path, data)?;
        }
        Ok(hash)
    }

    /// Store `data` under a project subdirectory, deduping by hash.
    pub fn put_for_project(&self, project: &str, data: &[u8]) -> Result<String, BlobError> {
        let root = self.root.join(project);
        fs::create_dir_all(&root)?;
        let hash = sha256_hex(data);
        let path = root.join(&hash);
        if !path.exists() {
            fs::write(&path, data)?;
        }
        Ok(hash)
    }

    /// Read and verify a blob by hash.
    pub fn get(&self, hash: &str) -> Result<Vec<u8>, BlobError> {
        self.verify_hash(hash)?;
        let path = self.blob_path(hash);
        if !path.exists() {
            return Err(BlobError::NotFound(hash.to_string()));
        }
        let bytes = fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != hash {
            return Err(BlobError::Corrupted(hash.to_string()));
        }
        Ok(bytes)
    }

    /// Read and verify a blob stored under a project subdirectory.
    pub fn get_for_project(&self, project: &str, hash: &str) -> Result<Vec<u8>, BlobError> {
        self.verify_hash(hash)?;
        let path = self.root.join(project).join(hash);
        if !path.exists() {
            return Err(BlobError::NotFound(hash.to_string()));
        }
        let bytes = fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != hash {
            return Err(BlobError::Corrupted(hash.to_string()));
        }
        Ok(bytes)
    }

    /// `true` when the blob file exists on disk.
    pub fn contains(&self, hash: &str) -> bool {
        self.blob_path(hash).exists()
    }

    /// Delete a blob by hash.
    pub fn delete(&self, hash: &str) -> Result<(), BlobError> {
        let path = self.blob_path(hash);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// List every blob hash stored at the root.
    pub fn list(&self) -> Result<Vec<String>, BlobError> {
        self.ensure_root()?;
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                out.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        out.sort();
        Ok(out)
    }

    /// Total bytes stored at the root.
    pub fn total_bytes(&self) -> u64 {
        self.bytes_for_dir(&self.root)
    }

    /// Bytes stored, grouped by project subdirectory. A flat root counts as "default".
    pub fn bytes_per_project(&self) -> BTreeMap<String, u64> {
        let mut map = BTreeMap::new();
        if !self.root.exists() {
            return map;
        }
        // Top-level files belong to the "default" project.
        let mut default_bytes = 0u64;
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    default_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        if default_bytes > 0 {
            map.insert("default".to_string(), default_bytes);
        }
        // Subdirectories are projects.
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let bytes = self.bytes_for_dir(&entry.path());
                    if bytes > 0 {
                        map.insert(name, bytes);
                    }
                }
            }
        }
        map
    }

    fn bytes_for_dir(&self, dir: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        total += meta.len();
                    } else if meta.is_dir() {
                        total += self.bytes_for_dir(&entry.path());
                    }
                }
            }
        }
        total
    }

    /// Remove blobs not in `referenced`. Returns the hashes removed.
    pub fn collect_orphans(&self, referenced: &HashSet<String>) -> Result<Vec<String>, BlobError> {
        self.ensure_root()?;
        let mut removed = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let hash = entry.file_name().to_string_lossy().to_string();
            if referenced.contains(&hash) {
                continue;
            }
            fs::remove_file(entry.path())?;
            removed.push(hash);
        }
        removed.sort();
        Ok(removed)
    }

    /// Collect orphans under a project subdirectory.
    pub fn collect_orphans_for_project(
        &self,
        project: &str,
        referenced: &HashSet<String>,
    ) -> Result<Vec<String>, BlobError> {
        let dir = self.root.join(project);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut removed = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let hash = entry.file_name().to_string_lossy().to_string();
            if referenced.contains(&hash) {
                continue;
            }
            fs::remove_file(entry.path())?;
            removed.push(hash);
        }
        removed.sort();
        Ok(removed)
    }

    fn verify_hash(&self, hash: &str) -> Result<(), BlobError> {
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(BlobError::InvalidHash(hash.to_string()));
        }
        Ok(())
    }

    /// Verify a blob without returning its bytes.
    pub fn verify(&self, hash: &str) -> Result<bool, BlobError> {
        match self.get(hash) {
            Ok(_) => Ok(true),
            Err(BlobError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn put_dedupes_by_hash() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let h1 = store.put(b"hello").unwrap();
        let h2 = store.put(b"hello").unwrap();
        assert_eq!(h1, h2);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn get_verifies_on_read() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let hash = store.put(b"hello").unwrap();
        let bytes = store.get(&hash).unwrap();
        assert_eq!(bytes, b"hello");

        // Corrupt the blob.
        let path = store.root().join(&hash);
        std::fs::write(&path, b"tampered").unwrap();
        assert!(matches!(store.get(&hash), Err(BlobError::Corrupted(_))));
    }

    #[test]
    fn corrupted_blob_is_reported_by_hash() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let hash = store.put(b"hello").unwrap();
        let path = store.root().join(&hash);
        std::fs::write(&path, b"tampered").unwrap();
        match store.get(&hash) {
            Err(BlobError::Corrupted(h)) => assert_eq!(h, hash),
            other => panic!("expected corrupted, got {other:?}"),
        }
    }

    #[test]
    fn a_re_read_refuses_to_serve_a_corrupted_blob() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let hash = store.put(b"hello").unwrap();
        let path = store.root().join(&hash);
        std::fs::write(&path, b"tampered").unwrap();
        assert!(store.get(&hash).is_err());
        assert!(store.verify(&hash).is_err());
    }

    #[test]
    fn collect_orphans_removes_unreferenced_blobs() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let h1 = store.put(b"one").unwrap();
        let h2 = store.put(b"two").unwrap();
        let referenced = HashSet::from([h1.clone()]);
        let removed = store.collect_orphans(&referenced).unwrap();
        assert_eq!(removed, vec![h2.clone()]);
        assert!(store.contains(&h1));
        assert!(!store.contains(&h2));
    }

    #[test]
    fn bytes_per_project_reports_groups() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        store.put(b"hello").unwrap();
        store.put_for_project("proj-a", b"world").unwrap();
        let map = store.bytes_per_project();
        assert!(map.contains_key("default"));
        assert!(map.contains_key("proj-a"));
    }

    #[test]
    fn invalid_hash_is_rejected() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        assert!(matches!(
            store.get("not-a-hash"),
            Err(BlobError::InvalidHash(_))
        ));
    }
}
