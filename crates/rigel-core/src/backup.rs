//! Encrypted local backups (roadmap #28).
//!
//! A backup is one age-encrypted tar stream. The passphrase never enters a file: it is supplied by
//! the caller (the CLI reads a file or environment variable) and age's scrypt recipient derives
//! the key. Every entry is authenticated by age before it reaches tar, and the manifest at the end
//! records a SHA-256 for every file so a restore can reject a truncated or altered logical tree.
//!
//! The index database is copied with SQLite's `VACUUM INTO`, not by copying `index.db` and its WAL
//! beside it: a live WAL database copied file-by-file can be torn. Other files are copied while
//! the app may still be writing them; the manifest describes the bytes that were actually read,
//! and `verify`/`drill` expose that fact instead of pretending the archive is a filesystem image.
//!
//! Generated sidecars and downloaded installers are intentionally not part of a backup:
//! `agentchat`, `updates`, `diagnostics` and `backups` are re-creatable or previously generated.
//! They are named in the create report rather than silently disappearing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::now_ms;

/// The archive is an age file containing this tar layout.
pub const FORMAT: &str = "rigel-backup/1";
/// The suffix the CLI uses when it chooses a default name.
pub const ARCHIVE_EXTENSION: &str = "rigel-backup";
/// A generous ceiling that still stops a mistaken home-directory backup from filling the disk.
pub const MAX_FILES: usize = 50_000;
pub const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MANIFEST: &str = "manifest.json";
const DATA_PREFIX: &str = "data/";
const TEMP_PREFIX: &str = ".rigel-backup-";

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("the data directory does not exist: {0}")]
    MissingSource(PathBuf),
    #[error("the backup destination is inside the data directory: {0}")]
    DestinationInsideSource(PathBuf),
    #[error("the backup destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("refusing to restore into a non-empty directory: {0}")]
    RestoreNotEmpty(PathBuf),
    #[error("refusing to restore over a file: {0}")]
    RestoreOverFile(PathBuf),
    #[error("the backup is larger than the {max_files}-file / {max_bytes}-byte limit")]
    TooLarge { max_files: usize, max_bytes: u64 },
    #[error("the backup archive is damaged: {0}")]
    Corrupt(String),
    #[error("the backup manifest is invalid: {0}")]
    Manifest(String),
    #[error("the backup contains an unsafe path: {0}")]
    UnsafePath(String),
    #[error("the passphrase is empty")]
    EmptyPassphrase,
    #[error("the archive could not be encrypted or decrypted: {0}")]
    Crypto(String),
    #[error("the archive could not be read or written: {0}")]
    Tar(String),
    #[error("the SQLite snapshot failed: {0}")]
    Database(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the manifest could not be encoded: {0}")]
    Json(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFile {
    /// Relative to the data directory, with `/` separators.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub created_at: i64,
    pub source: String,
    pub files: Vec<BackupFile>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateReport {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub created_at: i64,
    pub source: PathBuf,
    pub excluded: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub created_at: i64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreReport {
    pub path: PathBuf,
    pub into: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RotateReport {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrillReport {
    pub ok: bool,
    pub archive: String,
    pub files: usize,
    pub bytes: u64,
    pub database: String,
    pub checked_at: i64,
}

struct SourceFile {
    relative: String,
    absolute: PathBuf,
    bytes: u64,
}

/// Create one encrypted archive at `dest`. The destination must not exist.
pub async fn create(
    data_dir: &Path,
    dest: &Path,
    passphrase: &str,
) -> Result<CreateReport, BackupError> {
    if passphrase.is_empty() {
        return Err(BackupError::EmptyPassphrase);
    }
    if !data_dir.is_dir() {
        return Err(BackupError::MissingSource(data_dir.to_path_buf()));
    }
    let source = canonical_dir(data_dir)?;
    let destination = normalize_path(&absolute_path(dest)?);
    if within(&destination, &source) && !within(&destination, &source.join("backups")) {
        return Err(BackupError::DestinationInsideSource(destination));
    }
    if destination.exists() {
        return Err(BackupError::DestinationExists(destination));
    }
    if let Some(parent) = destination.parent() {
        create_dir(parent)?;
    }

    let temp = sibling_temp(&destination);
    let result = create_at(&source, &temp, passphrase).await;
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let mut report = result?;
    // The source files can change between the manifest hash and the copy. A backup that cannot
    // pass its own verification must not be published as a successful one.
    let verified = match verify(&temp, passphrase) {
        Ok(verified) => verified,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };
    report.files = verified.files;
    report.bytes = verified.bytes;
    if let Err(source) = fs::rename(&temp, &destination) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(&destination, source));
    }
    report.path = destination;
    report.source = source;
    Ok(report)
}

async fn create_at(
    source: &Path,
    temp: &Path,
    passphrase: &str,
) -> Result<CreateReport, BackupError> {
    let mut files = Vec::new();
    let mut excluded = Vec::new();
    collect_files(source, source, &mut files, &mut excluded)?;
    if files.len() > MAX_FILES {
        return Err(BackupError::TooLarge {
            max_files: MAX_FILES,
            max_bytes: MAX_BYTES,
        });
    }
    let declared: u64 = files.iter().map(|file| file.bytes).sum();
    if declared > MAX_BYTES {
        return Err(BackupError::TooLarge {
            max_files: MAX_FILES,
            max_bytes: MAX_BYTES,
        });
    }

    let db = source.join("index.db");
    let db_snapshot = if db.is_file() {
        Some(snapshot_database(&db).await?)
    } else {
        None
    };
    if let Some(snapshot) = &db_snapshot {
        let db_bytes = snapshot
            .path()
            .metadata()
            .map_err(|source| io_error(snapshot.path(), source))?
            .len();
        if files.len() + 1 > MAX_FILES || declared + db_bytes > MAX_BYTES {
            return Err(BackupError::TooLarge {
                max_files: MAX_FILES,
                max_bytes: MAX_BYTES,
            });
        }
    }

    let encryptor = age::Encryptor::with_user_passphrase(age::secrecy::SecretString::from(
        passphrase.to_owned(),
    ));
    let target = File::create(temp).map_err(|source| io_error(temp, source))?;
    let writer = encryptor
        .wrap_output(target)
        .map_err(|error| BackupError::Crypto(error.to_string()))?;
    let mut archive = tar::Builder::new(writer);
    let mut manifest_files = Vec::with_capacity(files.len() + usize::from(db_snapshot.is_some()));
    let mut total = 0u64;

    for file in &files {
        if file.relative == "index.db" {
            continue;
        }
        let (bytes, sha256) = append_file(
            &mut archive,
            &file.absolute,
            &format!("{DATA_PREFIX}{}", file.relative),
        )?;
        total += bytes;
        manifest_files.push(BackupFile {
            path: file.relative.clone(),
            bytes,
            sha256,
        });
    }
    if let Some(snapshot) = &db_snapshot {
        let (bytes, sha256) = append_file(
            &mut archive,
            snapshot.path(),
            &format!("{DATA_PREFIX}index.db"),
        )?;
        total += bytes;
        manifest_files.push(BackupFile {
            path: "index.db".to_string(),
            bytes,
            sha256,
        });
    }
    manifest_files.sort_by(|left, right| left.path.cmp(&right.path));

    let manifest = Manifest {
        format: FORMAT.to_string(),
        created_at: now_ms(),
        source: source.display().to_string(),
        files: manifest_files,
        total_bytes: total,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| BackupError::Json(error.to_string()))?;
    append_bytes(&mut archive, MANIFEST, &manifest_bytes)?;
    let writer = archive
        .into_inner()
        .map_err(|error| BackupError::Tar(error.to_string()))?;
    writer
        .finish()
        .map_err(|error| BackupError::Crypto(error.to_string()))?;

    Ok(CreateReport {
        path: temp.to_path_buf(),
        files: manifest.files.len(),
        bytes: total,
        created_at: manifest.created_at,
        source: source.to_path_buf(),
        excluded,
    })
}

fn append_file(
    archive: &mut tar::Builder<impl io::Write>,
    path: &Path,
    name: &str,
) -> Result<(u64, String), BackupError> {
    let mut file = File::open(path).map_err(|source| io_error(path, source))?;
    let mut hasher = Sha256::new();
    let bytes = io::copy(&mut file, &mut hasher).map_err(|source| io_error(path, source))?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))
        .map_err(|source| io_error(path, source))?;

    let mut header = tar::Header::new_gnu();
    header.set_size(bytes);
    header.set_mode(0o600);
    header.set_mtime(now_ms().max(0) as u64 / 1_000);
    header.set_cksum();
    archive
        .append_data(&mut header, name, &mut file)
        .map_err(|error| BackupError::Tar(error.to_string()))?;
    Ok((bytes, hex(&hasher.finalize())))
}

fn append_bytes(
    archive: &mut tar::Builder<impl io::Write>,
    name: &str,
    bytes: &[u8],
) -> Result<(), BackupError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(now_ms().max(0) as u64 / 1_000);
    header.set_cksum();
    archive
        .append_data(&mut header, name, bytes)
        .map_err(|error| BackupError::Tar(error.to_string()))
}

/// Decrypt and authenticate an archive, hashing every file and validating the manifest.
pub fn verify(path: &Path, passphrase: &str) -> Result<VerifyReport, BackupError> {
    if passphrase.is_empty() {
        return Err(BackupError::EmptyPassphrase);
    }
    let (manifest, _) = scan(path, passphrase)?;
    Ok(VerifyReport {
        path: path.to_path_buf(),
        files: manifest.files.len(),
        bytes: manifest.total_bytes,
        created_at: manifest.created_at,
        source: manifest.source,
    })
}

/// Restore into a directory that does not exist or is empty. Verification happens before any
/// destination file is created; the extraction is staged beside the destination and renamed only
/// after every hash matches.
pub fn restore(
    archive: &Path,
    into: &Path,
    passphrase: &str,
) -> Result<RestoreReport, BackupError> {
    let (manifest, expected) = scan(archive, passphrase)?;
    let destination = normalize_path(&absolute_path(into)?);
    if destination.is_file() {
        return Err(BackupError::RestoreOverFile(destination));
    }
    if destination.is_dir()
        && fs::read_dir(&destination)
            .map_err(|source| io_error(&destination, source))?
            .next()
            .is_some()
    {
        return Err(BackupError::RestoreNotEmpty(destination));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    create_dir(parent)?;
    let staging = sibling_temp(&destination);
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    create_dir(&staging)?;

    let extracted = extract(archive, passphrase, &staging, &manifest, &expected);
    if let Err(error) = extracted {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if destination.is_dir() {
        fs::remove_dir(&destination).map_err(|source| io_error(&destination, source))?;
    }
    if let Err(source) = fs::rename(&staging, &destination) {
        let _ = fs::remove_dir_all(&staging);
        return Err(io_error(&destination, source));
    }
    Ok(RestoreReport {
        path: archive.to_path_buf(),
        into: destination,
        files: manifest.files.len(),
        bytes: manifest.total_bytes,
    })
}

/// Re-encrypt the same tar stream under a new passphrase. The new archive is verified before it
/// replaces the output path, so a bad new key cannot leave a half-written backup.
pub fn rotate(
    archive: &Path,
    out: &Path,
    old_passphrase: &str,
    new_passphrase: &str,
) -> Result<RotateReport, BackupError> {
    if old_passphrase.is_empty() || new_passphrase.is_empty() {
        return Err(BackupError::EmptyPassphrase);
    }
    let destination = normalize_path(&absolute_path(out)?);
    if destination.exists() {
        return Err(BackupError::DestinationExists(destination));
    }
    if let Some(parent) = destination.parent() {
        create_dir(parent)?;
    }
    let temp = sibling_temp(&destination);
    let result = (|| {
        let mut reader = open_archive(archive, old_passphrase)?;
        let encryptor = age::Encryptor::with_user_passphrase(age::secrecy::SecretString::from(
            new_passphrase.to_owned(),
        ));
        let target = File::create(&temp).map_err(|source| io_error(&temp, source))?;
        let mut writer = encryptor
            .wrap_output(target)
            .map_err(|error| BackupError::Crypto(error.to_string()))?;
        io::copy(&mut reader, &mut writer).map_err(|source| io_error(&temp, source))?;
        writer
            .finish()
            .map_err(|error| BackupError::Crypto(error.to_string()))?;
        let verified = verify(&temp, new_passphrase)?;
        fs::rename(&temp, &destination).map_err(|source| io_error(&destination, source))?;
        Ok(RotateReport {
            path: destination,
            files: verified.files,
            bytes: verified.bytes,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Restore into a temporary directory, verify every file, and run SQLite's integrity check.
/// The temporary tree is always removed, so a drill is safe to run from a scheduler.
pub async fn drill(archive: &Path, passphrase: &str) -> Result<DrillReport, BackupError> {
    let temp = std::env::temp_dir().join(format!(
        "rigel-backup-drill-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = async {
        let restored = restore(archive, &temp, passphrase)?;
        let database = check_database(&temp.join("index.db")).await?;
        Ok(DrillReport {
            ok: true,
            archive: archive.display().to_string(),
            files: restored.files,
            bytes: restored.bytes,
            database,
            checked_at: now_ms(),
        })
    }
    .await;
    let _ = fs::remove_dir_all(&temp);
    result
}

async fn check_database(path: &Path) -> Result<String, BackupError> {
    if !path.is_file() {
        return Ok("not present".to_string());
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| BackupError::Database(error.to_string()))?;
    let result: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .map_err(|error| BackupError::Database(error.to_string()))?;
    pool.close().await;
    if result.eq_ignore_ascii_case("ok") {
        Ok("ok".to_string())
    } else {
        Err(BackupError::Database(format!(
            "SQLite integrity check said {result}"
        )))
    }
}

fn scan(
    archive: &Path,
    passphrase: &str,
) -> Result<(Manifest, BTreeMap<String, BackupFile>), BackupError> {
    let reader = open_archive(archive, passphrase)?;
    let mut tar = tar::Archive::new(reader);
    let mut manifest: Option<Manifest> = None;
    let mut seen: BTreeMap<String, BackupFile> = BTreeMap::new();
    for entry in tar
        .entries()
        .map_err(|error| BackupError::Tar(error.to_string()))?
    {
        let mut entry = entry.map_err(|error| BackupError::Tar(error.to_string()))?;
        let name = entry
            .path()
            .map_err(|error| BackupError::Tar(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if name == MANIFEST {
            if manifest.is_some() {
                return Err(BackupError::Corrupt(
                    "the archive contains more than one manifest".into(),
                ));
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|source| io_error(archive, source))?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(BackupError::Manifest(
                    "manifest is larger than 16 MiB".into(),
                ));
            }
            manifest = Some(
                serde_json::from_slice(&bytes)
                    .map_err(|error| BackupError::Manifest(error.to_string()))?,
            );
            continue;
        }
        let Some(relative) = name.strip_prefix(DATA_PREFIX) else {
            return Err(BackupError::Corrupt(format!(
                "unexpected archive entry {name}"
            )));
        };
        validate_relative_path(relative)?;
        if !entry.header().entry_type().is_file() {
            return Err(BackupError::Corrupt(format!(
                "archive entry {name} is not a regular file"
            )));
        }
        let mut hasher = Sha256::new();
        let bytes =
            io::copy(&mut entry, &mut hasher).map_err(|source| io_error(archive, source))?;
        let file = BackupFile {
            path: relative.to_string(),
            bytes,
            sha256: hex(&hasher.finalize()),
        };
        if seen.insert(relative.to_string(), file).is_some() {
            return Err(BackupError::Corrupt(format!(
                "archive contains {relative} more than once"
            )));
        }
    }

    let manifest = manifest.ok_or_else(|| BackupError::Manifest("no manifest".into()))?;
    if manifest.format != FORMAT {
        return Err(BackupError::Manifest(format!(
            "unsupported format {}",
            manifest.format
        )));
    }
    if manifest.files.len() != seen.len()
        || manifest.total_bytes != seen.values().map(|f| f.bytes).sum::<u64>()
    {
        return Err(BackupError::Corrupt(
            "the manifest does not match the archived file list".into(),
        ));
    }
    let mut declared = BTreeSet::new();
    for file in &manifest.files {
        validate_relative_path(&file.path)?;
        if !declared.insert(file.path.clone()) {
            return Err(BackupError::Manifest(format!(
                "{} appears more than once",
                file.path
            )));
        }
        let Some(found) = seen.get(&file.path) else {
            return Err(BackupError::Corrupt(format!(
                "{} is in the manifest but not the archive",
                file.path
            )));
        };
        if found.bytes != file.bytes || found.sha256 != file.sha256 {
            return Err(BackupError::Corrupt(format!(
                "{} does not match its manifest hash",
                file.path
            )));
        }
    }
    for path in seen.keys() {
        if !declared.contains(path) {
            return Err(BackupError::Corrupt(format!(
                "{path} is in the archive but not the manifest"
            )));
        }
    }
    Ok((manifest, seen))
}

fn extract(
    archive: &Path,
    passphrase: &str,
    destination: &Path,
    manifest: &Manifest,
    expected_files: &BTreeMap<String, BackupFile>,
) -> Result<(), BackupError> {
    let reader = open_archive(archive, passphrase)?;
    let mut tar = tar::Archive::new(reader);
    let mut written = BTreeSet::new();
    for entry in tar
        .entries()
        .map_err(|error| BackupError::Tar(error.to_string()))?
    {
        let mut entry = entry.map_err(|error| BackupError::Tar(error.to_string()))?;
        let name = entry
            .path()
            .map_err(|error| BackupError::Tar(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if name == MANIFEST {
            continue;
        }
        let Some(relative) = name.strip_prefix(DATA_PREFIX) else {
            return Err(BackupError::Corrupt(format!(
                "unexpected archive entry {name}"
            )));
        };
        validate_relative_path(relative)?;
        if !entry.header().entry_type().is_file() {
            return Err(BackupError::Corrupt(format!(
                "archive entry {name} is not a regular file"
            )));
        }
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            create_dir(parent)?;
        }
        let mut file = File::create(&target).map_err(|source| io_error(&target, source))?;
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|source| io_error(&target, source))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|source| io_error(&target, source))?;
            bytes += read as u64;
        }
        let sha256 = hex(&hasher.finalize());
        let expected = expected_files
            .get(relative)
            .ok_or_else(|| BackupError::Corrupt(format!("{relative} is not in the manifest")))?;
        if bytes != expected.bytes || sha256 != expected.sha256 {
            return Err(BackupError::Corrupt(format!(
                "{relative} does not match its manifest hash"
            )));
        }
        file.flush().map_err(|source| io_error(&target, source))?;
        written.insert(relative.to_string());
    }
    if written.len() != manifest.files.len() {
        return Err(BackupError::Corrupt(format!(
            "restored {} of {} files",
            written.len(),
            manifest.files.len()
        )));
    }
    Ok(())
}

fn open_archive(path: &Path, passphrase: &str) -> Result<impl Read, BackupError> {
    let file = File::open(path).map_err(|source| io_error(path, source))?;
    let decryptor =
        age::Decryptor::new(file).map_err(|error| BackupError::Crypto(error.to_string()))?;
    let identity =
        age::scrypt::Identity::new(age::secrecy::SecretString::from(passphrase.to_owned()));
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|error| BackupError::Crypto(error.to_string()))?;
    Ok(reader)
}

struct TempSnapshot(PathBuf);

impl TempSnapshot {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

async fn snapshot_database(db: &Path) -> Result<TempSnapshot, BackupError> {
    let out = std::env::temp_dir().join(format!(
        "{TEMP_PREFIX}db-{}.db",
        uuid::Uuid::new_v4().simple()
    ));
    let _ = fs::remove_file(&out);
    let options = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| BackupError::Database(error.to_string()))?;
    let result = sqlx::query("VACUUM INTO ?")
        .bind(out.display().to_string())
        .execute(&pool)
        .await
        .map_err(|error| BackupError::Database(error.to_string()));
    pool.close().await;
    if let Err(error) = result {
        let _ = fs::remove_file(&out);
        return Err(error);
    }
    Ok(TempSnapshot(out))
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<SourceFile>,
    excluded: &mut Vec<String>,
) -> Result<(), BackupError> {
    let mut entries = fs::read_dir(current)
        .map_err(|source| io_error(current, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| io_error(current, source))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| BackupError::Corrupt(format!("{} is outside the source", path.display())))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let file_type = entry
            .file_type()
            .map_err(|source| io_error(&path, source))?;
        if file_type.is_dir() {
            if current == root
                && matches!(
                    name.as_str(),
                    "updates" | "backups" | "diagnostics" | "agentchat"
                )
            {
                excluded.push(format!(
                    "{name}/ (re-creatable or generated; not part of a data backup)"
                ));
                continue;
            }
            collect_files(root, &path, files, excluded)?;
        } else if file_type.is_file() {
            if relative == "index.db" || relative == "index.db-wal" || relative == "index.db-shm" {
                continue;
            }
            if relative.contains('\\') {
                return Err(BackupError::UnsafePath(format!(
                    "{relative} contains a backslash, which cannot be restored safely on Windows"
                )));
            }
            let bytes = entry
                .metadata()
                .map_err(|source| io_error(&path, source))?
                .len();
            files.push(SourceFile {
                relative,
                absolute: path,
                bytes,
            });
        } else {
            excluded.push(format!("{relative} (not a regular file)"));
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), BackupError> {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return Err(BackupError::UnsafePath(path.to_string()));
    }
    let parsed = Path::new(path);
    let mut components = parsed.components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BackupError::UnsafePath(path.to_string()));
    }
    Ok(())
}

fn canonical_dir(path: &Path) -> Result<PathBuf, BackupError> {
    path.canonicalize()
        .map(|path| strip_verbatim(&path))
        .map_err(|source| io_error(path, source))
}

fn absolute_path(path: &Path) -> Result<PathBuf, BackupError> {
    let absolute = if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|source| io_error(path, source))
    }?;
    Ok(strip_verbatim(&absolute))
}

/// Make a lexical absolute path with `.` and `..` resolved, so a destination cannot hide inside
/// the source behind `..` before it exists.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    strip_verbatim(&out)
}

/// `canonicalize` returns a verbatim `\\?\C:\...` path on Windows. Strip only that prefix so a
/// path compared against a normal absolute path still matches; the filesystem meaning is the
/// same for local paths.
fn strip_verbatim(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = path.to_string_lossy().strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

fn within(path: &Path, parent: &Path) -> bool {
    path.starts_with(parent)
}

fn sibling_temp(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "backup".to_string());
    let temp = format!(".{name}.part-{}", uuid::Uuid::new_v4().simple());
    destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(temp)
}

fn create_dir(path: &Path) -> Result<(), BackupError> {
    fs::create_dir_all(path).map_err(|source| io_error(path, source))
}

fn io_error(path: &Path, source: io::Error) -> BackupError {
    BackupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypted_bytes(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap()
    }

    #[test]
    fn unsafe_manifest_paths_are_refused() {
        for path in [
            "../secret",
            "/etc/passwd",
            "C:/Windows",
            "a/../../b",
            "a\\b",
        ] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
        for path in ["index.db", "snapshots/one/files/a.txt", "settings.toml"] {
            assert!(validate_relative_path(path).is_ok(), "{path}");
        }
    }

    #[tokio::test]
    async fn a_backup_round_trips_and_rotates_its_key() {
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("Rigel");
        fs::create_dir_all(source.join("snapshots/one/files")).unwrap();
        fs::write(source.join("settings.toml"), "theme = \"paper\"\n").unwrap();
        fs::write(source.join("snapshots/one/files/a.txt"), "the only copy\n").unwrap();
        let db = source.join("index.db");
        let options = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE facts (name TEXT);")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO facts VALUES ('kept');")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let archive = data.path().join("one.rigel-backup");
        let created = create(&source, &archive, "correct horse battery staple")
            .await
            .unwrap();
        assert_eq!(created.files, 3);
        let raw = encrypted_bytes(&archive);
        assert!(
            !raw.windows(15).any(|window| window == b"the only copy\n"),
            "plaintext is not in the encrypted archive"
        );

        let verified = verify(&archive, "correct horse battery staple").unwrap();
        assert_eq!(verified.files, 3);
        assert!(verify(&archive, "wrong passphrase").is_err());

        let restored = data.path().join("restored");
        restore(&archive, &restored, "correct horse battery staple").unwrap();
        assert_eq!(
            fs::read_to_string(restored.join("snapshots/one/files/a.txt")).unwrap(),
            "the only copy\n"
        );
        assert_eq!(
            fs::read_to_string(restored.join("settings.toml")).unwrap(),
            "theme = \"paper\"\n"
        );
        assert!(restored.join("index.db").is_file());

        let drill = drill(&archive, "correct horse battery staple")
            .await
            .unwrap();
        assert!(drill.ok);
        assert_eq!(drill.database, "ok");

        let rotated_path = data.path().join("rotated.rigel-backup");
        let rotated = rotate(
            &archive,
            &rotated_path,
            "correct horse battery staple",
            "another long passphrase",
        )
        .unwrap();
        assert!(rotated.path.ends_with("rotated.rigel-backup"));
        assert!(verify(&rotated_path, "another long passphrase").is_ok());
        assert!(verify(&rotated_path, "correct horse battery staple").is_err());

        let non_empty = data.path().join("not-empty");
        fs::create_dir(&non_empty).unwrap();
        fs::write(non_empty.join("keep.txt"), "keep").unwrap();
        assert!(restore(&archive, &non_empty, "correct horse battery staple").is_err());

        let mut tampered = encrypted_bytes(&archive);
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        let corrupt = data.path().join("corrupt.rigel-backup");
        fs::write(&corrupt, tampered).unwrap();
        assert!(verify(&corrupt, "correct horse battery staple").is_err());

        assert!(
            create(
                &source,
                &source.join("unexpected.rigel-backup"),
                "correct horse battery staple"
            )
            .await
            .is_err()
        );
        let inside = create(
            &source,
            &source.join("backups/inside.rigel-backup"),
            "correct horse battery staple",
        )
        .await
        .unwrap();
        assert_eq!(inside.files, 3);
    }
}
