//! Plugin store, bundled tree and runtime, plus the built-in skill seed.
//!
//! The capability boundary itself is unit-tested in `plugin::host`; this file proves the
//! install path writes a receipt for exactly the tree it runs, and that the bundled plugin's
//! granted read works through a real child process when Python is present.

use std::fs;
use std::process::Stdio;
use std::sync::Arc;

use rigel_core::plugin::{PluginRuntime, PluginStore};
use rigel_core::{BudgetService, Cancels, SessionService, SkillIndex, StopAt, Tools};

fn python() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn plugin_call_is_a_registered_tool_the_permission_matrix_can_grant_or_refuse() {
    let names: Vec<String> = rigel_core::Tools::specs_all()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(names.iter().any(|name| name == "plugin_call"));
    assert!(
        rigel_core::permissions::all_tools()
            .iter()
            .any(|name| name == "plugin_call"),
        "a project with no permission file must grant the new tool, like every other tool"
    );
}

#[test]
fn a_local_install_records_a_receipt_and_survives_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("plugin.json"),
        r#"{"schema":1,"name":"echo","version":"1.2.3",
            "description":"echoes",
            "entry":{"command":"python","args":["main.py"]},
            "tools":[{"name":"echo","description":"Echo a value."}]}"#,
    )
    .unwrap();
    fs::write(source.path().join("main.py"), "print('{}')\n").unwrap();

    let store = PluginStore::new(data.path());
    let report = store.install_local(source.path(), false).unwrap();
    assert_eq!(report.name, "echo");
    assert_eq!(report.version, "1.2.3");
    assert_eq!(report.tools, vec!["echo.echo"]);
    assert!(report.directory.join("plugin.json").is_file());
    assert_eq!(report.digest.len(), 64);

    // A new store over the same directory is the restart case.
    let reopened = PluginStore::new(data.path());
    let installed = reopened.list().unwrap();
    assert_eq!(installed.len(), 1);
    assert!(installed[0].enabled);
    assert_eq!(
        installed[0].receipt.as_ref().unwrap().sha256.as_deref(),
        Some(report.digest.as_str())
    );

    assert!(
        store.install_local(source.path(), false).is_err(),
        "a second install without --force must refuse"
    );
    let replaced = store.install_local(source.path(), true).unwrap();
    assert!(replaced.replaced);
}

#[test]
fn a_manifest_rewritten_after_install_is_refused_instead_of_widening_the_grant() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("plugin.json"),
        r#"{"schema":1,"name":"echo","version":"1.0.0",
            "entry":{"command":"python","args":["main.py"]},
            "tools":[{"name":"echo","description":"Echo."}]}"#,
    )
    .unwrap();
    let store = PluginStore::new(data.path());
    let report = store.install_local(source.path(), false).unwrap();

    // The plugin decides it would like a read grant it was never reviewed for.
    fs::write(
        report.directory.join("plugin.json"),
        r#"{"schema":1,"name":"echo","version":"1.0.0",
            "entry":{"command":"python","args":["main.py"]},
            "capabilities":{"filesystem":{"read":["/"]}},
            "tools":[{"name":"echo","description":"Echo."}]}"#,
    )
    .unwrap();

    let (plugins, broken) = store.scan().unwrap();
    assert!(plugins.is_empty(), "the changed plugin must not be offered");
    assert!(
        broken
            .iter()
            .any(|(name, error)| name == "echo" && error.contains("changed since install")),
        "the refusal must name the cause: {broken:?}"
    );
    assert!(PluginRuntime::open(data.path()).enabled().is_empty());
}

#[test]
fn disabling_keeps_the_files_and_the_runtime_stops_offering_them() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fs::write(
        source.path().join("plugin.json"),
        r#"{"schema":1,"name":"echo","version":"1.0.0",
            "entry":{"command":"python","args":["main.py"]},
            "tools":[{"name":"echo","description":"Echo."}]}"#,
    )
    .unwrap();
    let store = PluginStore::new(data.path());
    store.install_local(source.path(), false).unwrap();

    let row = store.set_enabled("echo", false).unwrap();
    assert!(!row.enabled);
    assert_eq!(store.get("echo").unwrap().manifest.version, "1.0.0");

    let runtime = PluginRuntime::open(data.path());
    assert!(runtime.enabled().is_empty());
    drop(runtime);

    store.set_enabled("echo", true).unwrap();
    assert_eq!(PluginRuntime::open(data.path()).enabled().len(), 1);

    assert!(store.remove("echo").unwrap());
    assert!(!store.remove("echo").unwrap());
}

#[test]
fn a_tampered_install_names_the_cause_and_can_be_removed() {
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path());
    let report = store.install_bundled("rigel-hello", false).unwrap();
    let manifest = report.directory.join("plugin.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    value["capabilities"] = serde_json::json!({ "filesystem": { "read": ["/"] } });
    fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    // The same state the settings page reads must name the cause, not just say "not installed".
    let error = store.set_enabled("rigel-hello", false).unwrap_err();
    assert!(error.to_string().contains("broken"), "{error}");
    assert!(
        error.to_string().contains("changed since install"),
        "{error}"
    );

    let (plugins, broken) = store.scan().unwrap();
    assert!(plugins.is_empty());
    assert!(broken.iter().any(|(name, _)| name == "rigel-hello"));

    assert!(store.remove("rigel-hello").unwrap());
    assert!(!report.directory.exists());
    let (plugins, broken) = store.scan().unwrap();
    assert!(plugins.is_empty() && broken.is_empty());

    // The old receipt is gone, so the catalogue's normal install path works again.
    store.install_bundled("rigel-hello", false).unwrap();
}

