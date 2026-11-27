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
