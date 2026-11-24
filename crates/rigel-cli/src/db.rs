//! `rigel db` — inspect and change at-rest encryption for a project database.
//!
//! The key itself is never printed. `encrypt` stores a generated key in the OS keychain when
//! no key is configured; a headless caller sets `RIGEL_DB_KEY` instead.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::store::cipher::{
    DatabaseState, DbKey, database_state, decrypt_database, forget_key, resolve_key, store_key,
};
use serde_json::json;

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
        println!(
            "{}",
            json!({
                "path": path,
                "encrypted": encrypted,
                "key_configured": configured,
            })
        );
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

fn fail(error: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}
