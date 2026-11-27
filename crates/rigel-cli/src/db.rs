//! `rigel db` — inspect encryption and maintain a project's SQLite database.
//!
//! The key itself is never printed. `encrypt` stores a generated key in the OS keychain when
//! no key is configured; a headless caller sets `RIGEL_DB_KEY` instead.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use rigel_core::store::cipher::{
    DatabaseState, DbKey, database_state, decrypt_database, forget_key, resolve_key, store_key,
};
use serde::Serialize;
use serde_json::json;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

#[derive(Subcommand)]
pub enum DbCommand {
    /// Show whether the project database is encrypted and whether a key is configured.
    Status {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Encrypt a plaintext project database. Stores a generated key in the keychain if none exists.
    Encrypt {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
    },

    /// Decrypt an encrypted project database with the configured key.
    Decrypt {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
    },

    /// Remove the key from the OS keychain. Existing encrypted databases then need RIGEL_DB_KEY.
    ForgetKey,

    /// Checkpoint committed WAL pages into the database.
    Checkpoint {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// PASSIVE never waits for readers; FULL and RESTART wait up to SQLite's busy timeout.
        #[arg(long, value_enum, default_value_t = CheckpointMode::Passive)]
        mode: CheckpointMode,
        #[arg(long)]
        json: bool,
    },

    /// Refresh SQLite's query planner statistics.
    Analyze {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
    },

    /// Report database pages, bytes, WAL size, and bytes used by every table.
    Size {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Reclaim unused pages using SQLite VACUUM while WAL readers remain available.
    Vacuum {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum CheckpointMode {
    #[default]
    Passive,
    Full,
    Restart,
    Truncate,
}

impl CheckpointMode {
    fn pragma(self) -> &'static str {
        match self {
            Self::Passive => "PASSIVE",
            Self::Full => "FULL",
            Self::Restart => "RESTART",
            Self::Truncate => "TRUNCATE",
        }
    }
}

#[derive(Debug, Serialize)]
struct CheckpointReport {
    path: PathBuf,
    mode: String,
    busy: i32,
    wal_frames: i32,
    checkpointed_frames: i32,
}

#[derive(Debug, Serialize)]
struct TableSize {
    name: String,
    pages: i64,
    bytes: i64,
}

#[derive(Debug, Serialize)]
struct SizeReport {
    path: PathBuf,
    page_size: i64,
    page_count: i64,
    free_pages: i64,
    database_bytes: i64,
    wal_bytes: u64,
    tables: Vec<TableSize>,
}

pub async fn run(command: DbCommand) -> ExitCode {
    match command {
        DbCommand::Status {
            project,
            json: as_json,
        } => status(&project, as_json),
        DbCommand::Encrypt { project } => encrypt(&project).await,
        DbCommand::Decrypt { project } => decrypt(&project).await,
        DbCommand::ForgetKey => match forget_key() {
            Ok(()) => {
                println!("database key removed from the OS keychain");
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error),
        },
        DbCommand::Checkpoint {
            project,
            mode,
            json,
        } => checkpoint(&project, mode, json).await,
        DbCommand::Analyze { project } => analyze(&project).await,
        DbCommand::Size { project, json } => size(&project, json).await,
        DbCommand::Vacuum { project } => vacuum(&project).await,
    }
}

fn status(project: &Path, as_json: bool) -> ExitCode {
    let path = project.join(".rigel").join("rigel.db");
    let state = match database_state(&path) {
        Ok(state) => state,
        Err(error) => return fail(&error),
    };
    let key = match resolve_key() {
        Ok(key) => key,
        Err(error) => return fail(&error),
    };
    let encrypted = state == DatabaseState::Encrypted;
    let configured = key.is_some();
    if as_json {
        crate::json::print(json!({
            "path": path,
            "encrypted": encrypted,
            "key_configured": configured,
        }));
    } else {
        println!("database: {}", path.display());
        println!(
            "encryption: {}",
            if encrypted { "SQLCipher" } else { "plaintext" }
        );
        println!(
            "key: {}",
            if configured {
                "configured"
            } else {
                "not configured (the project stays plaintext)"
            }
        );
    }
    ExitCode::SUCCESS
}

async fn encrypt(project: &Path) -> ExitCode {
    let path = project.join(".rigel").join("rigel.db");
    let key = match resolve_key() {
        Ok(Some(key)) => key,
        Ok(None) => match DbKey::generate().and_then(|key| {
            store_key(&key)?;
            Ok(key)
        }) {
            Ok(key) => key,
            Err(error) => return fail(&error),
        },
        Err(error) => return fail(&error),
    };

    let store = match rigel_core::Store::open_project_with_key(project, Some(&key)).await {
        Ok(store) => store,
        Err(error) => return fail(&error),
    };
    store.pool().close().await;
    println!("encrypted {}", path.display());
    ExitCode::SUCCESS
}

async fn decrypt(project: &Path) -> ExitCode {
    let path = project.join(".rigel").join("rigel.db");
    let key = match resolve_key() {
        Ok(Some(key)) => key,
        Ok(None) => {
            eprintln!(
                "rigel: database {} is encrypted but no key is configured; set RIGEL_DB_KEY",
                path.display()
            );
            return ExitCode::from(2);
        }
        Err(error) => return fail(&error),
    };
    match decrypt_database(&path, &key).await {
        Ok(()) => {
            println!("decrypted {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

/// `PRAGMA wal_checkpoint` answers three integers. Read them with checked accessors: a
/// different SQLite build or a malformed answer must be a named error, never a panic on a row.
fn checkpoint_report(
    path: &Path,
    mode: CheckpointMode,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CheckpointReport, String> {
    if row.columns().len() < 3 {
        return Err(format!(
            "wal_checkpoint returned {} column(s), expected 3",
            row.columns().len()
        ));
    }
    let read = |index: usize, name: &str| {
        row.try_get::<i32, _>(index)
            .map_err(|error| format!("wal_checkpoint {name} column: {error}"))
    };
    Ok(CheckpointReport {
        path: path.to_path_buf(),
        mode: mode.pragma().to_ascii_lowercase(),
        busy: read(0, "busy")?,
        wal_frames: read(1, "wal_frames")?,
        checkpointed_frames: read(2, "checkpointed_frames")?,
    })
}

async fn checkpoint(project: &Path, mode: CheckpointMode, as_json: bool) -> ExitCode {
    let (path, pool) = match open_maintenance(project).await {
        Ok(open) => open,
        Err(error) => return fail(&error),
    };
    let pragma = format!("PRAGMA wal_checkpoint({})", mode.pragma());
    let result = async {
        let row = sqlx::query(&pragma)
            .fetch_one(&pool)
            .await
            .map_err(|error| error.to_string())?;
        checkpoint_report(&path, mode, &row)
    }
    .await;
    // Close on both paths: a failed pragma must not leave a pooled connection holding the DB.
    pool.close().await;
    let report = match result {
        Ok(report) => report,
        Err(error) => return fail(&error),
    };
    if as_json {
        crate::json::print(json!(report));
    } else {
        println!(
            "{}: {} checkpoint: {} of {} WAL frames copied{}",
            report.path.display(),
            report.mode,
            report.checkpointed_frames,
            report.wal_frames,
            if report.busy == 0 { "" } else { " (busy)" }
        );
    }
    ExitCode::SUCCESS
}

async fn analyze(project: &Path) -> ExitCode {
    let (path, pool) = match open_maintenance(project).await {
        Ok(open) => open,
        Err(error) => return fail(&error),
    };
    let result = sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .map_err(|error| error.to_string());
    pool.close().await;
    match result {
        Ok(_) => {
            println!("updated query planner statistics for {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

async fn size(project: &Path, as_json: bool) -> ExitCode {
    let (path, pool) = match open_maintenance(project).await {
        Ok(open) => open,
        Err(error) => return fail(&error),
    };
    let result = size_report(&path, &pool).await;
    pool.close().await;
    let report = match result {
        Ok(report) => report,
        Err(error) => return fail(&error),
    };
    if as_json {
        crate::json::print(json!(report));
    } else {
        println!("database: {}", report.path.display());
        println!(
            "pages: {} total, {} free at {} bytes/page ({} bytes; WAL {} bytes)",
            report.page_count,
            report.free_pages,
            report.page_size,
            report.database_bytes,
            report.wal_bytes
        );
        for table in report.tables {
            println!(
                "table {}: {} pages ({} bytes)",
                table.name, table.pages, table.bytes
            );
        }
    }
    ExitCode::SUCCESS
}

async fn vacuum(project: &Path) -> ExitCode {
    let (path, pool) = match open_maintenance(project).await {
        Ok(open) => open,
        Err(error) => return fail(&error),
    };
    // VACUUM is issued outside a transaction. In WAL mode SQLite can rebuild the file while
    // application readers continue using their snapshots; busy_timeout lets concurrent writers
    // finish instead of returning SQLITE_BUSY immediately.
    let result = sqlx::query("VACUUM")
        .execute(&pool)
        .await
        .map_err(|error| error.to_string());
    pool.close().await;
    match result {
        Ok(_) => {
            println!("vacuumed {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

async fn open_maintenance(project: &Path) -> Result<(PathBuf, SqlitePool), String> {
    let path = project.join(".rigel").join("rigel.db");
    let state = database_state(&path).map_err(|error| error.to_string())?;
    if state == DatabaseState::Missing {
        return Err(format!("database {} does not exist", path.display()));
    }
    let key = resolve_key().map_err(|error| error.to_string())?;
    if state == DatabaseState::Encrypted && key.is_none() {
        return Err(format!(
            "database {} is encrypted; configure RIGEL_DB_KEY before maintenance",
            path.display()
        ));
    }
    let mut options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(false)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    if state == DatabaseState::Encrypted {
        let key = key.expect("encrypted databases require a key");
        options = options
            .pragma("key", format!("\"x'{}'\"", key.expose_hex()))
            .pragma("cipher_compatibility", "4");
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let journal_mode: String = match sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
    {
        Ok(mode) => mode,
        Err(error) => {
            pool.close().await;
            return Err(error.to_string());
        }
    };
    if !journal_mode.eq_ignore_ascii_case("wal") {
        pool.close().await;
        return Err(format!(
            "database {} uses {journal_mode} mode; maintenance requires WAL mode",
            path.display()
        ));
    }
    Ok((path, pool))
}

async fn size_report(path: &Path, pool: &SqlitePool) -> Result<SizeReport, sqlx::Error> {
    let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
        .fetch_one(pool)
        .await?;
    let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
        .fetch_one(pool)
        .await?;
    let free_pages: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query(
        "SELECT dbstat.name, COUNT(*) AS pages, SUM(dbstat.pgsize) AS bytes \
         FROM dbstat \
         JOIN sqlite_schema ON sqlite_schema.name = dbstat.name \
         WHERE sqlite_schema.type = 'table' \
         GROUP BY dbstat.name ORDER BY dbstat.name",
    )
    .fetch_all(pool)
    .await?;
    let tables = rows
        .into_iter()
        .map(|row| TableSize {
            name: row.get("name"),
            pages: row.get("pages"),
            bytes: row.get("bytes"),
        })
        .collect();
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(SizeReport {
        path: path.to_path_buf(),
        page_size,
        page_count,
        free_pages,
        database_bytes: page_size.saturating_mul(page_count),
        wal_bytes,
        tables,
    })
}

fn fail(error: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn fixture(path: &Path) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sample (id INTEGER PRIMARY KEY, payload TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn size_report_includes_pages_for_each_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rigel.db");
        let pool = fixture(&path).await;
        sqlx::query("INSERT INTO sample(payload) VALUES ('hello'), ('world')")
            .execute(&pool)
            .await
            .unwrap();
        let report = size_report(&path, &pool).await.unwrap();
        assert!(report.database_bytes >= report.page_size);
        assert!(report.tables.iter().any(|table| table.name == "sample"));
        assert!(
            report
                .tables
                .iter()
                .all(|table| table.pages > 0 && table.bytes > 0)
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn checkpoint_report_never_panics_on_a_short_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rigel.db");
        let pool = fixture(&path).await;

        let short = sqlx::query("SELECT 1 AS busy")
            .fetch_one(&pool)
            .await
            .unwrap();
        let error = checkpoint_report(&path, CheckpointMode::Passive, &short).unwrap_err();
        assert!(error.contains("expected 3"), "{error}");

        let full = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .fetch_one(&pool)
            .await
            .unwrap();
        let report = checkpoint_report(&path, CheckpointMode::Passive, &full).unwrap();
        assert_eq!(report.busy, 0);
        assert_eq!(report.mode, "passive");
        pool.close().await;
    }

    #[tokio::test]
    async fn analyze_and_passive_checkpoint_complete_in_wal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rigel.db");
        let pool = fixture(&path).await;
        sqlx::query("CREATE INDEX sample_payload ON sample(payload)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("ANALYZE").execute(&pool).await.unwrap();
        let stats_table: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE name = 'sqlite_stat1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stats_table, 1);

        let checkpoint = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .fetch_one(&pool)
            .await
            .unwrap();
        let busy: i32 = checkpoint.get(0);
        assert_eq!(busy, 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn vacuum_keeps_wal_readers_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rigel.db");
        let pool = fixture(&path).await;
        let payload = "x".repeat(4096);
        for _ in 0..500 {
            sqlx::query("INSERT INTO sample(payload) VALUES (?)")
                .bind(&payload)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM sample WHERE id % 2 = 0")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await
            .unwrap();

        let vacuum_pool = pool.clone();
        let vacuum_started = Arc::new(AtomicBool::new(false));
        let vacuum_done = Arc::new(AtomicBool::new(false));
        let started = vacuum_started.clone();
        let done = vacuum_done.clone();
        let vacuum = tokio::spawn(async move {
            started.store(true, Ordering::SeqCst);
            sqlx::query("VACUUM").execute(&vacuum_pool).await.unwrap();
            done.store(true, Ordering::SeqCst);
        });

        while !vacuum_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        let mut reader_completed_during_vacuum = false;
        while !vacuum_done.load(Ordering::SeqCst) {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sample")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 250);
            reader_completed_during_vacuum = true;
            tokio::task::yield_now().await;
        }
        vacuum.await.unwrap();
        assert!(reader_completed_during_vacuum);
        pool.close().await;
    }
}
