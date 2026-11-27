//! Run one native task across repositories under one conservative spend cap.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use rigel_core::SessionService;
use serde::Serialize;

#[derive(Args)]
pub struct BatchArgs {
    /// The task prompt sent to every repository.
    #[arg(long)]
    prompt: String,
    /// Maximum total spend, in cents, across this batch.
    #[arg(long, value_name = "CENTS")]
    cap_cents: i64,
    /// Use a deterministic scripted model instead of a provider.
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,
    /// Provider used for every repository. Defaults to the normal agent provider.
    #[arg(long)]
    provider: Option<String>,
    /// Model used for every repository.
    #[arg(long)]
    model: Option<String>,
    /// Maximum model steps per repository.
    #[arg(long, default_value_t = rigel_core::DEFAULT_MAX_STEPS)]
    max_steps: usize,
    /// Print one JSON report per repository.
    #[arg(long)]
    json: bool,
    /// Repositories to process, in deterministic input order.
    #[arg(required = true, value_name = "REPOSITORY")]
    repositories: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct RepositoryReport {
    repository: String,
    session_id: String,
    status: String,
    exit_code: u8,
    spend_micros: i64,
    spend_cents: i64,
    session_cap_cents: i64,
}

#[derive(Debug, Serialize)]
struct BatchStop {
    reason: String,
    remaining_repositories: Vec<String>,
    spent_cents: i64,
    cap_cents: i64,
}

pub async fn run(args: BatchArgs, data_dir: PathBuf) -> ExitCode {
    if args.cap_cents <= 0 {
        eprintln!("rigel: --cap-cents must be greater than zero");
        return ExitCode::from(2);
    }
    if args.prompt.trim().is_empty() {
        eprintln!("rigel: --prompt must not be empty");
        return ExitCode::from(2);
    }

    let repositories = match canonical_repositories(&args.repositories) {
        Ok(repositories) => repositories,
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };
    let mut spent_cents = 0_i64;
    let mut failed = false;

    for (index, repository) in repositories.iter().enumerate() {
        let remaining_cents = args.cap_cents.saturating_sub(spent_cents);
        if remaining_cents <= 0 {
            emit_stop(
                &BatchStop {
                    reason: "spend cap reached".to_string(),
                    remaining_repositories: repositories[index..]
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                    spent_cents,
                    cap_cents: args.cap_cents,
                },
                args.json,
            );
            break;
        }

        let service = match SessionService::open(repository).await {
            Ok(service) => service,
            Err(error) => {
                failed = true;
                eprintln!("rigel: {}: {error}", repository.display());
                continue;
            }
        };
        let session = match service
            .start(Some("batch run"), Some(remaining_cents))
            .await
        {
            Ok(session) => session,
            Err(error) => {
                failed = true;
                eprintln!("rigel: {}: {error}", repository.display());
                continue;
            }
        };

        let exit = crate::phase3::run_agent(
            &data_dir,
            crate::phase3::AgentCommand::Run {
                project: repository.clone(),
                session: Some(session.id.clone()),
                agent: "native".to_string(),
                prompt: args.prompt.clone(),
                image: Vec::new(),
                dry_run: false,
                script: args.script.clone(),
                provider: args.provider.clone(),
                model: args.model.clone(),
                effort: None,
                approval: None,
                crash_after_journal: false,
                max_steps: args.max_steps,
                no_network: false,
                allow_origin: Vec::new(),
                // Keep the existing CLI's interactive approval behavior. JSON reports below
                // are emitted separately, after the runner has finished this repository.
                json: false,
            },
        )
        .await;
        let after = match service.get(&session.id).await {
            Ok(session) => session,
            Err(error) => {
                failed = true;
                eprintln!(
                    "rigel: {}: could not read batch spend: {error}",
                    repository.display()
                );
                continue;
            }
        };
        spent_cents = spent_cents.saturating_add(after.spend_cents.max(0));
        let exit_code = if exit == ExitCode::SUCCESS {
            0
        } else if exit == ExitCode::from(3) {
            3
        } else {
            1
        };
        let status = match exit_code {
            0 => "completed",
            3 => "budget_exhausted",
            _ => "failed",
        };
        if exit_code != 0 && exit_code != 3 {
            failed = true;
        }
        emit_report(
            &RepositoryReport {
                repository: repository.display().to_string(),
                session_id: session.id,
                status: status.to_string(),
                exit_code,
                spend_micros: after.spend_micros,
                spend_cents: after.spend_cents,
                session_cap_cents: remaining_cents,
            },
            args.json,
        );

        if spent_cents >= args.cap_cents {
            let next = index + 1;
            if next < repositories.len() {
                emit_stop(
                    &BatchStop {
                        reason: "spend cap reached".to_string(),
                        remaining_repositories: repositories[next..]
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect(),
                        spent_cents,
                        cap_cents: args.cap_cents,
                    },
                    args.json,
                );
            }
            break;
        }
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn canonical_repositories(repositories: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut canonical = Vec::with_capacity(repositories.len());
    for repository in repositories {
        let path = std::fs::canonicalize(repository)
            .map_err(|error| format!("{}: {error}", repository.display()))?;
        if !path.is_dir() {
            return Err(format!("{} is not a repository directory", path.display()));
        }
        canonical.push(path);
    }
    Ok(canonical)
}

fn emit_report(report: &RepositoryReport, json: bool) {
    if json {
        crate::json::print(serde_json::json!({"batch_report": report}));
    } else {
        println!(
            "\nbatch report: {} status={} spend=${:.6} cap=${:.2} session={}",
            report.repository,
            report.status,
            report.spend_micros as f64 / 1_000_000.0,
            report.session_cap_cents as f64 / 100.0,
            report.session_id
        );
    }
}

fn emit_stop(stop: &BatchStop, json: bool) {
    if json {
        crate::json::print(serde_json::json!({"batch_stop": stop}));
    } else {
        println!(
            "\nbatch stopped: {} after ${:.2} of ${:.2}",
            stop.reason,
            stop.spent_cents as f64 / 100.0,
            stop.cap_cents as f64 / 100.0
        );
        if !stop.remaining_repositories.is_empty() {
            println!("remaining repositories (input order):");
            for repository in &stop.remaining_repositories {
                println!("  {repository}");
            }
        }
    }
}
