//! The tree hash a checkpoint records (I8).
//!
//! A resume is clean only when the files the session can touch are byte-identical to the
//! checkpoint. Git's index is not usable for that: it ignores unstaged work, and a session's
//! changes are exactly the unstaged ones. So Rigel hashes the content of every tracked file
//! itself, plus the set of paths. `.rigel/` is excluded — it holds the session log, not the
//! project.
//!
//! Git is only used to *enumerate* files (`git ls-files`), which is cheap and respects
//! `.gitignore`. Without git, the walker skips the usual build directories by name.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("tree hash cache lock is poisoned")]
    CachePoisoned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    size: u64,
}

#[derive(Default)]
struct TreeHashCache {
    files: BTreeMap<String, (FileStamp, String)>,
    directories: HashMap<String, (Vec<(String, String)>, String)>,
}

static HASH_CACHES: OnceLock<Mutex<HashMap<PathBuf, TreeHashCache>>> = OnceLock::new();

/// Stable across runs and machines: sorted `path\0content-sha` lines, hashed once.
pub fn tree_hash(root: &Path) -> Result<String, TreeError> {
    let root_key = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let caches = HASH_CACHES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut caches = caches.lock().map_err(|_| TreeError::CachePoisoned)?;
    let cache = caches.entry(root_key).or_default();
    let files = tracked_files(root)?;
    let mut current = BTreeMap::new();
    let mut directories: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for relative in files {
        let path = root.join(&relative);
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(TreeError::Io { path, source }),
        };
        let stamp = FileStamp {
            modified: metadata.modified().ok(),
            size: metadata.len(),
        };
        let digest = match cache.files.get(&relative) {
            Some((previous, digest)) if previous == &stamp => digest.clone(),
            _ => {
                let bytes = fs::read(&path).map_err(|source| TreeError::Io {
                    path: path.clone(),
                    source,
                })?;
                sha256_hex(&bytes)
            }
        };
        let normalized = relative.replace('\\', "/");
        let parent = normalized
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("")
            .to_string();
        let name = normalized
            .rsplit('/')
            .next()
            .unwrap_or(&normalized)
            .to_string();
        directories
            .entry(parent.clone())
            .or_default()
            .push((format!("f:{name}"), digest.clone()));
        let mut ancestor = parent.as_str();
        while !ancestor.is_empty() {
            let parent = ancestor
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("");
            let name = ancestor.rsplit('/').next().unwrap_or(ancestor);
            directories
                .entry(parent.to_string())
                .or_default()
                .push((format!("d:{name}"), String::new()));
            ancestor = parent;
        }
        current.insert(relative, (stamp, digest));
    }
    cache.files = current;
    let mut all_paths: Vec<_> = cache.files.iter().collect();
    all_paths.sort_by(|left, right| left.0.cmp(right.0));
    let mut hasher = Sha256::new();
    for (relative, (_, digest)) in all_paths {
        hasher.update(relative.replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    let mut next_directories = HashMap::new();
    let mut directory_hashes = HashMap::new();
    let mut directory_names: Vec<_> = directories.keys().cloned().collect();
    directory_names.sort_by_key(|directory| std::cmp::Reverse(directory.matches('/').count()));
    for directory in directory_names {
        let mut children = directories.remove(&directory).unwrap_or_default();
        for (name, hash) in &mut children {
            if hash.is_empty() {
                let child_name = name.strip_prefix("d:").unwrap_or(name);
                let child_path = if directory.is_empty() {
                    child_name.to_string()
                } else {
                    format!("{directory}/{child_name}")
                };
                *hash = directory_hashes
                    .get(&child_path)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        children.sort();
        let digest = match cache.directories.get(&directory) {
            Some((previous, digest)) if previous == &children => digest.clone(),
            _ => {
                let mut directory_hasher = Sha256::new();
                for (name, child_hash) in &children {
                    directory_hasher.update(name.as_bytes());
                    directory_hasher.update([0]);
                    directory_hasher.update(child_hash.as_bytes());
                    directory_hasher.update(b"\n");
                }
                hex(&directory_hasher.finalize())
            }
        };
        directory_hashes.insert(directory.clone(), digest.clone());
        next_directories.insert(directory, (children, digest));
    }
    cache.directories = next_directories;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Every file the project owns: tracked and untracked, minus ignore rules and `.rigel/`. The
/// code graph reads through this so it sees the same tree a checkpoint does.
pub fn project_files(root: &Path) -> Result<Vec<String>, TreeError> {
    tracked_files(root)
}

fn tracked_files(root: &Path) -> Result<Vec<String>, TreeError> {
    if let Some(mut files) = git_files(root)? {
        files.sort();
        files.dedup();
        return Ok(files);
    }
    let mut files = Vec::new();
    walk(root, root, &mut files, 0)?;
    files.sort();
    Ok(files)
}

fn git_files(root: &Path) -> Result<Option<Vec<String>>, TreeError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args([
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]);
    crate::process::hide(&mut command);
    let output = command.output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        // No git, not a repository, or a broken install: fall back to the walker.
        _ => return Ok(None),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(Some(
        text.split('\0')
            .filter(|path| !path.is_empty())
            .filter(|path| !path.starts_with(".rigel/"))
            .map(str::to_string)
            .collect(),
    ))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) -> Result<(), TreeError> {
    if depth > 24 {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(TreeError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| TreeError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if matches!(
            name.as_str(),
            ".git" | ".rigel" | "node_modules" | "target" | "dist" | ".next"
        ) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| TreeError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            walk(root, &path, out, depth + 1)?;
        } else if file_type.is_file()
            && let Ok(relative) = path.strip_prefix(root)
        {
            // Graph ids, import resolution and the tests all speak `/`; the walker must too,
            // even when git was unavailable and Windows handed us backslashes.
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_changes_the_hash_and_a_revert_restores_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "one\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "two\n").unwrap();

        let first = tree_hash(tmp.path()).unwrap();
        fs::write(tmp.path().join("a.txt"), "ONE\n").unwrap();
        let second = tree_hash(tmp.path()).unwrap();
        assert_ne!(first, second);

        fs::write(tmp.path().join("a.txt"), "one\n").unwrap();
        assert_eq!(tree_hash(tmp.path()).unwrap(), first);
    }

    #[test]
    fn the_walker_speaks_forward_slashes() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut out = Vec::new();
        walk(tmp.path(), tmp.path(), &mut out, 0).unwrap();
        assert!(out.contains(&"src/main.rs".to_string()), "{out:?}");
    }

    #[test]
    fn the_session_directory_is_not_part_of_the_tree() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), "one\n").unwrap();
        let before = tree_hash(tmp.path()).unwrap();
        fs::create_dir_all(tmp.path().join(".rigel")).unwrap();
        fs::write(tmp.path().join(".rigel/rigel.db"), b"not a project file").unwrap();
        assert_eq!(tree_hash(tmp.path()).unwrap(), before);
    }

    #[test]
    fn changing_a_file_invalidates_only_its_directory_and_ancestors() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "one\n").unwrap();
        fs::write(tmp.path().join("docs/readme.md"), "notes\n").unwrap();

        let first = tree_hash(tmp.path()).unwrap();
        let root_key = fs::canonicalize(tmp.path()).unwrap();
        let caches = HASH_CACHES.get().unwrap().lock().unwrap();
        let cache = caches.get(&root_key).unwrap();
        let docs_before = cache.directories.get("docs").unwrap().1.clone();
        let src_before = cache.directories.get("src").unwrap().1.clone();
        drop(caches);

        fs::write(tmp.path().join("src/main.rs"), "two\n").unwrap();
        let second = tree_hash(tmp.path()).unwrap();
        assert_ne!(first, second);
        let caches = HASH_CACHES.get().unwrap().lock().unwrap();
        let cache = caches.get(&root_key).unwrap();
        assert_ne!(cache.directories.get("src").unwrap().1, src_before);
        assert_eq!(cache.directories.get("docs").unwrap().1, docs_before);
    }
}
