//! Roadmap #31: run the tests a change can plausibly affect, then the full suite when the
//! selection is not trustworthy.
//!
//! The selector is deliberately a map, not a proof. It reads the changed paths from Git, finds
//! the Cargo package or JavaScript package that owns each one, and names a package-level target.
//! A changed integration test names that exact Cargo test target. Anything it cannot attribute,
//! any manifest or migration change, too many files or packages, and a forced request makes the
//! full suite follow the selected runs. Every file that produces no target is reported with the
//! reason, so a fast run never silently stands in for a complete one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use thiserror::Error;

use crate::process;
use crate::symbolimpact::{self, PackageScope, SymbolImpact};
use crate::testrun::{self, TestPlan, TestRun, TestRunError};
use crate::tree;

/// A change larger than this is no longer a focused edit; let the full suite decide.
pub const MAX_CHANGED_FILES: usize = 6;
/// Touching more Cargo packages than this is a broad change, not an impacted edit.
pub const MAX_PACKAGES: usize = 2;
/// A dependency closure larger than this is slower than the full suite it would replace.
pub const MAX_AFFECTED_PACKAGES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangedFile {
    pub path: String,
    /// Git's status letter, or `?` for an untracked file.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestTarget {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    /// Why this target is in the plan.
    pub reason: String,
    /// Project-relative files this target covers.
    pub paths: Vec<String>,
}

impl TestTarget {
    fn from_plan(plan: TestPlan, reason: String, paths: Vec<String>) -> Self {
        Self {
            label: plan.label,
            program: plan.program,
            args: plan.args,
            reason,
            paths,
        }
    }

    fn plan(&self) -> TestPlan {
        TestPlan {
            program: self.program.clone(),
            args: self.args.clone(),
            label: self.label.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedChange {
    pub path: String,
    pub reason: String,
}

/// One package touched by the change, either directly or through a Cargo path dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactedPackage {
    pub name: String,
    /// Project-relative package root; empty for a single-package repository.
    pub root: String,
    /// `changed` or `dependent`.
    pub role: String,
    /// Directly changed files in this package; empty for a dependent.
    pub changed: Vec<String>,
    /// Integration-test target names the package declares.
    pub tests: Vec<String>,
    /// Why the package is in the plan: changed items, symbol references, or only the package
    /// dependency graph.
    pub selected_by: String,
    /// The concrete files and changed symbols that justify this package.
    pub references: Vec<PackageReference>,
}

/// One file-level reason a package was selected. `kind` is `changed`, `test`, `import` or
/// `source`; a changed row has the file that contains the item, not a use site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageReference {
    pub symbol: String,
    pub path: String,
    pub line: usize,
    pub kind: String,
}

/// The CODEOWNERS owners for one changed path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerRow {
    pub path: String,
    pub owners: Vec<String>,
}

/// Which graph the selection used, so a receipt says whether it was compiler-backed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphSummary {
    /// `cargo metadata` or `manifest scan`.
    pub source: String,
    pub packages: usize,
    pub edges: usize,
}

