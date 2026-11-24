//! `rigel test` — plan the tests a change can affect, then run them with a full-suite safety
//! net (roadmap #31). The selection itself is pure core logic; this module is the command
//! surface and the human/JSON receipt.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::flakes;
use rigel_core::proof;
use rigel_core::testimpact::{self, ChangedFile, ImpactPlan, ImpactRun};
use rigel_core::testrun::{self, TestPlan, TestRun};

#[derive(Subcommand)]
pub enum TestCommand {
    /// Show the impacted targets and every skipped change, without running anything.
    Plan {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// The git revision to compare with. Defaults to HEAD (tracked and untracked changes).
        #[arg(long, default_value = "HEAD", value_name = "REV")]
        since: String,
        /// Use this changed path instead of asking git; repeat for more than one.
        #[arg(long, value_name = "PATH")]
        changed_file: Vec<String>,
        /// Ask for the full suite even when the change is small and mapped.
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },

    /// Run impacted targets in order, then the full suite when the plan says it is needed.
    Run {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long, default_value = "HEAD", value_name = "REV")]
        since: String,
        #[arg(long, value_name = "PATH")]
        changed_file: Vec<String>,
        #[arg(long)]
        full: bool,
        /// Record proof-carrying evidence for the runs in `.rigel/proofs/`.
        #[arg(long)]
        proof: bool,
        #[arg(long)]
        json: bool,
    },

    /// Run one target several times and report tests whose outcome changed.
    Flakes {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// How many identical runs to compare (2–10).
        #[arg(long, default_value_t = flakes::DEFAULT_RUNS, value_name = "N")]
        runs: usize,
        #[arg(long)]
        json: bool,
        /// The command after `--`; defaults to the project's detected suite.
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
}

