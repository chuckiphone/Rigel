//! Detect Jujutsu workspaces, including colocated workspaces that also expose a Git repository.

use std::path::{Path, PathBuf};

/// A Jujutsu working copy root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JjWorkspace {
    root: PathBuf,
}

impl JjWorkspace {
    /// Find the nearest parent directory containing Jujutsu's workspace metadata.
    ///
    /// Looking for `.jj` directly works for both native and colocated Git backends and does not
    /// require spawning `jj`, which may not be installed on machines that only restore snapshots.
    pub fn detect(path: &Path) -> Option<Self> {
        let path = path.canonicalize().ok()?;
        path.ancestors()
            .find(|ancestor| ancestor.join(".jj").exists())
            .map(|root| Self {
                root: root.to_path_buf(),
            })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
