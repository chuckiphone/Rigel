//! Attachment store with per-project quota, type validation and hash dedup (roadmap 101).

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::patches::sha256_hex;

/// A store rooted at one project's blob directory uses this single stable project key.
const PROJECT_KEY: &str = "default";

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
    #[error("session id {0} is not a safe reference key")]
    UnsafeSessionId(String),
    #[error("project {0} is not a single safe path component")]
    UnsafeProject(String),
    #[error("hash {0} is not a 64-character lowercase hex digest")]
    UnsafeHash(String),
}

#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
    quota_bytes: u64,
    quota_count: usize,
    allowed_types: Vec<String>,
}

/// Blob storage for one project, outside the checkpointed tree.
pub fn default_root(project_root: &Path) -> PathBuf {
    project_root.join(".rigel").join("attachment-blobs")
}

fn safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id != "."
        && session_id != ".."
        && !session_id.contains(['/', '\\', ':', '\0'])
}

/// A project key becomes one directory name below the store root.
fn safe_project(project: &str) -> bool {
    safe_session_id(project)
}

/// Content addresses are exactly what `sha256_hex` produces: lowercase hex, 64 characters.
fn safe_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check_inputs(project: &str, hash: Option<&str>) -> Result<(), AttachmentError> {
    if !safe_project(project) {
        return Err(AttachmentError::UnsafeProject(project.to_string()));
    }
    if let Some(hash) = hash
        && !safe_hash(hash)
    {
        return Err(AttachmentError::UnsafeHash(hash.to_string()));
    }
    Ok(())
}

/// Store a blob and record that one session references it.
pub fn track_blob(
    project_root: &Path,
    session_id: &str,
    data: &[u8],
    mime_type: &str,
) -> Result<Attachment, AttachmentError> {
    if !safe_session_id(session_id) {
        return Err(AttachmentError::UnsafeSessionId(session_id.to_string()));
    }
    let store = AttachmentStore::new(default_root(project_root), u64::MAX, usize::MAX, Vec::new());
    store.put_for_session(PROJECT_KEY, session_id, data, mime_type)
}

