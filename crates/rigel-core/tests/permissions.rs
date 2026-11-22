//! Roadmap 66: the permission matrix is enforced by the same `Tools` dispatcher the
//! coordinator calls. These tests set both the project ceiling and the role allowance and then
//! ask the real dispatcher for a tool, so a widening role cannot pass by only looking right in
//! the settings table.

use std::sync::Arc;

use rigel_core::{
    AgentPermissionRow, BudgetService, Cancels, Coordinator, ProjectPermissions, ScriptedModel,
    SessionService, StopAt, SubagentPolicy, Tools, agent::roles, permissions::agent_tool_filter,
};
use serde_json::json;

async fn tools(root: &std::path::Path) -> Tools {
    let service = SessionService::open(root).await.unwrap();
    let cancels = Arc::new(Cancels::new());
    let budget = Arc::new(BudgetService::new(
        service.pool().clone(),
        cancels as Arc<dyn rigel_core::InterruptSink>,
    ));
    Tools::new(
        root.to_path_buf(),
        service,
        budget,
        root.join("index"),
        StopAt::Never,
    )
}

#[tokio::test]
async fn a_role_cannot_widen_the_project_grant_on_the_real_dispatch_path() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("README.md"), "the matrix\n").unwrap();
    ProjectPermissions::save(tmp.path(), &["read_file".to_string()]).unwrap();
    let tools = tools(tmp.path()).await;
    tools.restrict_to(roles::tools(Some("implementer")));

    // The implementer preset asks for edit_file and run_command, but the project ceiling holds
    // only read_file. The model neither sees nor can call the wider tools.
    let specs = tools.specs();
    assert!(specs.iter().any(|spec| spec.name == "read_file"));
    assert!(!specs.iter().any(|spec| spec.name == "edit_file"));
    assert!(!specs.iter().any(|spec| spec.name == "run_command"));

    let denied = tools
        .execute(
            "session",
            "agent",
            "run_command",
            json!({ "command": "echo should-not-run" }),
        )
        .await;
    assert!(!denied.ok);
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");
    assert!(
        denied.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("project permission matrix"),
        "{}",
        denied.body
    );

    // Even an explicitly wider role allowance is still intersected with the project grant.
    tools.restrict_to(Some(vec![
        "read_file".to_string(),
        "write_file".to_string(),
    ]));
    let widened = tools
        .execute(
            "session",
            "agent",
            "write_file",
            json!({ "path": "blocked.txt", "content": "no\n" }),
        )
        .await;
    assert!(!widened.ok);
    assert_eq!(widened.body["error"]["code"], "NOT_ALLOWED");
    assert!(!tmp.path().join("blocked.txt").exists());

    let readable = tools
        .execute(
            "session",
            "agent",
            "read_file",
            json!({ "path": "README.md" }),
        )
        .await;
    assert!(readable.ok, "{}", readable.body);
}

#[tokio::test]
async fn child_tool_sets_inherit_the_project_ceiling() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(tmp.path(), &["read_file".to_string()]).unwrap();
    let parent = tools(tmp.path()).await;
    let child = parent.writer(tmp.path().join("worktree"), vec!["README.md".to_string()]);
    child.restrict_to(Some(vec!["write_file".to_string()]));

    // A worktree has no `.rigel/permissions.json` of its own; the inherited ceiling must still
    // withhold the write rather than being replaced by a fresh all-tools default.
    assert!(!child.specs().iter().any(|spec| spec.name == "write_file"));
    let denied = child
        .execute(
            "session",
            "agent",
            "write_file",
            json!({ "path": "README.md", "content": "no\n" }),
        )
        .await;
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");
}

#[tokio::test]
async fn a_parent_cannot_delegate_a_tool_it_lacks() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("README.md"), "the matrix\n").unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &["read_file".to_string(), "list_files".to_string()],
    )
    .unwrap();
    let parent = tools(tmp.path()).await;
    parent.restrict_to(agent_tool_filter(None, Some(&["list_files".to_string()])));

    // The child's researcher role and the project both allow read_file, but the parent's
    // effective grant at spawn does not. The child must inherit the intersection.
    let child = parent.subagent();
    child.restrict_to(agent_tool_filter(Some("researcher"), None));
    assert!(!child.effective_grant().contains("read_file"));
    assert!(child.effective_grant().contains("list_files"));
    assert!(!child.specs().iter().any(|spec| spec.name == "read_file"));

    let denied = child
        .execute(
            "session",
            "agent",
            "read_file",
            json!({ "path": "README.md" }),
        )
        .await;
    assert!(!denied.ok);
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");
    assert!(
        denied.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("parent's effective grant at spawn"),
        "{}",
        denied.body
    );
}

