//! `rigel handoff` as a program: the prompt carries the project rules and worktree evidence,
//! and it can be used unchanged as the first turn of a new session.

use std::path::Path;
use std::process::Command;

use rigel_core::{SessionService, plan};
use serde_json::{Value, json};
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_rigel");
    let output = Command::new(exe)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("rigel runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn git(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn repo() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    assert!(git(tmp.path(), &["init", "-q"]), "git init");
    assert!(git(tmp.path(), &["config", "user.email", "rigel@test"]));
    assert!(git(tmp.path(), &["config", "user.name", "Rigel Test"]));
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# Project instructions\n\n- Never mark a task done before its named test passes.\n- Keep the database local.\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn version() -> u8 { 1 }\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".gitignore"), ".rigel/\n").unwrap();
    assert!(git(tmp.path(), &["add", "."]));
    assert!(git(tmp.path(), &["commit", "-q", "-m", "initial"]));
    // One tracked edit and one untracked file: the handoff must show both, but only the
    // tracked edit can appear in `git diff --stat`.
    std::fs::write(
        tmp.path().join("src/lib.rs"),
        "pub fn version() -> u8 { 2 }\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("notes.md"), "remember the test\n").unwrap();
    tmp
}

#[tokio::test]
async fn a_handoff_carries_the_rules_and_round_trips_into_a_new_session() {
    let data = TempDir::new().unwrap();
    let project = repo();
    let project_path = project.path().to_str().unwrap().to_string();
    let service = SessionService::open(project.path()).await.unwrap();
    let session = service.start(Some("handoff test"), None).await.unwrap();
    let agent = service
        .register_agent_with(
            &session.id,
            "native",
            Some("implementer"),
            Some("test/model"),
        )
        .await
        .unwrap();
    let turn = service
        .append_turn(
            &agent.id,
            Some("Finish the handoff prompt.\nKeep the constraints intact."),
        )
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "file_change",
            "applied",
            &json!({
                "tool": "propose_patch",
                "result": { "path": "src/lib.rs" }
            }),
            &json!({}),
        )
        .await
        .unwrap();
    service
        .append_item(
            &turn.id,
            "message",
            "complete",
            &json!({
                "role": "assistant",
                "text": "Started the command; the integration test is still missing."
            }),
            &json!({}),
        )
        .await
        .unwrap();
    service.finish_turn(&turn.id, "completed").await.unwrap();
    plan::propose(
        service.pool(),
        &session.id,
        "1. Write the integration test.\n2. Run the CLI test.",
        &["run_command".to_string()],
    )
    .await
    .unwrap();

    let args = [
        "handoff",
        "--project",
        project_path.as_str(),
        "--session",
        session.id.as_str(),
        "--open-question",
        "Which check proves the handoff is complete?",
        "--json",
    ];
    let (code, stdout) = rigel(data.path(), &args);
    assert_eq!(code, 0, "{stdout}");
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    let prompt = value["prompt"].as_str().expect("prompt");

    assert_eq!(value["session"]["id"], session.id);
    assert!(
        value["constraints"]["text"]
            .as_str()
            .unwrap()
            .contains("Never mark a task done before its named test passes.")
    );
    assert_eq!(value["files"]["session"][0], "src/lib.rs");
    assert!(
        value["files"]["worktree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "src/lib.rs")
    );
    assert!(
        value["files"]["worktree"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "notes.md" && file["status"] == "??")
    );
    assert_eq!(value["diff"]["is_repo"], true);
    assert!(
        value["diff"]["summary"]
            .as_str()
            .unwrap()
            .contains("1 file changed")
    );
    assert!(
        value["plan"]
            .as_str()
            .unwrap()
            .contains("1. Write the integration test.")
    );
    assert_eq!(
        value["open_questions"][0],
        "Which check proves the handoff is complete?"
    );
    assert!(
        value["last_output"]
            .as_str()
            .unwrap()
            .contains("integration test is still missing")
    );

    assert!(prompt.starts_with("# Handoff — handoff test"));
    assert!(prompt.contains("## Task"));
    assert!(prompt.contains("Keep the constraints intact."));
    assert!(prompt.contains("## Constraints"));
    assert!(prompt.contains("Never mark a task done before its named test passes."));
    assert!(prompt.contains("## Files"));
    assert!(prompt.contains("`src/lib.rs`"));
    assert!(prompt.contains("## Diff summary"));
    assert!(prompt.contains("1 file changed"));
    assert!(prompt.contains("## Open questions"));
    assert!(prompt.contains("- [ ] Which check proves the handoff is complete?"));
    assert!(prompt.contains("## Open plan (unverified)"));
    assert!(prompt.contains("1. Write the integration test."));
    assert!(prompt.contains("nothing here is marked done"));
    assert!(prompt.contains("not proof that the task is complete"));
    assert!(!prompt.contains("- [x]"));

    // The plain form writes the same prompt; omitting --session selects the newest session.
    let out = data.path().join("handoff.md");
    let out_text = out.to_str().unwrap();
    let (code, stdout) = rigel(
        data.path(),
        &[
            "handoff",
            "--project",
            &project_path,
            "--open-question",
            "Which check proves the handoff is complete?",
            "--out",
            out_text,
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(stdout.trim(), prompt);
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), prompt);

    // Round-trip: the prompt is a valid first turn in a brand-new session, constraints and
    // open questions included, and it does not arrive pre-checked.
    let next = service.start(Some("continue"), None).await.unwrap();
    let next_agent = service
        .register_agent(&next.id, "native", None)
        .await
        .unwrap();
    let next_turn = service
        .append_turn(&next_agent.id, Some(prompt))
        .await
        .unwrap();
    let reloaded = service.turn(&next_turn.id).await.unwrap();
    assert_eq!(reloaded.prompt.as_deref(), Some(prompt));
    let transcript = service.transcript(&next.id).await.unwrap();
    assert_eq!(transcript[0].turn.prompt.as_deref(), Some(prompt));
    assert!(
        transcript[0]
            .turn
            .prompt
            .as_deref()
            .unwrap()
            .contains("Never mark a task done before its named test passes.")
    );
}
