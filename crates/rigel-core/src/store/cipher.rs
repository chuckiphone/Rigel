//! SQLCipher-at-rest for the project database (roadmap 76).
//!
//! The project database may be encrypted with SQLCipher. The key is a 32-byte value held in
//! the OS keychain (entry `rigel/db-key`) or supplied for headless runs with
//! `RIGEL_DB_KEY`, which must be 64 hexadecimal characters. The key is never written to a
//! project file and never logged.
//!
//! A key is opt-in: a project with no key stays plaintext, exactly as before. Once a key is
//! configured, opening an existing plaintext database migrates it in place before the store is
//! returned; opening an encrypted database without a key fails closed instead of creating an
//! empty plaintext database. Migration and decryption keep the source bytes until the new
//! file has passed `PRAGMA integrity_check`.
//!
//! Recovery limits: the migration is not a backup system. If the process is interrupted while
//! the file is being swapped, the original is left beside the database as
//! `rigel.pre-encryption-<pid>` or `rigel.pre-decryption-<pid>`. Rename it back before opening
//! the project. The daily backups under `.rigel/backups/` use the same key and are encrypted.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

/// Headless key override. A 64-character hexadecimal value, never a passphrase.
pub const KEY_ENV: &str = "RIGEL_DB_KEY";

/// The keychain entry holding the project database key.
const KEYCHAIN_SERVICE: &str = "rigel";
const KEYCHAIN_ACCOUNT: &str = "db-key";

const PLAINTEXT_HEADER: &[u8] = b"SQLite format 3\0";
const KEY_HEX_LEN: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum CipherError {
    #[error("the database encryption key must be exactly 64 hexadecimal characters")]
    InvalidKey,

    #[error("the OS keychain could not be read for the database key: {0}")]
    Keychain(String),

    #[error(
        "database {path} is encrypted but no key is configured; set RIGEL_DB_KEY to the 64-character key or run `rigel db encrypt` to store one in the OS keychain"
    )]
    MissingKey { path: PathBuf },

    #[error(
        "database {path} could not be opened with the configured key; the key is wrong or the file is corrupt ({detail})"
    )]
    WrongKey { path: PathBuf, detail: String },

    #[error("database {path} is already encrypted; decrypt it before encrypting it again")]
    AlreadyEncrypted { path: PathBuf },

    #[error("database {path} is not encrypted; there is nothing to decrypt")]
    NotEncrypted { path: PathBuf },

    #[error("could not read or replace {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Whether a database path is plaintext SQLite, SQLCipher, or not there yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseState {
    Missing,
    Plaintext,
    Encrypted,
}

/// A validated 32-byte database key.
#[derive(Clone)]
pub struct DbKey {
    hex: String,
}

impl DbKey {
    /// Parse the 64-character hexadecimal form used by the keychain and `RIGEL_DB_KEY`.
    pub fn parse(value: &str) -> Result<Self, CipherError> {
        let value = value.trim();
        if value.len() != KEY_HEX_LEN || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CipherError::InvalidKey);
        }
        Ok(Self {
            hex: value.to_ascii_lowercase(),
        })
    }

    /// Generate a fresh 256-bit key. The caller stores it; it is never printed.
    pub fn generate() -> Result<Self, CipherError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|error| CipherError::Keychain(error.to_string()))?;
        let mut hex = String::with_capacity(KEY_HEX_LEN);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
        }
        Self::parse(&hex)
    }

    /// The hexadecimal form. Public only so a caller can store or compare a key; it is not
    /// included in errors or `Debug`.
    pub fn expose_hex(&self) -> &str {
        &self.hex
    }

    pub(crate) fn pragma_value(&self) -> String {
        // SQLCipher's raw-key notation: the double quotes are part of the pragma value.
        format!("\"x'{}'\"", self.hex)
    }
}

impl std::fmt::Debug for DbKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DbKey(<redacted>)")
    }
}

/// Resolve the key from `RIGEL_DB_KEY` first, then the OS keychain. `None` means the project
/// is intentionally plaintext.
pub fn resolve_key() -> Result<Option<DbKey>, CipherError> {
    if let Ok(value) = std::env::var(KEY_ENV) {
        if value.trim().is_empty() {
            return Err(CipherError::InvalidKey);
        }
        return DbKey::parse(&value).map(Some);
    }
    keychain_key()
}

