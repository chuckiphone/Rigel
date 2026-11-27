//! Per-project retention controls. Destructive apply requires the exact preview totals.

use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::retention::RetentionPolicy;
use rigel_core::{Store, default_index_dir};
use serde_json::json;

#[derive(Subcommand)]
pub enum RetentionCommand {
    /// Set retention windows in days. A value of zero disables that cleanup category.
    Set {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: std::path::PathBuf,
        #[arg(long, default_value_t = 90)]
        turns_days: u32,
        #[arg(long, default_value_t = 90)]
        blobs_days: u32,
        #[arg(long, default_value_t = 90)]
        logs_days: u32,
        #[arg(long, default_value_t = 90)]
        snapshots_days: u32,
        #[arg(long)]
        json: bool,
    },
    /// Preview retained data that is eligible for deletion.
    Preview {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: std::path::PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Apply cleanup only when preview count and byte totals are confirmed exactly.
    Apply {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: std::path::PathBuf,
        #[arg(long, required = true)]
        confirm_count: u64,
        #[arg(long, required = true)]
        confirm_bytes: u64,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: RetentionCommand) -> ExitCode {
    match command {
        RetentionCommand::Set {
            project,
            turns_days,
            blobs_days,
            logs_days,
            snapshots_days,
            json: as_json,
        } => {
            let policy = RetentionPolicy {
                turns_days,
                blobs_days,
                logs_days,
                snapshots_days,
            };
            match rigel_core::retention::save_policy(&project, policy) {
                Ok(()) if as_json => {
                    crate::json::print(json!({"policy": policy}));
                    ExitCode::SUCCESS
                }
                Ok(()) => {
                    println!("retention policy saved for {}", project.display());
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error),
            }
        }
        RetentionCommand::Preview {
            project,
            json: as_json,
        } => match open(&project).await {
            Ok(store) => match rigel_core::retention::preview(
                &store,
                &project,
                &default_index_dir(),
                rigel_core::now_ms(),
            )
            .await
            {
                Ok(report) => {
                    print_report(&report, as_json);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error),
            },
            Err(error) => fail(&error),
        },
        RetentionCommand::Apply {
            project,
            confirm_count,
            confirm_bytes,
            json: as_json,
        } => match open(&project).await {
            Ok(store) => match rigel_core::retention::apply(
                &store,
                &project,
                &default_index_dir(),
                rigel_core::now_ms(),
                confirm_count,
                confirm_bytes,
            )
            .await
            {
                Ok(report) => {
                    print_report(&report, as_json);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&error),
            },
            Err(error) => fail(&error),
        },
    }
}

async fn open(project: &Path) -> Result<Store, rigel_core::StorageError> {
    Store::open_project(project).await
}

fn print_report(report: &rigel_core::retention::RetentionPreview, as_json: bool) {
    if as_json {
        crate::json::print(report);
    } else {
        println!("Retention preview for {}", report.project);
        println!("category    count       bytes");
        println!(
            "turns       {:>5} {:>11}",
            report.turns.count, report.turns.bytes
        );
        println!(
            "blobs       {:>5} {:>11}",
            report.blobs.count, report.blobs.bytes
        );
        println!(
            "logs        {:>5} {:>11}",
            report.logs.count, report.logs.bytes
        );
        println!(
            "snapshots   {:>5} {:>11}",
            report.snapshots.count, report.snapshots.bytes
        );
        println!(
            "total       {:>5} {:>11}",
            report.total_count, report.total_bytes
        );
        println!(
            "To apply, repeat --confirm-count {} --confirm-bytes {}.",
            report.total_count, report.total_bytes
        );
    }
}

fn fail(error: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}
