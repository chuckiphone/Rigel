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
use crate::testrun::{self, TestPlan, TestRun, TestRunError};
use crate::tree;

/// A change larger than this is no longer a focused edit; let the full suite decide.
pub const MAX_CHANGED_FILES: usize = 6;
/// Touching more Cargo packages than this is a broad change, not an impacted edit.
pub const MAX_PACKAGES: usize = 2;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactPlan {
    pub changed: Vec<ChangedFile>,
    pub targets: Vec<TestTarget>,
    /// The full suite to run after the selected targets, only when the selection is risky.
    pub full: Vec<TestTarget>,
    pub skipped: Vec<SkippedChange>,
    /// Why the full suite is required; empty when selected targets are the whole run.
    pub risk: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactRun {
    pub plan: ImpactPlan,
    pub runs: Vec<TestRun>,
    pub full_runs: Vec<TestRun>,
    pub ok: bool,
    /// A selected or full target failed, so nothing after it ran.
    pub stopped: bool,
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
    Ok(changes.into_values().collect())
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
    let files = tree::project_files(root).map_err(|error| ImpactError::Tree(error.to_string()))?;
    let cargo = discover_cargo_packages(root, &files);
    let node = discover_node_packages(root, &files);

    let mut targets: BTreeMap<String, TestTarget> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut risk = BTreeSet::new();
    let mut packages: BTreeSet<String> = BTreeSet::new();
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
                add_target(
                    &mut targets,
                    package_target(package, "changed Cargo manifest", vec![path.to_string()]),
                );
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
                add_target(
                    &mut targets,
                    package_target(
                        package,
                        &format!("source change in package {}", package.name),
                        vec![path.to_string()],
                    ),
                );
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

    Ok(ImpactPlan {
        changed: changed.to_vec(),
        targets: targets.into_values().collect(),
        full,
        skipped,
        risk: risk.into_iter().collect(),
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
}

fn discover_cargo_packages(root: &Path, files: &[String]) -> Vec<CargoPackage> {
    let mut packages: Vec<CargoPackage> = files
        .iter()
        .filter(|path| path.ends_with("Cargo.toml") && !is_generated(path))
        .filter_map(|path| {
            let name = cargo_package_name(&root.join(path))?;
            Some(CargoPackage {
                root: path.strip_suffix("/Cargo.toml").unwrap_or("").to_string(),
                name,
            })
        })
        .collect();
    packages.sort_by_key(|left| std::cmp::Reverse(left.root.len()));
    packages
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
            Some(NodePackage {
                root: path.strip_suffix("/package.json").unwrap_or("").to_string(),
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