pub async fn run(command: TestCommand) -> ExitCode {
    match command {
        TestCommand::Plan {
            project,
            since,
            changed_file,
            full,
            json,
        } => match build_plan(&project, &since, &changed_file, full) {
            Ok(plan) => {
                if json {
                    print_json(&plan);
                } else {
                    print_plan(&plan);
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error, json),
        },
        TestCommand::Run {
            project,
            since,
            changed_file,
            full,
            proof,
            json,
        } => {
            let plan = match build_plan(&project, &since, &changed_file, full) {
                Ok(plan) => plan,
                Err(error) => return fail(&error, json),
            };
            if !json {
                print_plan(&plan);
            }
            if plan.targets.is_empty() && plan.full.is_empty() {
                if plan.changed.is_empty() || plan.risk.is_empty() {
                    let mut report = ImpactRun {
                        plan,
                        runs: Vec::new(),
                        full_runs: Vec::new(),
                        ok: true,
                        stopped: false,
                        proof: None,
                    };
                    if proof && let Err(error) = attach_proof(&project, &since, &mut report) {
                        return fail(&error, json);
                    }
                    if json {
                        print_json(&report);
                    } else {
                        println!("no tests selected: nothing to run");
                        if let Some(bundle) = &report.proof {
                            println!(
                                "proof    {} ({} evidence, {})",
                                bundle.id,
                                bundle.evidence.len(),
                                if bundle.ok { "passed" } else { "failed" }
                            );
                        }
                    }
                    return ExitCode::SUCCESS;
                }
                return fail(
                    "no test target was selected and no full suite is available",
                    json,
                );
            }
            match testimpact::run(&project, plan).await {
                Ok(mut report) => {
                    if proof && let Err(error) = attach_proof(&project, &since, &mut report) {
                        return fail(&error, json);
                    }
                    if json {
                        print_json(&report);
                    } else {
                        print_runs(&report);
                    }
                    if report.ok {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(error) => fail(&error.to_string(), json),
            }
        }
        TestCommand::Flakes {
            project,
            runs,
            json,
            command,
        } => {
            let plan = if let Some((program, args)) = command.split_first() {
                TestPlan {
                    program: program.clone(),
                    args: args.to_vec(),
                    label: command.join(" "),
                }
            } else {
                match testrun::detect(&project) {
                    Some(plan) => plan,
                    None => {
                        return fail(
                            "no test command found; pass one after `--` or add a manifest",
                            json,
                        );
                    }
                }
            };
            match flakes::hunt(&project, &plan, runs).await {
                Ok(report) => {
                    if json {
                        print_json(&report);
                    } else {
                        print_flakes(&report);
                    }
                    if report.failed() {
                        ExitCode::from(1)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(error) => fail(&error.to_string(), json),
            }
        }
    }
}

pub(crate) fn build_plan(
    project: &Path,
    since: &str,
    explicit: &[String],
    full: bool,
) -> Result<ImpactPlan, String> {
    if !project.is_dir() {
        return Err(format!(
            "the project folder is not available: {}",
            project.display()
        ));
    }
    let changed = if explicit.is_empty() {
        testimpact::changed_files(project, since).map_err(|error| error.to_string())?
    } else {
        explicit
            .iter()
            .map(|path| ChangedFile {
                path: normalize(project, path),
                status: "explicit".into(),
            })
            .collect()
    };
    testimpact::plan_with_since(project, since, &changed, full).map_err(|error| error.to_string())
}

fn normalize(project: &Path, path: &str) -> String {
    let path = Path::new(path);
    let relative = path.strip_prefix(project).unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");
    text.strip_prefix("./").unwrap_or(&text).to_string()
}

fn print_plan(plan: &ImpactPlan) {
    if plan.changed.is_empty() {
        println!("changed  none");
    } else {
        println!("changed  {} file(s)", plan.changed.len());
        for file in &plan.changed {
            println!("  {} {}", file.status, file.path);
        }
        if !plan.symbols.changed.is_empty() {
            println!(
                "symbols  {}: {} changed item(s), {} reference(s), reach depth {}/{}, {} declaration(s), {} cycle/revisit skip(s)",
                plan.symbols.source,
                plan.symbols.changed.len(),
                plan.symbols
                    .reached
                    .iter()
                    .map(|reach| reach.references.len())
                    .sum::<usize>(),
                plan.symbols.max_depth,
                plan.symbols.depth_limit,
                plan.symbols.reached_symbols,
                plan.symbols.cycles,
            );
        }
    }
    for target in &plan.targets {
        println!("selected {}", target.label);
        println!(
            "  {}  {}",
            target.reason,
            if target.paths.is_empty() {
                String::new()
            } else {
                target.paths.join(", ")
            }
            .trim_end()
        );
    }
    for target in &plan.full {
        println!("full     {}  ({})", target.label, target.reason);
    }
    if plan.targets.is_empty() && plan.full.is_empty() {
        println!("tests    none selected");
    }
    for skipped in &plan.skipped {
        println!("skipped  {} — {}", skipped.path, skipped.reason);
    }
    for risk in &plan.risk {
        println!("risk     {risk}");
    }
}

fn print_runs(report: &ImpactRun) {
    for run in &report.runs {
        print_run("selected", run);
    }
    for run in &report.full_runs {
        print_run("full", run);
    }
    if report.stopped {
        println!("stopped  a target failed; later targets were not run");
    } else if report.ok {
        println!("tests    passed");
    }
    if let Some(bundle) = &report.proof {
        println!(
            "proof    {} ({} evidence, {})",
            bundle.id,
            bundle.evidence.len(),
            if bundle.ok { "passed" } else { "failed" }
        );
    }
}

fn print_flakes(report: &flakes::FlakeReport) {
    println!("command  {}", report.command);
    for (index, run) in report.runs.iter().enumerate() {
        println!(
            "run {:<3} {:<6} {} passed, {} failed, {} ms{}",
            index + 1,
            if run.ok { "ok" } else { "failed" },
            run.passed,
            run.failed,
            run.duration_ms,
            if run.timed_out { " (timed out)" } else { "" }
        );
    }
    for test in &report.flaky {
        if let Some(row) = report.tests.iter().find(|row| &row.name == test) {
            println!(
                "flaky    {} ({} passed, {} failed across {} run(s)) — propose quarantine",
                test, row.passed, row.failed, row.runs
            );
        } else {
            println!("flaky    {test} — propose quarantine");
        }
    }
    for test in &report.unstable_set {
        println!("unstable {test} — did not appear in every run");
    }
    if report.aggregate_flaky {
        println!("flaky    run outcomes differed; no per-test result named the test");
    }
    if report.stable() {
        println!(
            "tests    stable across {} run(s){}",
            report.total_runs,
            if report.all_ok {
                ""
            } else {
                "; the suite failed consistently"
            }
        );
    }
    for note in &report.notes {
        println!("note     {note}");
    }
}

fn attach_proof(project: &Path, since: &str, report: &mut ImpactRun) -> Result<(), String> {
    let evidence: Vec<proof::Evidence> = report
        .runs
        .iter()
        .chain(report.full_runs.iter())
        .map(proof::evidence_from_run)
        .collect();
    let bundle = proof::record(
        project,
        since,
        evidence,
        vec!["recorded by rigel test run --proof".to_string()],
    )
    .map_err(|error| error.to_string())?;
    report.proof = Some(bundle);
    Ok(())
}

fn print_run(kind: &str, run: &TestRun) {
    let status = if run.ok { "ok" } else { "failed" };
    println!(
        "{kind:<8} {status:<6} {} passed, {} failed, {} ms",
        run.passed, run.failed, run.duration_ms
    );
    if !run.ok {
        print!("{}", run.output);
        if !run.output.ends_with('\n') {
            println!();
        }
    }
}

fn print_json(value: &impl serde::Serialize) {
    match serde_json::to_string(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("rigel: could not encode JSON: {error}"),
    }
}

fn fail(error: &str, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", serde_json::json!({ "ok": false, "error": error }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(1)
}