impl Default for GraphSummary {
    fn default() -> Self {
        Self {
            source: "none".into(),
            packages: 0,
            edges: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactPlan {
    pub changed: Vec<ChangedFile>,
    pub targets: Vec<TestTarget>,
    /// The full suite to run after the selected targets, only when the selection is risky.
    pub full: Vec<TestTarget>,
    pub skipped: Vec<SkippedChange>,
    /// Why the full suite is required; empty when selected targets are the whole run.
    pub risk: Vec<String>,
    /// The packages the change reaches, directly and through the Cargo dependency graph.
    pub packages: Vec<ImpactedPackage>,
    /// CODEOWNERS owners for the changed paths that have a rule.
    pub owners: Vec<OwnerRow>,
    pub graph: GraphSummary,
    /// The changed items and their scanned references. Empty when symbol resolution was not
    /// attempted; package selection still applies.
    pub symbols: SymbolImpact,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactRun {
    pub plan: ImpactPlan,
    pub runs: Vec<TestRun>,
    pub full_runs: Vec<TestRun>,
    pub ok: bool,
    /// A selected or full target failed, so nothing after it ran.
    pub stopped: bool,
    /// Proof-carrying evidence, when `rigel test run --proof` wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof: Option<crate::proof::ProofBundle>,
}

#[derive(Debug, Error)]
pub enum ImpactError {
    #[error("the project folder is not available: {0}")]
    MissingProject(PathBuf),
    #[error("git could not list changed files: {0}")]
    Git(String),
    #[error("the project tree could not be read: {0}")]
    Tree(String),
    #[error("the test runner failed: {0}")]
    Test(#[from] TestRunError),
}

/// List tracked and untracked changes relative to `since` (`HEAD` by default). Renames report
/// their new path; deleted files stay in the list because their package still owns the tests.
pub fn changed_files(root: &Path, since: &str) -> Result<Vec<ChangedFile>, ImpactError> {
    if !root.is_dir() {
        return Err(ImpactError::MissingProject(root.to_path_buf()));
    }
    let since = if since.trim().is_empty() {
        "HEAD"
    } else {
        since.trim()
    };
    let tracked = git(root, &["diff", "--name-status", "-z", since, "--"])?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut changes: BTreeMap<String, ChangedFile> = BTreeMap::new();
    for change in parse_name_status(&tracked) {
        changes.insert(change.path.clone(), change);
    }
    for path in zero_separated(&untracked) {
        changes.entry(path.clone()).or_insert(ChangedFile {
            path,
            status: "?".into(),
        });
    }
    // `.rigel/` is Rigel's own state (sessions, proofs, audit rows), never a project change.
    Ok(changes
        .into_values()
        .filter(|change| change.path != ".rigel" && !change.path.starts_with(".rigel/"))
        .collect())
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, ImpactError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    process::hide(&mut command);
    let output = command
        .output()
        .map_err(|error| ImpactError::Git(format!("git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ImpactError::Git(format!(
            "git {}: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(output.stdout)
}

fn zero_separated(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).replace('\\', "/"))
        .collect()
}

fn parse_name_status(bytes: &[u8]) -> Vec<ChangedFile> {
    let fields: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).replace('\\', "/"))
        .collect();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = fields[index].clone();
        if status.starts_with('R') || status.starts_with('C') {
            // `R100\0old\0new\0`: the new path is the one a package owns now.
            if index + 2 < fields.len() {
                changes.push(ChangedFile {
                    path: fields[index + 2].clone(),
                    status,
                });
            }
            index += 3;
        } else if index + 1 < fields.len() {
            changes.push(ChangedFile {
                path: fields[index + 1].clone(),
                status,
            });
            index += 2;
        } else {
            break;
        }
    }
    changes
}

/// Build the impacted-test plan from an explicit change list. This is pure: no command runs,
/// which is what `test plan` prints and what tests can pin without a real suite.
pub fn plan(
    root: &Path,
    changed: &[ChangedFile],
    force_full: bool,
) -> Result<ImpactPlan, ImpactError> {
    plan_with_since(root, "HEAD", changed, force_full)
}

/// Like [`plan`], but resolves changed items against `since` so a symbol change can be read from
/// the real Git hunk rather than inferred from the file path.
pub fn plan_with_since(
    root: &Path,
    since: &str,
    changed: &[ChangedFile],
    force_full: bool,
) -> Result<ImpactPlan, ImpactError> {
    let files = tree::project_files(root).map_err(|error| ImpactError::Tree(error.to_string()))?;
    let (cargo, graph) = discover_cargo_packages(root, &files);
    let node = discover_node_packages(root, &files);

    let mut targets: BTreeMap<String, TestTarget> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut risk = BTreeSet::new();
    let mut packages: BTreeSet<String> = BTreeSet::new();
    // A package whose whole suite must run. A changed integration test adds its exact target
    // instead; a source change inside the same package still needs the package target.
    let mut package_needed: BTreeSet<String> = BTreeSet::new();
    let mut changed_by_package: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut changed_sources = 0usize;
    let mut needs_full_root = false;
    let mut full: Vec<TestTarget> = Vec::new();

    for file in changed {
        let path = file.path.as_str();
        if is_generated(path) {
            skipped.push(SkippedChange {
                path: path.to_string(),
                reason: "generated or vendored path; not selected".into(),
            });
            if is_source(path) {
                risk.insert("source changed in a generated or vendored tree".into());
                needs_full_root = true;
            }
            continue;
        }

        if is_cargo_manifest(path) {
            risk.insert(format!("{path} is a build manifest"));
            needs_full_root = true;
            if let Some(package) = cargo_package_for(path, &cargo) {
                packages.insert(package.name.clone());
                package_needed.insert(package.name.clone());
                changed_by_package
                    .entry(package.name.clone())
                    .or_default()
                    .push(path.to_string());
            }
            continue;
        }
        if is_lockfile(path) {
            risk.insert(format!("{path} is a dependency lockfile"));
            needs_full_root = true;
            continue;
        }
        if is_package_manifest(path) {
            risk.insert(format!("{path} is a package manifest"));
            match node_package_for(path, &node) {
                Some(package) => {
                    selected_or_full(&mut targets, &mut full, node_target(package, None))
                }
                None => skipped.push(SkippedChange {
                    path: path.to_string(),
                    reason: "package manifest has no test script".into(),
                }),
            }
            continue;
        }
        if is_project_config(path) {
            risk.insert(format!("{path} can change how the suite is run"));
            needs_full_root = true;
            continue;
        }

        if path.ends_with(".rs") {
            let Some(package) = cargo_package_for(path, &cargo) else {
                skipped.push(SkippedChange {
                    path: path.to_string(),
                    reason: "Rust file is not inside a Cargo package".into(),
                });
                risk.insert("unmapped Rust change".into());
                needs_full_root = true;
                continue;
            };
            changed_sources += 1;
            packages.insert(package.name.clone());
            changed_by_package
                .entry(package.name.clone())
                .or_default()
                .push(path.to_string());
            if let Some((test, relative)) = rust_test_target(path, package) {
                let mut target = package_target(
                    package,
                    &format!("changed test {relative}"),
                    vec![path.to_string()],
                );
                target.args.extend(["--test".to_string(), test]);
                target.label = label(&target.program, &target.args);
                add_target(&mut targets, target);
            } else {
                package_needed.insert(package.name.clone());
            }
            continue;
        }

        if is_node_source(path) {
            if let Some(package) = node_package_for(path, &node) {
                changed_sources += 1;
                let target = if is_node_test(path) {
                    node_target(package, Some(relative_to(path, &package.root)))
                } else {
                    node_target(package, None)
                };
                add_target(&mut targets, target);
            } else {
                skipped.push(SkippedChange {
                    path: path.to_string(),
                    reason: "JavaScript/TypeScript file is not in a package with a test script"
                        .into(),
                });
                risk.insert("unmapped JavaScript/TypeScript change".into());
                needs_full_root = true;
            }
            continue;
        }

        if is_source(path) {
            skipped.push(SkippedChange {
                path: path.to_string(),
                reason: "no test mapping for this source language yet".into(),
            });
            risk.insert(format!("unmapped source change: {path}"));
            needs_full_root = true;
            continue;
        }

        skipped.push(SkippedChange {
            path: path.to_string(),
            reason: "documentation or data change; no test target".into(),
        });
    }

    if changed_sources > MAX_CHANGED_FILES {
        risk.insert(format!(
            "{changed_sources} source files changed (limit {MAX_CHANGED_FILES})"
        ));
        needs_full_root = true;
    }
    if packages.len() > MAX_PACKAGES {
        risk.insert(format!(
            "{} Cargo packages changed (limit {MAX_PACKAGES})",
            packages.len()
        ));
        needs_full_root = true;
    }
    if force_full {
        risk.insert("full suite requested".into());
        needs_full_root = true;
    }

    // The compiler-backed graph turns a changed package into the packages that depend on it.
    // Those packages' tests exercise the changed code, so they are selected rather than left
    // to the full-suite fallback.
    let affected = affected_closure(&cargo, &packages);
    if affected.len() > MAX_AFFECTED_PACKAGES {
        risk.insert(format!(
            "{} packages are in the dependency closure (limit {MAX_AFFECTED_PACKAGES})",
            affected.len()
        ));
        needs_full_root = true;
    }

    // Symbol resolution is additive evidence on top of the package graph. It never replaces a
    // package target: the package answer remains the floor when a name or language is ambiguous.
    let scopes = symbol_scopes(&cargo, &node);
    let priority_roots = symbol_priority_roots(&affected, &cargo, &node, changed);
    let symbols = symbolimpact::analyze(root, since, changed, &scopes, &files, &priority_roots);
    let known_packages: BTreeSet<String> = scopes.iter().map(|scope| scope.name.clone()).collect();
    let reached_packages: BTreeSet<String> = symbols
        .reached
        .iter()
        .flat_map(|reach| reach.references.iter())
        .filter(|reference| known_packages.contains(&reference.package))
        .map(|reference| reference.package.clone())
        .collect();
    let total_packages: BTreeSet<String> = affected
        .iter()
        .cloned()
        .chain(reached_packages.iter().cloned())
        .collect();
    if total_packages.len() > MAX_AFFECTED_PACKAGES {
        risk.insert(format!(
            "{} packages are reached by the package graph or symbol references (limit {MAX_AFFECTED_PACKAGES})",
            total_packages.len()
        ));
        needs_full_root = true;
    }
    for name in &affected {
        let Some(package) = cargo.iter().find(|package| &package.name == name) else {
            continue;
        };
        let direct = packages.contains(name);
        if direct && !package_needed.contains(name) {
            // A changed integration test already named the exact target.
            continue;
        }
        let reason = if direct {
            format!("changed package {name}")
        } else {
            let via: Vec<String> = packages
                .iter()
                .filter(|changed| depends_on(&cargo, name, changed))
                .cloned()
                .collect();
            if via.is_empty() {
                "depends on a changed package".to_string()
            } else {
                format!("depends on changed package {}", via.join(", "))
            }
        };
        add_target(
            &mut targets,
            package_target(
                package,
                &reason,
                changed_by_package.get(name).cloned().unwrap_or_default(),
            ),
        );
    }

    // A test file that names a changed symbol gets an exact target. The package target above
    // stays too, so symbol selection can add precision but never remove the package-level floor.
    for reach in &symbols.reached {
        for reference in &reach.references {
            if let Some(target) = symbol_test_target(&reach.symbol, reference, &cargo, &node) {
                add_target(&mut targets, target);
            }
        }
    }

    if needs_full_root && let Some(target) = full_root_target(root) {
        full.push(target);
    }
    if force_full {
        for package in &node {
            add_target(&mut targets, node_target(package, None));
        }
    }
    dedupe_targets(&mut full);
    full.retain(|target| !targets.keys().any(|label| label == &target.label));

    let references_by_package = package_references(&symbols, &cargo);
    let impacted: Vec<ImpactedPackage> = affected
        .iter()
        .filter_map(|name| {
            let package = cargo.iter().find(|package| &package.name == name)?;
            let references = references_by_package.get(name).cloned().unwrap_or_default();
            let has_symbol_reference = references.iter().any(|row| row.kind != "changed");
            let selected_by = match (packages.contains(name), has_symbol_reference) {
                (true, true) => "changed symbols and package dependency".to_string(),
                (true, false) => "changed file".to_string(),
                (false, true) => "package dependency and symbol references".to_string(),
                (false, false) => "package dependency".to_string(),
            };
            Some(ImpactedPackage {
                name: package.name.clone(),
                root: package.root.clone(),
                role: if packages.contains(name) {
                    "changed".into()
                } else {
                    "dependent".into()
                },
                changed: changed_by_package.get(name).cloned().unwrap_or_default(),
                tests: package.tests.clone(),
                selected_by,
                references,
            })
        })
        .collect();
    let owners = owner_rows(root, changed);

    Ok(ImpactPlan {
        changed: changed.to_vec(),
        targets: targets.into_values().collect(),
        full,
        skipped,
        risk: risk.into_iter().collect(),
        packages: impacted,
        owners,
        graph,
        symbols,
    })
}

fn selected_or_full(
    selected: &mut BTreeMap<String, TestTarget>,
    full: &mut Vec<TestTarget>,
    target: TestTarget,
) {
    if selected.contains_key(&target.label) {
        return;
    }
    if full.iter().any(|existing| existing.label == target.label) {
        return;
    }
    full.push(target);
}

fn add_target(targets: &mut BTreeMap<String, TestTarget>, target: TestTarget) {
    if let Some(existing) = targets.get_mut(&target.label) {
        for path in target.paths {
            if !existing.paths.contains(&path) {
                existing.paths.push(path);
            }
        }
        return;
    }
    targets.insert(target.label.clone(), target);
}

fn dedupe_targets(targets: &mut Vec<TestTarget>) {
    let mut seen = BTreeSet::new();
    targets.retain(|target| seen.insert(target.label.clone()));
}

fn full_root_target(root: &Path) -> Option<TestTarget> {
    testrun::detect(root).map(|plan| {
        TestTarget::from_plan(plan, "full suite after an impacted run".into(), Vec::new())
    })
}

fn package_target(package: &CargoPackage, reason: &str, paths: Vec<String>) -> TestTarget {
    let mut args = vec!["test".to_string()];
    if !package.root.is_empty() {
        args.push("-p".into());
        args.push(package.name.clone());
    }
    let label = label("cargo", &args);
    TestTarget {
        label,
        program: "cargo".into(),
        args,
        reason: reason.into(),
        paths,
    }
}

fn rust_test_target(path: &str, package: &CargoPackage) -> Option<(String, String)> {
    let relative = relative_to(path, &package.root);
    let mut parts = relative.split('/');
    if parts.next()? != "tests" {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        // Cargo only recognizes test files directly under `tests/`; a nested file belongs to
        // the package suite, not its own target.
        return None;
    }
    file.strip_suffix(".rs")
        .map(|test| (test.to_string(), relative.clone()))
}

fn symbol_scopes(cargo: &[CargoPackage], node: &[NodePackage]) -> Vec<PackageScope> {
    cargo
        .iter()
        .map(|package| PackageScope {
            name: package.name.clone(),
            root: package.root.clone(),
        })
        .chain(node.iter().map(|package| PackageScope {
            name: package.name.clone(),
            root: package.root.clone(),
        }))
        .collect()
}

fn symbol_priority_roots(
    affected: &BTreeSet<String>,
    cargo: &[CargoPackage],
    node: &[NodePackage],
    changed: &[ChangedFile],
) -> Vec<String> {
    let mut roots = BTreeSet::new();
    for name in affected {
        if let Some(package) = cargo.iter().find(|package| &package.name == name) {
            roots.insert(package.root.clone());
        }
    }
    for change in changed {
        if let Some(package) = node_package_for(&change.path, node) {
            roots.insert(package.root.clone());
        }
    }
    roots.into_iter().collect()
}

/// Add a precise test target only when the reference sits in a test file and the runner has a
/// target smaller than the package suite. This is deliberately additive; the package target stays.
fn symbol_test_target(
    symbol: &str,
    reference: &symbolimpact::SymbolReference,
    cargo: &[CargoPackage],
    node: &[NodePackage],
) -> Option<TestTarget> {
    if reference.kind != "test" {
        return None;
    }
    let path = reference.path.as_str();
    if path.ends_with(".rs") {
        let package = cargo_package_for(path, cargo)?;
        let (test, relative) = rust_test_target(path, package)?;
        let mut target = package_target(
            package,
            &format!("symbol {symbol} is used by {relative}"),
            vec![path.to_string()],
        );
        target.args.extend(["--test".to_string(), test]);
        target.label = label(&target.program, &target.args);
        return Some(target);
    }
    if is_node_source(path) && is_node_test(path) {
        let package = node_package_for(path, node)?;
        let mut target = node_target(package, Some(relative_to(path, &package.root)));
        target.reason = format!("symbol {symbol} is used by {path}");
        return Some(target);
    }
    if path.ends_with("_test.go") {
        let dir = Path::new(path)
            .parent()
            .map(|dir| dir.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let args = if dir.is_empty() {
            vec!["test".to_string(), "./...".to_string()]
        } else {
            vec!["test".to_string(), format!("./{dir}")]
        };
        return Some(TestTarget {
            label: label("go", &args),
            program: "go".into(),
            args,
            reason: format!("symbol {symbol} is used by {path}"),
            paths: vec![path.to_string()],
        });
    }
    if path.ends_with(".py") {
        let args = vec!["-m".to_string(), "pytest".to_string(), path.to_string()];
        return Some(TestTarget {
            label: label("python", &args),
            program: "python".into(),
            args,
            reason: format!("symbol {symbol} is used by {path}"),
            paths: vec![path.to_string()],
        });
    }
    None
}

fn package_references(
    symbols: &SymbolImpact,
    cargo: &[CargoPackage],
) -> BTreeMap<String, Vec<PackageReference>> {
    let mut by_package: BTreeMap<String, Vec<PackageReference>> = BTreeMap::new();
    for changed in &symbols.changed {
        let Some(package) = cargo_package_for(&changed.file, cargo) else {
            continue;
        };
        by_package
            .entry(package.name.clone())
            .or_default()
            .push(PackageReference {
                symbol: changed.name.clone(),
                path: changed.file.clone(),
                line: changed.line,
                kind: "changed".into(),
            });
    }
    for reach in &symbols.reached {
        for reference in &reach.references {
            let Some(package) = cargo_package_for(&reference.path, cargo) else {
                continue;
            };
            by_package
                .entry(package.name.clone())
                .or_default()
                .push(PackageReference {
                    symbol: reach.symbol.clone(),
                    path: reference.path.clone(),
                    line: reference.line,
                    kind: reference.kind.clone(),
                });
        }
    }
    for rows in by_package.values_mut() {
        rows.sort_by(|left, right| {
            (&left.symbol, &left.path, left.line, &left.kind).cmp(&(
                &right.symbol,
                &right.path,
                right.line,
                &right.kind,
            ))
        });
        rows.dedup();
    }
    by_package
}

fn relative_to(path: &str, root: &str) -> String {
    if root.is_empty() {
        path.to_string()
    } else {
        path.strip_prefix(root)
            .unwrap_or(path)
            .trim_start_matches('/')
            .to_string()
    }
}

fn label(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/* ------------------------------- packages -------------------------------- */

#[derive(Debug, Clone)]
struct CargoPackage {
    /// Project-relative package root, empty for a single-package repository.
    root: String,
    name: String,
    /// Internal Cargo package names this package depends on.
    dependencies: Vec<String>,
    /// Integration-test target names declared by the package.
    tests: Vec<String>,
}

fn discover_cargo_packages(root: &Path, files: &[String]) -> (Vec<CargoPackage>, GraphSummary) {
    if let Some(mut packages) = cargo_metadata_packages(root) {
        let edges = packages
            .iter()
            .map(|package| package.dependencies.len())
            .sum();
        let count = packages.len();
        packages.sort_by_key(|left| std::cmp::Reverse(left.root.len()));
        return (
            packages,
            GraphSummary {
                source: "cargo metadata".into(),
                packages: count,
                edges,
            },
        );
    }
    let mut packages = manifest_scan_packages(root, files);
    let edges = packages
        .iter()
        .map(|package| package.dependencies.len())
        .sum();
    let count = packages.len();
    packages.sort_by_key(|left| std::cmp::Reverse(left.root.len()));
    (
        packages,
        GraphSummary {
            source: "manifest scan".into(),
            packages: count,
            edges,
        },
    )
}

#[derive(serde::Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
}

#[derive(serde::Deserialize)]
struct MetadataPackage {
    name: String,
    manifest_path: String,
    #[serde(default)]
    dependencies: Vec<MetadataDependency>,
    #[serde(default)]
    targets: Vec<MetadataTarget>,
}

#[derive(serde::Deserialize)]
struct MetadataDependency {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(serde::Deserialize)]
struct MetadataTarget {
    name: String,
    #[serde(default)]
    kind: Vec<String>,
}

/// Ask Cargo for the workspace graph without resolving or compiling anything. Metadata is the
/// compiler-backed answer; the manifest scan below is the offline fallback when it cannot run.
fn cargo_metadata_packages(root: &Path) -> Option<Vec<CargoPackage>> {
    if !root.join("Cargo.toml").is_file() {
        return None;
    }
    let mut command = Command::new("cargo");
    command
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"));
    // A plan must never rewrite the project's lockfile or reach the network. `--locked` is
    // accepted only when a lockfile exists; a fresh project falls back to the manifest scan.
    if root.join("Cargo.lock").is_file() {
        command.arg("--locked").arg("--offline");
    } else {
        return None;
    }
    process::hide(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout).ok()?;
    if metadata.packages.is_empty() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let mut packages = Vec::new();
    let mut by_dir: BTreeMap<String, String> = BTreeMap::new();
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    for package in &metadata.packages {
        let Some(dir) = Path::new(&package.manifest_path).parent() else {
            continue;
        };
        let Some(root_rel) = relative_dir(&root, dir) else {
            continue;
        };
        let tests = package
            .targets
            .iter()
            .filter(|target| target.kind.iter().any(|kind| kind == "test"))
            .map(|target| target.name.clone())
            .collect();
        by_dir.insert(root_rel.clone(), package.name.clone());
        by_name.insert(package.name.clone(), packages.len());
        packages.push(CargoPackage {
            root: root_rel,
            name: package.name.clone(),
            dependencies: Vec::new(),
            tests,
        });
    }
    for package in &metadata.packages {
        let Some(&index) = by_name.get(&package.name) else {
            continue;
        };
        let Some(dir) = Path::new(&package.manifest_path).parent() else {
            continue;
        };
        let mut dependencies = Vec::new();
        for dependency in &package.dependencies {
            if dependency.source.is_some() {
                continue;
            }
            let mut target = None;
            if let Some(path) = &dependency.path {
                let resolved = if Path::new(path).is_absolute() {
                    Path::new(path).to_path_buf()
                } else {
                    dir.join(path)
                };
                target = relative_dir(&root, &resolved).and_then(|rel| by_dir.get(&rel).cloned());
            }
            if target.is_none() {
                target = by_dir
                    .values()
                    .find(|name| **name == dependency.name)
                    .cloned();
            }
            if let Some(name) = target
                && name != package.name
                && !dependencies.contains(&name)
            {
                dependencies.push(name);
            }
        }
        packages[index].dependencies = dependencies;
    }
    Some(packages)
}

fn relative_dir(root: &Path, dir: &Path) -> Option<String> {
    let dir = dir.canonicalize().ok()?;
    let relative = dir.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// Fallback when `cargo metadata` is unavailable: read each manifest's package name, its path
/// dependencies and its `tests/` targets. Registry dependencies are never treated as internal.
fn manifest_scan_packages(root: &Path, files: &[String]) -> Vec<CargoPackage> {
    let mut raw = Vec::new();
    for path in files
        .iter()
        .filter(|path| path.ends_with("Cargo.toml") && !is_generated(path))
    {
        let Some(name) = cargo_package_name(&root.join(path)) else {
            continue;
        };
        let dir = path.strip_suffix("/Cargo.toml").unwrap_or("").to_string();
        let tests = files
            .iter()
            .filter_map(|file| {
                let relative = relative_to(file, &dir);
                let mut parts = relative.split('/');
                if parts.next()? != "tests" {
                    return None;
                }
                let file = parts.next()?;
                if parts.next().is_some() {
                    return None;
                }
                file.strip_suffix(".rs").map(str::to_string)
            })
            .collect();
        raw.push((dir, name, cargo_path_dependencies(&root.join(path)), tests));
    }
    let by_dir: BTreeMap<String, String> = raw
        .iter()
        .map(|(dir, name, _, _)| (dir.clone(), name.clone()))
        .collect();
    raw.into_iter()
        .map(
            |(root, name, path_deps, tests): (
                String,
                String,
                Vec<(String, String)>,
                Vec<String>,
            )| {
                let mut dependencies: Vec<String> = Vec::new();
                for (_, target) in path_deps {
                    let resolved = normalize_relative(&root, &target);
                    if let Some(dependency) = by_dir.get(&resolved)
                        && dependency != &name
                        && !dependencies.contains(dependency)
                    {
                        dependencies.push(dependency.clone());
                    }
                }
                CargoPackage {
                    root,
                    name,
                    dependencies,
                    tests,
                }
            },
        )
        .collect()
}

fn cargo_path_dependencies(manifest: &Path) -> Vec<(String, String)> {
    let text = fs::read_to_string(manifest).unwrap_or_default();
    let mut section = String::new();
    let mut table_dependency: Option<String> = None;
    let mut dependencies = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_string();
            table_dependency = ["dependencies.", "dev-dependencies.", "build-dependencies."]
                .iter()
                .find_map(|prefix| section.strip_prefix(prefix).map(str::to_string));
            continue;
        }
        if let Some(name) = &table_dependency {
            if let Some(path) = path_value(line) {
                dependencies.push((name.clone(), path));
            }
            continue;
        }
        if !is_dependency_section(&section) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(path) = path_value(value) {
            dependencies.push((
                key.trim().trim_matches('"').trim_matches('\'').to_string(),
                path,
            ));
        }
    }
    dependencies
}

fn is_dependency_section(section: &str) -> bool {
    matches!(
        section,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    ) || section.ends_with(".dependencies")
        || section.ends_with(".dev-dependencies")
        || section.ends_with(".build-dependencies")
}

fn path_value(text: &str) -> Option<String> {
    let at = text.find("path")?;
    let rest = text[at + 4..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)?;
    Some(rest[1..1 + end].to_string())
}

fn normalize_relative(base: &str, path: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    let path = path.replace('\\', "/");
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Every package that reaches one of the changed packages through path dependencies.
fn affected_closure(packages: &[CargoPackage], changed: &BTreeSet<String>) -> BTreeSet<String> {
    let mut affected = changed.clone();
    loop {
        let mut grew = false;
        for package in packages {
            if affected.contains(&package.name) {
                continue;
            }
            if package
                .dependencies
                .iter()
                .any(|dependency| affected.contains(dependency))
            {
                affected.insert(package.name.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    affected
}

fn depends_on(packages: &[CargoPackage], from: &str, target: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(name) = stack.pop() {
        let Some(package) = packages.iter().find(|package| package.name == name) else {
            continue;
        };
        for dependency in &package.dependencies {
            if dependency == target {
                return true;
            }
            if seen.insert(dependency.clone()) {
                stack.push(dependency.clone());
            }
        }
    }
    false
}

fn cargo_package_name(manifest: &Path) -> Option<String> {
    let text = fs::read_to_string(manifest).ok()?;
    let mut package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            package = line == "[package]";
            continue;
        }
        if !package {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let name = rest.trim().trim_matches('"').trim_matches('\'').trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn cargo_package_for<'a>(path: &str, packages: &'a [CargoPackage]) -> Option<&'a CargoPackage> {
    packages.iter().find(|package| within(path, &package.root))
}

#[derive(Debug, Clone)]
struct NodePackage {
    name: String,
    root: String,
    script: String,
}

fn discover_node_packages(root: &Path, files: &[String]) -> Vec<NodePackage> {
    let mut packages: Vec<NodePackage> = files
        .iter()
        .filter(|path| path.ends_with("package.json") && !is_generated(path))
        .filter_map(|path| {
            let raw = fs::read_to_string(root.join(path)).ok()?;
            let manifest: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let scripts = manifest.get("scripts")?.as_object()?;
            let script = ["test", "test:e2e", "test:unit"].into_iter().find(|name| {
                scripts
                    .get(*name)
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| !value.trim().is_empty())
            })?;
            let root = path.strip_suffix("/package.json").unwrap_or("").to_string();
            let name = manifest
                .get("name")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if root.is_empty() {
                        "root".to_string()
                    } else {
                        root.rsplit('/').next().unwrap_or(&root).to_string()
                    }
                });
            Some(NodePackage {
                name,
                root,
                script: script.to_string(),
            })
        })
        .collect();
    packages.sort_by_key(|left| std::cmp::Reverse(left.root.len()));
    packages
}

fn node_package_for<'a>(path: &str, packages: &'a [NodePackage]) -> Option<&'a NodePackage> {
    packages.iter().find(|package| within(path, &package.root))
}

fn node_target(package: &NodePackage, test_path: Option<String>) -> TestTarget {
    let mut args = Vec::new();
    if !package.root.is_empty() {
        args.push("--prefix".into());
        args.push(package.root.clone());
    }
    args.push("run".into());
    args.push(package.script.clone());
    if let Some(path) = test_path {
        args.push("--".into());
        args.push(path);
    }
    let label = label("npm", &args);
    let (program, args) = testrun::npm_command(&args);
    TestTarget {
        label,
        program,
        args,
        reason: if package.root.is_empty() {
            "package test script".into()
        } else {
            format!("test script in {}", package.root)
        },
        paths: Vec::new(),
    }
}

fn within(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
}

/* ------------------------------- owners --------------------------------- */

struct CodeRule {
    pattern: String,
    owners: Vec<String>,
    negated: bool,
}

struct CodeOwners {
    rules: Vec<CodeRule>,
}

fn load_codeowners(root: &Path) -> CodeOwners {
    let path = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"]
        .iter()
        .map(|relative| root.join(relative))
        .find(|candidate| candidate.is_file());
    let Some(path) = path else {
        return CodeOwners { rules: Vec::new() };
    };
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            continue;
        };
        let negated = pattern.starts_with('!');
        let pattern = pattern.trim_start_matches('!');
        if pattern.is_empty() {
            continue;
        }
        rules.push(CodeRule {
            pattern: pattern.to_string(),
            owners: parts.map(str::to_string).collect(),
            negated,
        });
    }
    CodeOwners { rules }
}