#[test]
fn removal_refuses_to_walk_out_of_the_plugin_store() {
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path());
    let outside = data.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("plugin.json"), "{}").unwrap();

    assert!(!store.remove("../outside").unwrap());
    assert!(outside.join("plugin.json").is_file());
}

#[tokio::test]
async fn disabling_a_plugin_removes_plugin_call_from_the_next_turns_tools() {
    let data = tempfile::tempdir().unwrap();
    let project = data.path().join("project");
    fs::create_dir(&project).unwrap();
    let index = data.path().join("index");
    PluginStore::new(&index)
        .install_bundled("rigel-hello", false)
        .unwrap();

    let service = SessionService::open(&project).await.unwrap();
    let budget = Arc::new(BudgetService::new(
        service.pool().clone(),
        Arc::new(Cancels::new()) as Arc<dyn rigel_core::InterruptSink>,
    ));
    let tools = Tools::new(
        project.clone(),
        service.clone(),
        budget.clone(),
        index.clone(),
        StopAt::Never,
    );
    assert!(tools.specs().iter().any(|spec| spec.name == "plugin_call"));

    PluginStore::new(&index)
        .set_enabled("rigel-hello", false)
        .unwrap();
    let tools = Tools::new(project, service, budget, index, StopAt::Never);
    assert!(
        !tools.specs().iter().any(|spec| spec.name == "plugin_call"),
        "a disabled plugin must not be offered to the next turn"
    );
}

#[test]
fn a_pinned_install_checks_out_exactly_the_sha() {
    let git = |args: &[&str], cwd: &std::path::Path| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let probe = std::process::Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.is_ok_and(|status| status.success()) {
        return;
    }

    let repo = tempfile::tempdir().unwrap();
    git(&["init", "--quiet"], repo.path());
    fs::write(
        repo.path().join("plugin.json"),
        r#"{"schema":1,"name":"pinned","version":"2.0.0",
            "entry":{"command":"python","args":["main.py"]},
            "tools":[{"name":"ping","description":"Ping."}]}"#,
    )
    .unwrap();
    fs::write(repo.path().join("main.py"), "print('{}')\n").unwrap();
    git(&["add", "."], repo.path());
    git(
        &[
            "-c",
            "user.email=rigel@example.invalid",
            "-c",
            "user.name=Rigel Test",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
        repo.path(),
    );
    let sha = git(&["rev-parse", "HEAD"], repo.path());
    let repo_url = repo.path().to_string_lossy().replace('\\', "/");

    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path());
    let report = store
        .install_git(&repo_url, &sha, None, false)
        .expect("a local repository at its own commit installs");
    assert_eq!(report.name, "pinned");
    assert_eq!(report.version, "2.0.0");

    let short = store.install_git(&repo_url, &sha[..12], None, true);
    assert!(short.is_err(), "a short sha must be refused before cloning");
}

#[tokio::test]
async fn the_bundled_plugin_answers_through_the_capability_broker() {
    if !python() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path());
    store.install_bundled("rigel-hello", false).unwrap();

    let runtime = PluginRuntime::open(data.path());
    let hello = runtime
        .call(
            Some("rigel-hello"),
            "hello",
            serde_json::json!({ "name": "Rigel" }),
        )
        .await
        .unwrap();
    assert!(hello.text().contains("Hello, Rigel"));

    let note = runtime
        .call(
            Some("rigel-hello"),
            "read_note",
            serde_json::json!({ "path": "plugin.json" }),
        )
        .await
        .unwrap();
    assert_eq!(note.content["ok"], true);
    assert!(
        note.content["content"]
            .as_str()
            .unwrap()
            .contains("rigel-hello")
    );
}

#[tokio::test]
async fn a_bundled_plugin_cannot_read_outside_its_directory() {
    if !python() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "NEVER LEAVE").unwrap();
    let store = PluginStore::new(data.path());
    store.install_bundled("rigel-hello", false).unwrap();

    let runtime = PluginRuntime::open(data.path());
    let answer = runtime
        .call(
            Some("rigel-hello"),
            "read_note",
            serde_json::json!({ "path": "../secret.txt" }),
        )
        .await
        .unwrap();
    assert_eq!(answer.content["ok"], false);
    assert_eq!(answer.content["error"]["code"], "CAPABILITY_DENIED");
    assert!(!answer.text().contains("NEVER LEAVE"));
}

#[tokio::test]
async fn built_in_skills_are_indexed_once_and_searchable_offline() {
    let data = tempfile::tempdir().unwrap();
    let index = SkillIndex::open(data.path()).await.unwrap();
    let first = index.install_builtin().await.unwrap();
    assert_eq!(first.written, rigel_core::skills::builtin::all().len());

    let hits = index.search("llmfit", 5).await.unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.id == "builtin:llmfit" && hit.source == "builtin"),
        "the bundled llmfit row should be searchable: {hits:?}"
    );
    let detail = index.get("builtin:llmfit").await.unwrap().unwrap();
    assert!(detail.skill.has_body);

    let before = index.count("").await.unwrap();
    let second = index.install_builtin().await.unwrap();
    assert_eq!(second.written, first.written);
    assert_eq!(index.count("").await.unwrap(), before);
}