/// Store a key in the OS keychain. The value is passed to the platform store, never logged.
pub fn store_key(key: &DbKey) -> Result<(), CipherError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .map_err(|error| CipherError::Keychain(error.to_string()))?
        .set_password(&key.hex)
        .map_err(|error| CipherError::Keychain(error.to_string()))
}

/// Remove the stored key. This does not decrypt existing databases; they then need
/// `RIGEL_DB_KEY` to open.
pub fn forget_key() -> Result<(), CipherError> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .map_err(|error| CipherError::Keychain(error.to_string()))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(CipherError::Keychain(error.to_string())),
    }
}

fn keychain_key() -> Result<Option<DbKey>, CipherError> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .map_err(|error| CipherError::Keychain(error.to_string()))?;
    match entry.get_password() {
        Ok(value) => DbKey::parse(&value).map(Some),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(CipherError::Keychain(error.to_string())),
    }
}

/// Classify a file by its first sixteen bytes. SQLCipher leaves no SQLite header.
pub fn database_state(path: &Path) -> Result<DatabaseState, CipherError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DatabaseState::Missing);
        }
        Err(source) => {
            return Err(CipherError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut header = [0u8; 16];
    let read = file.read(&mut header).map_err(|source| CipherError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if read == 0 {
        return Ok(DatabaseState::Missing);
    }
    if read == header.len() && header == PLAINTEXT_HEADER {
        Ok(DatabaseState::Plaintext)
    } else {
        Ok(DatabaseState::Encrypted)
    }
}

pub(crate) fn options(path: &Path, key: Option<&DbKey>) -> SqliteConnectOptions {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    match key {
        Some(key) => options
            .pragma("key", key.pragma_value())
            .pragma("cipher_compatibility", "4"),
        None => options,
    }
}

/// Migrate a plaintext database to SQLCipher. The source is untouched until the encrypted
/// copy passes an integrity check.
pub async fn encrypt_database(path: &Path, key: &DbKey) -> Result<(), CipherError> {
    match database_state(path)? {
        DatabaseState::Plaintext => {}
        DatabaseState::Encrypted => {
            return Err(CipherError::AlreadyEncrypted {
                path: path.to_path_buf(),
            });
        }
        DatabaseState::Missing => {
            return Err(CipherError::NotEncrypted {
                path: path.to_path_buf(),
            });
        }
    }

    let temp = swap_path(path, "encrypting");
    remove_if_exists(&temp)?;
    let result = encrypt_into(path, &temp, key).await;
    match result {
        Ok(()) => {
            replace_with(path, &temp, "pre-encryption")?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

async fn encrypt_into(path: &Path, temp: &Path, key: &DbKey) -> Result<(), CipherError> {
    let mut source = plaintext_connection(path).await?;
    let attach = format!(
        "ATTACH DATABASE '{}' AS encrypted KEY {}",
        quote_path(temp),
        key.pragma_value()
    );
    sqlx::raw_sql(&attach).execute(&mut source).await?;
    sqlx::query("SELECT sqlcipher_export('encrypted')")
        .execute(&mut source)
        .await?;
    sqlx::raw_sql("DETACH DATABASE encrypted")
        .execute(&mut source)
        .await?;
    source.close().await?;
    verify(temp, Some(key)).await
}

/// Migrate a SQLCipher database back to plaintext. The encrypted source is untouched until
/// the plaintext copy passes an integrity check.
pub async fn decrypt_database(path: &Path, key: &DbKey) -> Result<(), CipherError> {
    match database_state(path)? {
        DatabaseState::Encrypted => {}
        DatabaseState::Plaintext => {
            return Err(CipherError::NotEncrypted {
                path: path.to_path_buf(),
            });
        }
        DatabaseState::Missing => {
            return Err(CipherError::NotEncrypted {
                path: path.to_path_buf(),
            });
        }
    }

    let temp = swap_path(path, "decrypting");
    remove_if_exists(&temp)?;
    let result = decrypt_into(path, &temp, key).await;
    match result {
        Ok(()) => {
            replace_with(path, &temp, "pre-decryption")?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

async fn decrypt_into(path: &Path, temp: &Path, key: &DbKey) -> Result<(), CipherError> {
    let mut source = keyed_connection(path, key).await?;
    // An ATTACH without KEY inherits the main database's key. The empty key makes the
    // export target plaintext.
    let attach = format!("ATTACH DATABASE '{}' AS plaintext KEY ''", quote_path(temp));
    sqlx::raw_sql(&attach).execute(&mut source).await?;
    sqlx::query("SELECT sqlcipher_export('plaintext')")
        .execute(&mut source)
        .await?;
    sqlx::raw_sql("DETACH DATABASE plaintext")
        .execute(&mut source)
        .await?;
    source.close().await?;
    verify(temp, None).await
}

async fn verify(path: &Path, key: Option<&DbKey>) -> Result<(), CipherError> {
    let mut conn = match key {
        Some(key) => keyed_connection(path, key).await?,
        None => plaintext_connection(path).await?,
    };
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut conn)
        .await?;
    let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type = 'table'")
        .fetch_one(&mut conn)
        .await?;
    conn.close().await?;
    if !integrity.eq_ignore_ascii_case("ok") || tables == 0 {
        return Err(CipherError::Database(sqlx::Error::Protocol(format!(
            "migrated database failed its integrity check: {integrity}"
        ))));
    }
    Ok(())
}

async fn plaintext_connection(path: &Path) -> Result<SqliteConnection, CipherError> {
    // The create flag belongs to the connection, not just the source file: SQLite will not
    // create an ATTACH target from a connection opened without it.
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    Ok(SqliteConnection::connect_with(&options).await?)
}

async fn keyed_connection(path: &Path, key: &DbKey) -> Result<SqliteConnection, CipherError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .pragma("key", key.pragma_value())
        .pragma("cipher_compatibility", "4");
    Ok(SqliteConnection::connect_with(&options).await?)
}

fn swap_path(path: &Path, purpose: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rigel.db".to_string());
    path.with_file_name(format!("{name}.{purpose}-{}", std::process::id()))
}

fn replace_with(path: &Path, temp: &Path, purpose: &str) -> Result<(), CipherError> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rigel.db".to_string());
    let backup = path.with_file_name(format!("{name}.{purpose}-{}", std::process::id()));
    remove_if_exists(&backup)?;
    remove_sidecars(path)?;

    fs::rename(path, &backup).map_err(|source| CipherError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Err(source) = fs::rename(temp, path) {
        let _ = fs::rename(&backup, path);
        return Err(CipherError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

fn remove_sidecars(path: &Path) -> Result<(), CipherError> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        remove_if_exists(&sidecar)?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), CipherError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CipherError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn quote_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_64_hex_characters_and_redacts_itself() {
        let key = DbKey::parse(&"ab".repeat(32)).unwrap();
        assert_eq!(key.expose_hex(), "ab".repeat(32));
        assert_eq!(format!("{key:?}"), "DbKey(<redacted>)");
        assert!(matches!(
            DbKey::parse("not-a-key"),
            Err(CipherError::InvalidKey)
        ));
        assert!(matches!(
            DbKey::parse(&"zz".repeat(32)),
            Err(CipherError::InvalidKey)
        ));
    }

    #[test]
    fn generated_keys_are_valid_and_distinct() {
        let first = DbKey::generate().unwrap();
        let second = DbKey::generate().unwrap();
        assert_eq!(first.expose_hex().len(), 64);
        assert_ne!(first.expose_hex(), second.expose_hex());
    }

    #[test]
    fn a_missing_file_is_missing_and_a_plaintext_header_is_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rigel.db");
        assert_eq!(database_state(&path).unwrap(), DatabaseState::Missing);
        fs::write(&path, b"not a database").unwrap();
        assert_eq!(database_state(&path).unwrap(), DatabaseState::Encrypted);
        fs::write(&path, PLAINTEXT_HEADER).unwrap();
        assert_eq!(database_state(&path).unwrap(), DatabaseState::Plaintext);
    }
}