impl CodeOwners {
    /// The last matching rule wins, as CODEOWNERS specifies; `!` clears the owners.
    fn owners_for(&self, path: &str) -> Vec<String> {
        let mut owners = Vec::new();
        for rule in &self.rules {
            if codeowners_match(&rule.pattern, path) {
                if rule.negated {
                    owners.clear();
                } else {
                    owners = rule.owners.clone();
                }
            }
        }
        owners
    }
}

fn owner_rows(root: &Path, changed: &[ChangedFile]) -> Vec<OwnerRow> {
    let rules = load_codeowners(root);
    changed
        .iter()
        .filter_map(|file| {
            let owners = rules.owners_for(&file.path);
            (!owners.is_empty()).then(|| OwnerRow {
                path: file.path.clone(),
                owners,
            })
        })
        .collect()
}

fn codeowners_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    if pattern.is_empty() {
        return false;
    }
    let mut pattern = pattern.to_string();
    if pattern.ends_with('/') {
        pattern.push_str("**");
    }
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path_parts: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if pattern_parts.is_empty() {
        return false;
    }
    if pattern_parts.len() == 1 && !pattern.contains('/') {
        // A pattern without a slash matches a file or directory at any depth.
        return path_parts
            .iter()
            .any(|part| glob_segment(pattern_parts[0], part));
    }
    glob_parts(&pattern_parts, &path_parts)
}

