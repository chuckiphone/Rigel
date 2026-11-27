//! `rigel proof` — record and verify proof-carrying test evidence (roadmap #25).
//!
//! A bundle names the base commit, hashes every changed file and records the exact commands
//! that ran. `verify` re-fingerprints the working tree, so a review can tell fresh evidence
//! from a bundle that was correct before the tree moved.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::proof::{self, ProofBundle, ProofVerification};
use rigel_core::testrun::{self, TestPlan};

#[derive(Subcommand)]
pub enum ProofCommand {
    /// Run one command and record its evidence for the current change.
    Record {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long, default_value = "HEAD", value_name = "REV")]
        since: String,
        /// A note stored with the bundle; repeat for more than one.
        #[arg(long = "note", value_name = "TEXT")]
        notes: Vec<String>,
        #[arg(long)]
        json: bool,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },

    /// List recorded bundles, newest first.
    List {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Show one bundle by id or an unambiguous prefix.
    Show {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Verify the newest bundle (or one named by id) against the working tree.
    Verify {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// A bundle id or prefix. Defaults to the newest bundle.
        #[arg(long)]
        id: Option<String>,
        /// Verify every recorded bundle instead of the newest one.
        #[arg(long, conflicts_with = "id")]
        all: bool,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: ProofCommand) -> ExitCode {
    match command {
        ProofCommand::Record {
            project,
            since,
            notes,
            json,
            command,
        } => {
            let Some((program, args)) = command.split_first() else {
                eprintln!("rigel: a proof command is required after `--`");
                return ExitCode::from(1);
            };
            let plan = TestPlan {
                program: program.clone(),
                args: args.to_vec(),
                label: command.join(" "),
            };
            let run = match testrun::run_plan(&project, &plan).await {
                Ok(run) => run,
                Err(error) => {
                    if json {
                        crate::json::print(
                            serde_json::json!({ "ok": false, "error": error.to_string() }),
                        );
                    }
                    eprintln!("rigel: {error}");
                    return ExitCode::from(1);
                }
            };
            let evidence = proof::evidence_from_run(&run);
            let bundle = match proof::record(&project, &since, vec![evidence], notes) {
                Ok(bundle) => bundle,
                Err(error) => {
                    if json {
                        crate::json::print(
                            serde_json::json!({ "ok": false, "error": error.to_string() }),
                        );
                    }
                    eprintln!("rigel: {error}");
                    return ExitCode::from(2);
                }
            };
            if json {
                print_json(&bundle);
            } else {
                println!(
                    "proof    {} ({} evidence, {})",
                    bundle.id,
                    bundle.evidence.len(),
                    if bundle.ok { "passed" } else { "failed" }
                );
                println!(
                    "  {}",
                    proof::proofs_dir(&project)
                        .join(format!("{}.json", bundle.id))
                        .display()
                );
            }
            if run.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        ProofCommand::List { project, json } => match proof::list(&project) {
            Ok(bundles) => {
                if json {
                    print_json(&bundles);
                } else if bundles.is_empty() {
                    println!("proofs   none recorded");
                } else {
                    for bundle in bundles {
                        println!(
                            "proof    {}  {} file(s), {} evidence, {}  {}",
                            bundle.id,
                            bundle.files,
                            bundle.evidence,
                            if bundle.ok { "passed" } else { "failed" },
                            bundle.path.display()
                        );
                    }
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error.to_string(), json),
        },
        ProofCommand::Show { project, id, json } => match proof::load(&project, &id) {
            Ok(bundle) => {
                if json {
                    print_json(&bundle);
                } else {
                    print_bundle(&bundle, &project);
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error.to_string(), json),
        },
        ProofCommand::Verify {
            project,
            id,
            all,
            json,
        } => {
            let results = match &id {
                Some(id) => proof::verify(&project, id).map(|one| vec![one]),
                None => proof::verify_newest_or_all(&project, all),
            };
            match results {
                Ok(results) => {
                    if json {
                        print_json(&results);
                    } else {
                        for result in &results {
                            print_verification(result);
                        }
                    }
                    if results.iter().all(ProofVerification::verified) {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(2)
                    }
                }
                Err(error) => fail(&error.to_string(), json),
            }
        }
    }
}

fn print_bundle(bundle: &ProofBundle, project: &std::path::Path) {
    println!("proof    {}", bundle.id);
    println!("  base: {}", bundle.base);
    println!("  tree: {}", bundle.tree);
    println!(
        "  files: {}  evidence: {}  {}",
        bundle.files.len(),
        bundle.evidence.len(),
        if bundle.ok { "passed" } else { "failed" }
    );
    println!(
        "  {}",
        proof::proofs_dir(project)
            .join(format!("{}.json", bundle.id))
            .display()
    );
    for file in &bundle.files {
        println!("  {} {} {}", file.status, file.path, file.sha256);
    }
    for evidence in &bundle.evidence {
        println!(
            "  evidence: {} — {} passed, {} failed, exit {:?}",
            evidence.command, evidence.passed, evidence.failed, evidence.exit_code
        );
    }
    for note in &bundle.notes {
        println!("  note: {note}");
    }
}

fn print_verification(result: &ProofVerification) {
    println!(
        "proof    {}  {}",
        result.id,
        if result.verified() {
            "verified"
        } else {
            "not verified"
        }
    );
    println!(
        "  valid: {}  freshness: {}  outcome: {}  evidence: {}",
        result.valid,
        result.freshness,
        if result.ok { "passed" } else { "failed" },
        result.evidence
    );
    if !result.changed.is_empty() {
        println!("  changed since the proof: {}", result.changed.join(", "));
    }
    for reason in &result.reasons {
        println!("  reason: {reason}");
    }
}

fn print_json(value: &impl serde::Serialize) {
    crate::json::print_pretty(value);
}

fn fail(error: &str, as_json: bool) -> ExitCode {
    if as_json {
        crate::json::print(serde_json::json!({ "ok": false, "error": error }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}