/// Drop a deleted session's references, then purge blobs no remaining session references.
pub fn purge_session_blobs(
    project_root: &Path,
    session_id: &str,
) -> Result<Vec<String>, AttachmentError> {
    if !safe_session_id(session_id) {
        return Err(AttachmentError::UnsafeSessionId(session_id.to_string()));
    }
    let store = AttachmentStore::new(default_root(project_root), u64::MAX, usize::MAX, Vec::new());
    store.drop_session(PROJECT_KEY, session_id)
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
        if !safe_project(project) {
            return (0, 0);
        }
        let dir = self.project_dir(project);
        if !dir.exists() {
            return (0, 0);
        }
        let mut bytes = 0u64;
        let mut count = 0usize;
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && !name.ends_with(".meta")
                    && !name.ends_with(".refs")
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
        check_inputs(project, None)?;
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
        let meta = mime_type.to_string();
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
        check_inputs(project, Some(hash))?;
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
        safe_project(project) && safe_hash(hash) && self.blob_path(project, hash).exists()
    }

    fn refs_path(&self, project: &str, hash: &str) -> PathBuf {
        self.project_dir(project).join(format!("{hash}.refs"))
    }

    fn read_refs(&self, project: &str, hash: &str) -> BTreeSet<String> {
        match fs::read_to_string(self.refs_path(project, hash)) {
            Ok(text) => text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect(),
            Err(_) => BTreeSet::new(),
        }
    }

    /// Record that a session references an existing blob. Shared blobs carry every session id.
    pub fn reference(
        &self,
        project: &str,
        hash: &str,
        session_id: &str,
    ) -> Result<(), AttachmentError> {
        check_inputs(project, Some(hash))?;
        if !safe_session_id(session_id) {
            return Err(AttachmentError::UnsafeSessionId(session_id.to_string()));
        }
        if !self.blob_path(project, hash).exists() {
            return Err(AttachmentError::NotFound(hash.to_string()));
        }
        let mut refs = self.read_refs(project, hash);
        refs.insert(session_id.to_string());
        let body = refs.into_iter().collect::<Vec<_>>().join("\n");
        fs::write(self.refs_path(project, hash), format!("{body}\n"))?;
        Ok(())
    }

    /// Store and reference in one step, so no blob can exist unattributed by construction.
    pub fn put_for_session(
        &self,
        project: &str,
        session_id: &str,
        data: &[u8],
        mime_type: &str,
    ) -> Result<Attachment, AttachmentError> {
        let attachment = self.put(project, data, mime_type)?;
        self.reference(project, &attachment.hash, session_id)?;
        Ok(attachment)
    }

    /// Sessions currently referencing one blob.
    pub fn sessions_for(&self, project: &str, hash: &str) -> Vec<String> {
        if !safe_project(project) || !safe_hash(hash) {
            return Vec::new();
        }
        self.read_refs(project, hash).into_iter().collect()
    }

    /// Every blob referenced by one session, for a caller that owns its own purge.
    pub fn referenced_by(&self, project: &str, session_id: &str) -> HashSet<String> {
        if !safe_project(project) || !safe_session_id(session_id) {
            return HashSet::new();
        }
        let dir = self.project_dir(project);
        let mut out = HashSet::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".refs") {
                continue;
            }
            let hash = name.strip_suffix(".refs").unwrap_or(&name);
            if self.read_refs(project, hash).contains(session_id) {
                out.insert(hash.to_string());
            }
        }
        out
    }

    /// Remove a session's references and purge every blob no remaining session references.
    pub fn drop_session(
        &self,
        project: &str,
        session_id: &str,
    ) -> Result<Vec<String>, AttachmentError> {
        check_inputs(project, None)?;
        if !safe_session_id(session_id) {
            return Err(AttachmentError::UnsafeSessionId(session_id.to_string()));
        }
        for hash in self.referenced_by(project, session_id) {
            let path = self.refs_path(project, &hash);
            let mut refs = self.read_refs(project, &hash);
            refs.remove(session_id);
            if refs.is_empty() {
                let _ = fs::remove_file(&path);
            } else {
                let body = refs.into_iter().collect::<Vec<_>>().join("\n");
                fs::write(&path, format!("{body}\n"))?;
            }
        }
        // A blob without any reference is unreachable; keep only blobs a session still names.
        let live: HashSet<String> = self
            .list(project)?
            .into_iter()
            .filter(|hash| !self.sessions_for(project, hash).is_empty())
            .collect();
        self.purge(project, &live)
    }

    /// Purge attachments not in `referenced`. Never removes a referenced blob.
    pub fn purge(
        &self,
        project: &str,
        referenced: &HashSet<String>,
    ) -> Result<Vec<String>, AttachmentError> {
        check_inputs(project, None)?;
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
            if name.ends_with(".meta") || name.ends_with(".refs") {
                continue;
            }
            if referenced.contains(&name) {
                continue;
            }
            fs::remove_file(entry.path())?;
            // Remove sidecars too: a purged blob must not leave stale references behind.
            let meta = dir.join(format!("{name}.meta"));
            let _ = fs::remove_file(meta);
            let _ = fs::remove_file(self.refs_path(project, &name));
            removed.push(name);
        }
        removed.sort();
        Ok(removed)
    }

    /// List attachment hashes for a project.
    pub fn list(&self, project: &str) -> Result<Vec<String>, AttachmentError> {
        check_inputs(project, None)?;
        let dir = self.project_dir(project);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".meta") && !name.ends_with(".refs") {
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
    fn a_session_purge_keeps_shared_blobs_and_removes_unreferenced_ones() {
        let (_dir, store) = store();
        let shared = store
            .put_for_session("p", "s1", b"shared", "text/plain")
            .unwrap();
        store.reference("p", &shared.hash, "s2").unwrap();
        let alone = store
            .put_for_session("p", "s3", b"only-s3", "text/plain")
            .unwrap();
        let unreferenced = store.put("p", b"nobody", "text/plain").unwrap();

        assert_eq!(store.sessions_for("p", &shared.hash), vec!["s1", "s2"]);
        let removed = store.drop_session("p", "s1").unwrap();
        assert!(
            !removed.contains(&shared.hash),
            "s2 still references the shared blob"
        );
        assert!(store.contains("p", &shared.hash));
        assert_eq!(store.sessions_for("p", &shared.hash), vec!["s2"]);
        assert!(store.contains("p", &alone.hash));
        assert!(
            !store.contains("p", &unreferenced.hash),
            "an unreferenced blob is purged"
        );

        let removed = store.drop_session("p", "s2").unwrap();
        assert!(removed.contains(&shared.hash));
        assert!(!store.contains("p", &shared.hash));
        assert!(store.contains("p", &alone.hash));
        assert_eq!(
            fs::read_to_string(store.refs_path("p", &alone.hash)).unwrap(),
            "s3\n"
        );

        assert!(matches!(
            store.reference("p", &shared.hash, "../escape"),
            Err(AttachmentError::UnsafeSessionId(_))
        ));
    }

    #[test]
    fn unsafe_project_and_hash_inputs_are_refused() {
        let (dir, store) = store();
        for project in ["../escape", "p/q", "p\\q", "/absolute", "..", ""] {
            assert!(
                matches!(
                    store.put(project, b"x", "text/plain"),
                    Err(AttachmentError::UnsafeProject(_))
                ),
                "{project} is not one path component"
            );
            assert!(matches!(
                store.list(project),
                Err(AttachmentError::UnsafeProject(_))
            ));
            assert!(matches!(
                store.get(project, &"a".repeat(64)),
                Err(AttachmentError::UnsafeProject(_))
            ));
        }
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "nothing escaped the root"
        );

        let good = store.put("p", b"x", "text/plain").unwrap();
        for hash in [
            "short".to_string(),
            "g".repeat(64),
            good.hash.to_uppercase(),
        ] {
            assert!(
                matches!(store.get("p", &hash), Err(AttachmentError::UnsafeHash(_))),
                "{hash}"
            );
            assert!(!store.contains("p", &hash));
        }
        assert!(store.get("p", &good.hash).is_ok());
        assert!(matches!(
            store.reference("p", "../escape", "s1"),
            Err(AttachmentError::UnsafeHash(_))
        ));
        assert!(matches!(
            store.purge("../escape", &HashSet::new()),
            Err(AttachmentError::UnsafeProject(_))
        ));
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
