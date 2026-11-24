//! `rigel impact` — the change-impact graph before a merge or a test run (roadmap #26).
//!
//! This is the same plan `rigel test plan` builds, printed for review: the packages a change
//! reaches through Cargo path dependencies, the tests each package declares, and CODEOWNERS
//! owners. It never runs a test.

use std::path::PathBuf;
use std::process::ExitCode;

use rigel_core::testimpact::ImpactPlan;

pub fn run(
    project: PathBuf,
    since: String,
    changed_file: Vec<String>,
    full: bool,
    json: bool,
) -> ExitCode {
    let plan = match crate::testing::build_plan(&project, &since, &changed_file, full) {
        Ok(plan) => plan,
        Err(error) => {
            if json {
                println!("{}", serde_json::json!({ "ok": false, "error": error }));
            }
            eprintln!("rigel: {error}");
            return ExitCode::from(1);
        }
    };
    if json {
        match serde_json::to_string_pretty(&plan) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("rigel: could not encode JSON: {error}");
                return ExitCode::from(1);
            }
        }
    } else {
        print_impact(&plan);
    }
    ExitCode::SUCCESS
}

fn print_impact(plan: &ImpactPlan) {
    println!(
        "graph    {}: {} package(s), {} dependency edge(s)",
        plan.graph.source, plan.graph.packages, plan.graph.edges
    );
    if plan.changed.is_empty() {
        println!("changed  none");
    } else {
        println!("changed  {} file(s)", plan.changed.len());
        for file in &plan.changed {
            println!("  {} {}", file.status, file.path);
        }
    }
    if plan.symbols.changed.is_empty() {
        if plan.symbols.source != "unavailable"
            || !plan.symbols.unresolved.is_empty()
            || !plan.symbols.notes.is_empty()
        {
            println!("symbols  {}: no changed item resolved", plan.symbols.source);
            for path in &plan.symbols.unresolved {
                println!("symbol   unresolved: {path}");
            }
            for note in &plan.symbols.notes {
                println!("symbol   note: {note}");
            }
        }
    } else {
        println!(
            "symbols  {}: {} changed item(s), language {}",
            plan.symbols.source,
            plan.symbols.changed.len(),
            if plan.symbols.languages.is_empty() {
                "unknown".to_string()
            } else {
                plan.symbols.languages.join(", ")
            }
        );
        for symbol in plan.symbols.changed.iter().take(12) {
            println!(
                "symbol   {} {} ({}:{}, {})",
                symbol.kind, symbol.name, symbol.file, symbol.line, symbol.language
            );
        }
        for reach in &plan.symbols.reached {
            for reference in reach.references.iter().take(8) {
                println!(
                    "reach    {} via {}:{} ({}, {})",
                    reach.symbol, reference.path, reference.line, reference.kind, reference.package
                );
            }
        }
        for note in &plan.symbols.notes {
            println!("symbol   note: {note}");
        }
    }
    if plan.packages.is_empty() {
        println!("package  none");
    }
    for package in &plan.packages {
        let root = if package.root.is_empty() {
            "(root)"
        } else {
            package.root.as_str()
        };
        println!("package  {} ({}, {root})", package.name, package.role);
        if !package.changed.is_empty() {
            println!("  changed: {}", package.changed.join(", "));
        }
        println!("  selected by: {}", package.selected_by);
        for reference in package.references.iter().take(8) {
            println!(
                "  reach: {} via {}:{} ({})",
                reference.symbol, reference.path, reference.line, reference.kind
            );
        }
        if !package.tests.is_empty() {
            println!("  tests: {}", package.tests.join(", "));
        }
    }
    for owner in &plan.owners {
        println!("owners   {} {}", owner.path, owner.owners.join(" "));
    }
    for target in &plan.targets {
        println!("selected {}  ({})", target.label, target.reason);
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