fn glob_parts(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        if glob_parts(&pattern[1..], path) {
            return true;
        }
        return !path.is_empty() && glob_parts(pattern, &path[1..]);
    }
    if path.is_empty() {
        return false;
    }
    glob_segment(pattern[0], path[0]) && glob_parts(&pattern[1..], &path[1..])
}

fn glob_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    fn walk(pattern: &[char], text: &[char]) -> bool {
        match pattern.first() {
            None => text.is_empty(),
            Some('*') => {
                walk(&pattern[1..], text) || (!text.is_empty() && walk(pattern, &text[1..]))
            }
            Some('?') => !text.is_empty() && walk(&pattern[1..], &text[1..]),
            Some(character) => text.first() == Some(character) && walk(&pattern[1..], &text[1..]),
        }
    }
    walk(&pattern, &text)
}

/* -------------------------------- paths --------------------------------- */

fn is_generated(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".git/",
        "target/",
        "node_modules/",
        "dist/",
        "build/",
        "coverage/",
        ".next/",
        "vendor/",
        ".rigel/",
        "__pycache__/",
        ".venv/",
        "venv/",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix) || lower.contains(&format!("/{prefix}")))
}

fn is_cargo_manifest(path: &str) -> bool {
    path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

fn is_package_manifest(path: &str) -> bool {
    path == "package.json" || path.ends_with("/package.json")
}

fn is_lockfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
    )
}

