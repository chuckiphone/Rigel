//! `rigel impact` as a program: the compiler-backed package graph and CODEOWNERS owners.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn rigel(project: &Path, args: &[&str]) -> (i32, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(args)
        .arg("--project")
        .arg(project)
        .output()
        .expect("rigel runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "not json: {error}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.code().unwrap_or(-1), json)
}

fn git(project: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    command.arg("-C").arg(project).args(args);
    assert!(
        command.status().expect("git runs").success(),
        "git {args:?}"
    );
}

fn fixture() -> TempDir {
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join("crates/core/src")).unwrap();
    fs::create_dir_all(project.path().join("crates/app/src")).unwrap();
    fs::create_dir_all(project.path().join("crates/app/tests")).unwrap();
    fs::create_dir_all(project.path().join(".github")).unwrap();
    fs::write(
        project.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        project.path().join("crates/core/Cargo.toml"),
        "[package]\nname = \"impact-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        project.path().join("crates/core/src/lib.rs"),
        "pub fn core() {}\n",
    )
    .unwrap();
    fs::write(
        project.path().join("crates/app/Cargo.toml"),
        "[package]\nname = \"impact-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nimpact-core = { path = \"../core\" }\n",
    )
    .unwrap();
    fs::write(
        project.path().join("crates/app/src/lib.rs"),
        "pub fn app() { impact_core::core(); }\n",
    )
    .unwrap();
    fs::write(
        project.path().join("crates/app/tests/integration.rs"),
        "use impact_core::core;\n#[test]\nfn core_test() { core(); }\n",
    )
    .unwrap();
    fs::write(
        project.path().join(".github/CODEOWNERS"),
        "/crates/core/ @core-team\n/crates/app/ @app-team\n",
    )
    .unwrap();

    let mut lock = Command::new("cargo");
    lock.arg("generate-lockfile")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project.path().join("Cargo.toml"));
    assert!(lock.status().expect("cargo runs").success());

    git(project.path(), &["init", "-q"]);
    git(
        project.path(),
        &["config", "user.email", "test@example.com"],
    );
    git(project.path(), &["config", "user.name", "Test"]);
    git(project.path(), &["add", "."]);
    git(project.path(), &["commit", "-qm", "base"]);
    project
}

#[test]
fn a_changed_core_package_reaches_the_app_and_names_both_owners() {
    let project = fixture();
    fs::write(
        project.path().join("crates/core/src/lib.rs"),
        "pub fn core() { let _ = 2; }\n",
    )
    .unwrap();

    let (code, plan) = rigel(
        project.path(),
        &[
            "impact",
            "--changed-file",
            "crates/core/src/lib.rs",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{plan}");
    assert_eq!(plan["graph"]["source"], "cargo metadata");
    assert_eq!(plan["graph"]["packages"], 2);
    assert_eq!(plan["graph"]["edges"], 1);

    let packages = plan["packages"].as_array().unwrap();
    assert!(
        packages
            .iter()
            .any(|package| { package["name"] == "impact-core" && package["role"] == "changed" })
    );
    assert!(
        packages
            .iter()
            .any(|package| { package["name"] == "impact-app" && package["role"] == "dependent" })
    );

    let targets = plan["targets"].as_array().unwrap();
    assert!(
        targets
            .iter()
            .any(|target| target["label"] == "cargo test -p impact-core")
    );
    assert!(
        targets
            .iter()
            .any(|target| target["label"] == "cargo test -p impact-app")
    );
    assert!(
        targets
            .iter()
            .any(|target| target["label"] == "cargo test -p impact-app --test integration"),
        "a test that names the changed symbol gets an exact target"
    );

    assert_eq!(plan["symbols"]["source"], "git diff");
    assert!(
        plan["symbols"]["changed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|symbol| symbol["name"] == "core" && symbol["kind"] == "fn")
    );
    assert!(
        plan["symbols"]["reached"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reach| reach["references"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |reference| reference["path"] == "crates/app/tests/integration.rs"
                        && reference["kind"] == "test"
                ))
    );
    let app = packages
        .iter()
        .find(|package| package["name"] == "impact-app")
        .unwrap();
    assert!(
        app["selected_by"]
            .as_str()
            .unwrap()
            .contains("symbol references")
    );
    assert!(
        app["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reference| reference["path"] == "crates/app/tests/integration.rs")
    );

    let owners = plan["owners"].as_array().unwrap();
    assert!(
        owners
            .iter()
            .any(|row| row["path"] == "crates/core/src/lib.rs" && row["owners"][0] == "@core-team")
    );
}

#[test]
fn a_change_in_the_app_does_not_select_the_dependency() {
    let project = fixture();
    fs::write(
        project.path().join("crates/app/src/lib.rs"),
        "pub fn app() { let _ = 2; }\n",
    )
    .unwrap();
    let (code, plan) = rigel(
        project.path(),
        &[
            "impact",
            "--changed-file",
            "crates/app/src/lib.rs",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{plan}");
    let labels: Vec<&str> = plan["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, vec!["cargo test -p impact-app"]);
    assert_eq!(plan["packages"].as_array().unwrap().len(), 1);
}
