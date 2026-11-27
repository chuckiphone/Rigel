use std::process::Command;

#[test]
fn skills_init_creates_a_parseable_skill_from_the_cli() {
    let directory = tempfile::tempdir().unwrap();
    let skill = directory.path().join("SKILL.md");
    let output = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .args(["skills", "init"])
        .arg(&skill)
        .args(["--name", "Example"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = std::fs::read_to_string(skill).unwrap();
    let metadata = rigel_core::skills::frontmatter::indexed_metadata(&body, "fallback");
    assert_eq!(metadata.0, "Example");
    assert_eq!(metadata.1, "Describe what this skill does.");
}

#[test]
fn builtin_install_runs_persisted_golden_suite_and_reports_failure_diff() {
    let directory = tempfile::tempdir().unwrap();
    let suites = directory.path().join("suite.json");
    std::fs::write(
        &suites,
        r#"[{"name":"must explain evaluation","prompt":"Explain evaluation","expected_contains":["expected marker"]}]"#,
    )
    .unwrap();
    let saved = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(directory.path())
        .args(["skills", "eval", "set", "builtin:skillevaluator"])
        .arg(&suites)
        .output()
        .unwrap();
    assert!(
        saved.status.success(),
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );

    let script = directory.path().join("model.json");
    std::fs::write(&script, r#"{"steps":[{"text":"actual model output"}]}"#).unwrap();
    let installed = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(directory.path())
        .args(["skills", "builtin", "--install", "--to"])
        .arg(directory.path().join("harness"))
        .env("RIGEL_SKILL_EVAL_SCRIPT", script)
        .output()
        .unwrap();

    assert!(!installed.status.success());
    let error = String::from_utf8_lossy(&installed.stderr);
    assert!(error.contains("must explain evaluation"), "{error}");
    assert!(error.contains("- expected marker"), "{error}");
    assert!(error.contains("+ actual model output"), "{error}");
    assert!(
        !directory
            .path()
            .join("harness/skillevaluator/SKILL.md")
            .exists()
    );
}