#[tokio::test]
async fn a_child_with_a_broader_role_is_clamped_to_its_parent() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &[
            "read_file".to_string(),
            "list_files".to_string(),
            "write_file".to_string(),
        ],
    )
    .unwrap();
    let parent = tools(tmp.path()).await;
    parent.restrict_to(roles::tools(Some("reviewer")));

    // The reviewer parent cannot write. An implementer child asks for write and the project
    // grants it, but the child is still clamped to the parent's effective grant. The writable
    // child proves the denial comes from the parent, not from the read-only child surface.
    let child = parent.writer(tmp.path().join("worktree"), vec!["blocked.txt".to_string()]);
    child.restrict_to(roles::tools(Some("implementer")));
    assert!(child.effective_grant().contains("read_file"));
    assert!(!child.effective_grant().contains("write_file"));
    assert!(!child.specs().iter().any(|spec| spec.name == "write_file"));

    let denied = child
        .execute(
            "session",
            "agent",
            "write_file",
            json!({ "path": "blocked.txt", "content": "no\n" }),
        )
        .await;
    assert!(!denied.ok);
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");
    assert!(!tmp.path().join("blocked.txt").exists());
}

#[tokio::test]
async fn child_role_and_override_still_narrow_below_the_parent() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &[
            "read_file".to_string(),
            "list_files".to_string(),
            "write_file".to_string(),
        ],
    )
    .unwrap();
    let parent = tools(tmp.path()).await;
    let child = parent.writer(tmp.path().join("worktree"), vec!["blocked.txt".to_string()]);

    // The parent offers the whole project grant, but the child's override narrows it further.
    child.restrict_to(agent_tool_filter(
        Some("implementer"),
        Some(&["read_file".to_string()]),
    ));
    assert!(child.effective_grant().contains("read_file"));
    assert!(!child.effective_grant().contains("write_file"));
    assert!(!child.specs().iter().any(|spec| spec.name == "write_file"));

    // The child's role narrows below the parent as well.
    child.restrict_to(roles::tools(Some("reviewer")));
    assert!(child.effective_grant().contains("read_file"));
    assert!(!child.effective_grant().contains("write_file"));
}

#[tokio::test]
async fn an_agent_override_narrows_below_its_role_on_the_real_dispatch_path() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("README.md"), "the matrix\n").unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &[
            "read_file".to_string(),
            "list_files".to_string(),
            "edit_file".to_string(),
            "write_file".to_string(),
        ],
    )
    .unwrap();

    // `Tools::new` must read the project file itself: the direct API path builds tools this
    // way, outside a coordinator turn.
    let tools = tools(tmp.path()).await;
    tools.restrict_to(agent_tool_filter(
        Some("implementer"),
        Some(&["read_file".to_string(), "list_files".to_string()]),
    ));

    assert!(tools.specs().iter().any(|spec| spec.name == "read_file"));
    assert!(!tools.specs().iter().any(|spec| spec.name == "write_file"));
    let denied = tools
        .execute(
            "session",
            "agent",
            "write_file",
            json!({ "path": "blocked.txt", "content": "no\n" }),
        )
        .await;
    assert!(!denied.ok);
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");
    assert!(!tmp.path().join("blocked.txt").exists());

    // An override cannot add what the role withholds, even when it names every writable tool.
    tools.restrict_to(agent_tool_filter(
        Some("reviewer"),
        Some(&["write_file".to_string(), "edit_file".to_string()]),
    ));
    assert!(!tools.specs().iter().any(|spec| spec.name == "write_file"));
    let widened = tools
        .execute(
            "session",
            "agent",
            "write_file",
            json!({ "path": "widened.txt", "content": "no\n" }),
        )
        .await;
    assert_eq!(widened.body["error"]["code"], "NOT_ALLOWED");
    assert!(!tmp.path().join("widened.txt").exists());

    // An empty override is a denial, not a missing filter.
    tools.restrict_to(agent_tool_filter(Some("implementer"), Some(&[])));
    assert!(tools.specs().is_empty());
    let read = tools
        .execute(
            "session",
            "agent",
            "read_file",
            json!({ "path": "README.md" }),
        )
        .await;
    assert_eq!(read.body["error"]["code"], "NOT_ALLOWED");
}