fn is_project_config(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/")
        || lower.starts_with("migrations/")
        || lower.contains("/migrations/")
        || lower.starts_with("scripts/")
        || lower == "build.rs"
        || lower.ends_with("/build.rs")
}

fn is_node_source(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|extension| matches!(extension, "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx"))
}

fn is_node_test(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.contains("/__tests__/")
        || lower.contains(".test.")
        || lower.contains(".spec.")
}

fn is_source(path: &str) -> bool {
    let extension = path.rsplit('.').next().unwrap_or_default();
    matches!(
        extension,
        "rs" | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "cs"
            | "rb"
            | "php"
            | "swift"
            | "vue"
            | "svelte"
    )
}

/// Proof bundles use the selector's exact source list so a docs-only change is treated the same
/// way in both receipts.
pub(crate) fn is_source_path(path: &str) -> bool {
    is_source(path)
}

/* --------------------------------- run ---------------------------------- */

/// Run selected targets in order, stopping at the first failure. The full suite runs only after
/// every selected target passes, so a broken impacted target is never hidden by a full run.
pub async fn run(root: &Path, plan: ImpactPlan) -> Result<ImpactRun, ImpactError> {
    let mut runs = Vec::new();
    let mut ok = true;
    let mut stopped = false;
    for target in &plan.targets {
        let run = testrun::run_plan(root, &target.plan()).await?;
        let failed = !run.ok;
        runs.push(run);
        if failed {
            ok = false;
            stopped = true;
            break;
        }
    }
    let mut full_runs = Vec::new();
    if !stopped {
        for target in &plan.full {
            let run = testrun::run_plan(root, &target.plan()).await?;
            let failed = !run.ok;
            full_runs.push(run);
            if failed {
                ok = false;
                stopped = true;
                break;
            }
        }
    }
    Ok(ImpactRun {
        plan,
        runs,
        full_runs,
        ok,
        stopped,
        proof: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: "M".into(),
        }
    }

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("crates/demo/src")).unwrap();
        fs::create_dir_all(dir.path().join("crates/demo/tests")).unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/demo\"]\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/demo/src/lib.rs"),
            "pub fn one() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/demo/tests/a.rs"),
            "#[test]\nfn one() {}\n",
        )
        .unwrap();
        fs::write(dir.path().join("README.md"), "demo\n").unwrap();
        dir
    }

    #[test]
    fn a_source_change_selects_its_cargo_package() {
        let project = project();
        let plan = plan(project.path(), &[change("crates/demo/src/lib.rs")], false).unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].label, "cargo test -p demo");
        assert!(plan.full.is_empty());
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn a_changed_integration_test_selects_the_test_target() {
        let project = project();
        let plan = plan(project.path(), &[change("crates/demo/tests/a.rs")], false).unwrap();
        assert_eq!(plan.targets[0].label, "cargo test -p demo --test a");
        assert!(plan.full.is_empty());
    }

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("crates/core/src")).unwrap();
        fs::create_dir_all(dir.path().join("crates/app/src")).unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/core/Cargo.toml"),
            "[package]\nname = \"demo-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/core/src/lib.rs"),
            "pub fn core() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/app/Cargo.toml"),
            "[package]\nname = \"demo-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndemo-core = { path = \"../core\" }\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("crates/app/src/lib.rs"),
            "pub fn app() { demo_core::core(); }\n",
        )
        .unwrap();
        let mut command = Command::new("cargo");
        command
            .arg("generate-lockfile")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(dir.path().join("Cargo.toml"));
        process::hide(&mut command);
        assert!(
            command.status().unwrap().success(),
            "the fixture lockfile is generated"
        );
        dir
    }

    #[test]
    fn a_changed_package_selects_the_packages_that_depend_on_it() {
        let project = workspace();
        let plan = plan(project.path(), &[change("crates/core/src/lib.rs")], false).unwrap();
        let labels: Vec<&str> = plan
            .targets
            .iter()
            .map(|target| target.label.as_str())
            .collect();
        assert!(labels.contains(&"cargo test -p demo-core"), "{labels:?}");
        assert!(labels.contains(&"cargo test -p demo-app"), "{labels:?}");
        assert_eq!(plan.graph.source, "cargo metadata");
        assert_eq!(plan.graph.packages, 2);
        assert_eq!(plan.graph.edges, 1);
        assert!(
            plan.packages
                .iter()
                .any(|package| package.name == "demo-app" && package.role == "dependent")
        );
        assert!(
            plan.targets
                .iter()
                .any(|target| target.label == "cargo test -p demo-app"
                    && target.reason.contains("demo-core"))
        );
    }

    #[test]
    fn a_change_in_the_dependent_package_does_not_select_the_dependency() {
        let project = workspace();
        let plan = plan(project.path(), &[change("crates/app/src/lib.rs")], false).unwrap();
        let labels: Vec<&str> = plan
            .targets
            .iter()
            .map(|target| target.label.as_str())
            .collect();
        assert_eq!(labels, vec!["cargo test -p demo-app"]);
        assert_eq!(plan.packages.len(), 1);
    }

    #[test]
    fn a_symbol_reference_adds_an_exact_test_without_dropping_package_targets() {
        let project = workspace();
        fs::create_dir_all(project.path().join("crates/app/tests")).unwrap();
        fs::write(
            project.path().join("crates/app/tests/integration.rs"),
            "use demo_core::core;\n#[test]\nfn core_test() { assert_eq!(core(), 1); }\n",
        )
        .unwrap();
        let plan = plan(project.path(), &[change("crates/core/src/lib.rs")], false).unwrap();
        let labels: Vec<&str> = plan
            .targets
            .iter()
            .map(|target| target.label.as_str())
            .collect();
        assert!(labels.contains(&"cargo test -p demo-core"), "{labels:?}");
        assert!(labels.contains(&"cargo test -p demo-app"), "{labels:?}");
        assert!(
            labels.contains(&"cargo test -p demo-app --test integration"),
            "{labels:?}"
        );
        assert!(
            plan.symbols
                .changed
                .iter()
                .any(|symbol| symbol.name == "core" && symbol.kind == "fn")
        );
        let app = plan
            .packages
            .iter()
            .find(|package| package.name == "demo-app")
            .unwrap();
        assert!(app.selected_by.contains("symbol references"));
        assert!(app.references.iter().any(|reference| reference.path
            == "crates/app/tests/integration.rs"
            && reference.kind == "test"));
    }

    #[test]
    fn an_unrelated_symbol_does_not_add_a_test_target() {
        let project = workspace();
        fs::create_dir_all(project.path().join("crates/app/tests")).unwrap();
        fs::write(
            project.path().join("crates/app/tests/integration.rs"),
            "use demo_core::core;\n#[test]\nfn core_test() { assert_eq!(core(), 1); }\n",
        )
        .unwrap();
        let plan = plan(project.path(), &[change("crates/app/src/lib.rs")], false).unwrap();
        let labels: Vec<&str> = plan
            .targets
            .iter()
            .map(|target| target.label.as_str())
            .collect();
        assert_eq!(labels, vec!["cargo test -p demo-app"]);
        assert!(!plan.symbols.changed.is_empty());
        assert!(plan.symbols.reached.is_empty());
    }

    #[test]
    fn codeowners_are_applied_last_match_first() {
        let project = project();
        fs::create_dir_all(project.path().join(".github")).unwrap();
        fs::write(
            project.path().join(".github/CODEOWNERS"),
            "* @fallback\n/crates/demo/ @demo\ncrates/demo/src/*.rs @src\n",
        )
        .unwrap();
        let plan = plan(
            project.path(),
            &[change("crates/demo/src/lib.rs"), change("README.md")],
            false,
        )
        .unwrap();
        let owners: BTreeMap<&str, &Vec<String>> = plan
            .owners
            .iter()
            .map(|row| (row.path.as_str(), &row.owners))
            .collect();
        assert_eq!(owners["crates/demo/src/lib.rs"], &vec!["@src".to_string()]);
        assert_eq!(owners["README.md"], &vec!["@fallback".to_string()]);
    }

    #[test]
    fn a_negated_codeowners_rule_clears_the_owners() {
        let project = project();
        fs::write(
            project.path().join("CODEOWNERS"),
            "/crates/demo/ @demo\n!crates/demo/generated/\n",
        )
        .unwrap();
        let rules = load_codeowners(project.path());
        assert_eq!(rules.owners_for("crates/demo/src/lib.rs"), vec!["@demo"]);
        assert!(rules.owners_for("crates/demo/generated/out.rs").is_empty());
    }

    #[test]
    fn path_dependencies_are_read_without_metadata() {
        let project = workspace();
        let deps = cargo_path_dependencies(&project.path().join("crates/app/Cargo.toml"));
        assert_eq!(deps, vec![("demo-core".to_string(), "../core".to_string())]);
    }

    #[test]
    fn docs_are_reported_as_skipped_and_run_nothing() {
        let project = project();
        let plan = plan(project.path(), &[change("README.md")], false).unwrap();
        assert!(plan.targets.is_empty());
        assert!(plan.full.is_empty());
        assert_eq!(plan.skipped[0].path, "README.md");
        assert!(plan.risk.is_empty());
    }

    #[test]
    fn a_manifest_change_requires_the_full_suite() {
        let project = project();
        let plan = plan(project.path(), &[change("Cargo.toml")], false).unwrap();
        assert!(plan.full.iter().any(|target| target.label == "cargo test"));
        assert!(plan.risk.iter().any(|reason| reason.contains("manifest")));
    }

    #[test]
    fn an_unmapped_source_change_requires_the_full_suite() {
        let project = project();
        let plan = plan(
            project.path(),
            &[change("crates/outside/src/main.rs")],
            false,
        )
        .unwrap();
        assert!(plan.targets.is_empty());
        assert!(plan.full.iter().any(|target| target.label == "cargo test"));
        assert!(
            plan.skipped[0]
                .reason
                .contains("not inside a Cargo package")
        );
    }

    #[test]
    fn a_broad_change_and_a_forced_run_both_add_the_full_suite() {
        let project = project();
        let many: Vec<ChangedFile> = (0..=MAX_CHANGED_FILES)
            .map(|index| change(&format!("crates/demo/src/{index}.rs")))
            .collect();
        let broad = plan(project.path(), &many, false).unwrap();
        assert!(broad.full.iter().any(|target| target.label == "cargo test"));

        let forced = plan(project.path(), &[], true).unwrap();
        assert!(
            forced
                .full
                .iter()
                .any(|target| target.label == "cargo test")
        );
        assert!(
            forced
                .risk
                .iter()
                .any(|reason| reason == "full suite requested")
        );
    }

    #[tokio::test]
    async fn a_failed_impacted_target_stops_before_the_full_suite() {
        #[cfg(windows)]
        let (fail, pass) = (
            TestTarget {
                label: "fail".into(),
                program: "cmd".into(),
                args: vec!["/C".into(), "echo 1 failed & exit /b 1".into()],
                reason: "fixture".into(),
                paths: Vec::new(),
            },
            TestTarget {
                label: "full".into(),
                program: "cmd".into(),
                args: vec!["/C".into(), "echo full passed".into()],
                reason: "fixture".into(),
                paths: Vec::new(),
            },
        );
        #[cfg(not(windows))]
        let (fail, pass) = (
            TestTarget {
                label: "fail".into(),
                program: "sh".into(),
                args: vec!["-c".into(), "printf '1 failed\\n'; exit 1".into()],
                reason: "fixture".into(),
                paths: Vec::new(),
            },
            TestTarget {
                label: "full".into(),
                program: "sh".into(),
                args: vec!["-c".into(), "printf 'full passed\\n'".into()],
                reason: "fixture".into(),
                paths: Vec::new(),
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let report = run(
            dir.path(),
            ImpactPlan {
                changed: vec![change("src/lib.rs")],
                targets: vec![fail],
                full: vec![pass],
                skipped: Vec::new(),
                risk: vec!["fixture risk".into()],
                packages: Vec::new(),
                owners: Vec::new(),
                graph: GraphSummary::default(),
                symbols: SymbolImpact::default(),
            },
        )
        .await
        .unwrap();
        assert!(!report.ok);
        assert!(report.stopped);
        assert_eq!(report.runs.len(), 1);
        assert!(report.full_runs.is_empty());
    }

    #[tokio::test]
    async fn a_clean_impacted_run_reaches_the_full_suite() {
        #[cfg(windows)]
        let target = || TestTarget {
            label: "pass".into(),
            program: "cmd".into(),
            args: vec!["/C".into(), "echo 1 passed".into()],
            reason: "fixture".into(),
            paths: Vec::new(),
        };
        #[cfg(not(windows))]
        let target = || TestTarget {
            label: "pass".into(),
            program: "sh".into(),
            args: vec!["-c".into(), "printf '1 passed\\n'".into()],
            reason: "fixture".into(),
            paths: Vec::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let report = run(
            dir.path(),
            ImpactPlan {
                changed: vec![change("src/lib.rs")],
                targets: vec![target()],
                full: vec![target()],
                skipped: Vec::new(),
                risk: vec!["fixture risk".into()],
                packages: Vec::new(),
                owners: Vec::new(),
                graph: GraphSummary::default(),
                symbols: SymbolImpact::default(),
            },
        )
        .await
        .unwrap();
        assert!(report.ok);
        assert!(!report.stopped);
        assert_eq!(report.runs.len(), 1);
        assert_eq!(report.full_runs.len(), 1);
    }

    #[test]
    fn git_changes_include_modified_and_untracked_files() {
        let project = project();
        let git = |args: &[&str]| {
            let mut command = Command::new("git");
            command.arg("-C").arg(project.path()).args(args);
            process::hide(&mut command);
            assert!(command.status().unwrap().success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);

        fs::write(
            project.path().join("crates/demo/src/lib.rs"),
            "pub fn two() {}\n",
        )
        .unwrap();
        fs::write(project.path().join("notes.txt"), "later\n").unwrap();
        let changed = changed_files(project.path(), "HEAD").unwrap();
        let paths: Vec<&str> = changed.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"crates/demo/src/lib.rs"));
        assert!(paths.contains(&"notes.txt"));
        assert!(changed.iter().any(|file| file.status == "?"));
    }
}