#[tokio::test]
async fn the_coordinator_reads_an_agent_override_for_its_next_turn() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &["read_file".to_string(), "write_file".to_string()],
    )
    .unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("override turn"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("implementer"))
        .await
        .unwrap();
    service
        .set_agent_tool_overrides(&session.id, &agent.id, Some(&["read_file".to_string()]))
        .await
        .unwrap();

    let cancels = Arc::new(Cancels::new());
    let budget = Arc::new(BudgetService::new(
        service.pool().clone(),
        cancels.clone() as Arc<dyn rigel_core::InterruptSink>,
    ));
    let tools = tools(tmp.path()).await;
    let script = r#"{"steps":[
        {"tool_calls":[{"name":"write_file","arguments":{"path":"blocked.txt","content":"no\n"}}]},
        {"text":"done\n"}
    ]}"#;
    let coordinator = Coordinator {
        session: service.clone(),
        budget,
        tools,
        model: Arc::new(ScriptedModel::from_json(script).unwrap()),
        sink: Arc::new(rigel_core::NullSink),
        cancels,
        project_root: tmp.path().to_path_buf(),
        model_name: "script".to_string(),
        effort: None,
        max_steps: 4,
        subagents: SubagentPolicy::default(),
        depth: 0,
        parent_cancel: None,
        worktree_dir: None,
        chat_parent: None,
    };
    let run = coordinator
        .run(&session.id, &agent.id, "write the file")
        .await
        .unwrap();
    assert!(
        !tmp.path().join("blocked.txt").exists(),
        "the override denied the write even though the role and project allow it"
    );
    let items = service.items(&run.turn.id).await.unwrap();
    let denied = items
        .iter()
        .find(|item| item.body.get("tool").and_then(|tool| tool.as_str()) == Some("write_file"))
        .expect("the attempted write is recorded");
    assert_eq!(denied.body["result"]["error"]["code"], "NOT_ALLOWED");
}

#[tokio::test]
async fn an_agent_override_is_stored_and_reported_without_widening() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &[
            "read_file".to_string(),
            "edit_file".to_string(),
            "write_file".to_string(),
        ],
    )
    .unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("override"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("implementer"))
        .await
        .unwrap();
    assert_eq!(agent.tool_overrides, None);

    let narrowed = service
        .set_agent_tool_overrides(&session.id, &agent.id, Some(&["read_file".to_string()]))
        .await
        .unwrap();
    assert_eq!(narrowed.tool_overrides, Some(vec!["read_file".to_string()]));

    let matrix = ProjectPermissions::matrix(
        tmp.path(),
        &[AgentPermissionRow {
            id: agent.id.clone(),
            role: agent.role.clone(),
            tool_overrides: narrowed.tool_overrides.clone(),
        }],
    )
    .unwrap();
    let row = &matrix.agents[0];
    assert_eq!(row.effective, ["read_file"]);
    assert!(row.narrowed_by_override.contains(&"edit_file".to_string()));

    // A typo is refused, and clearing the override returns the agent to its role.
    assert!(
        service
            .set_agent_tool_overrides(&session.id, &agent.id, Some(&["not_a_tool".to_string()]),)
            .await
            .is_err()
    );
    let cleared = service
        .set_agent_tool_overrides(&session.id, &agent.id, None)
        .await
        .unwrap();
    assert_eq!(cleared.tool_overrides, None);
}

#[tokio::test]
async fn the_project_grant_is_a_ceiling_even_for_a_custom_role() {
    let tmp = tempfile::tempdir().unwrap();
    ProjectPermissions::save(
        tmp.path(),
        &["read_file".to_string(), "list_files".to_string()],
    )
    .unwrap();
    let tools = tools(tmp.path()).await;
    // No role preset means "custom": the old normal set would be every tool. The project grant
    // still removes every changing tool.
    tools.restrict_to(None);
    assert_eq!(
        tools
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>(),
        ["read_file", "list_files"]
    );

    let denied = tools
        .execute(
            "session",
            "agent",
            "run_command",
            json!({ "command": "echo blocked" }),
        )
        .await;
    assert_eq!(denied.body["error"]["code"], "NOT_ALLOWED");

    // `spawn_agents` is run by the coordinator, not `Tools::execute`, but it is a registered
    // tool and must be stopped by the same ceiling before that special dispatch.
    let spawn = tools
        .execute("session", "agent", "spawn_agents", json!({ "agents": [] }))
        .await;
    assert_eq!(spawn.body["error"]["code"], "NOT_ALLOWED");
    assert!(
        spawn.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("project permission matrix")
    );
}
