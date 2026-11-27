use std::collections::BTreeMap;

use rigel_core::{SessionTemplates, TemplateAgentSection, TemplateModel, TemplateVariable};

#[test]
fn v1_file_loads_through_v2_and_v2_fields_persist() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionTemplates::open(tmp.path());
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(
        store.path(),
        r#"[{"id":"legacy","name":"Legacy","model":"m","effort":null,"max_steps":5,"agents":[],"skills":[],"prompt":"hello","created_at":7}]"#,
    )
    .unwrap();

    let legacy = store.list_v2().unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].prompt, "hello");
    assert!(legacy[0].variables.is_empty());
    assert!(legacy[0].slots.is_empty());

    let mut upgraded = legacy[0].clone();
    upgraded.variables = BTreeMap::from([("name".to_string(), "Ada".to_string())]);
    upgraded.slots.push("intro".to_string());
    upgraded.agent_sections.push(TemplateAgentSection {
        agent_id: "writer".to_string(),
        section: "Focus on correctness".to_string(),
    });
    upgraded.variable_defs.push(TemplateVariable {
        name: "name".to_string(),
        default: Some("friend".to_string()),
    });
    store.save_v2(upgraded.clone()).unwrap();

    let reopened = SessionTemplates::open(tmp.path());
    assert_eq!(reopened.list_v2().unwrap(), vec![upgraded]);
    assert_eq!(reopened.list().unwrap()[0].name, "Legacy");
}

#[test]
fn preview_reports_changes_without_persisting_them() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionTemplates::open(tmp.path());
    let original = store.save_v2(TemplateModel::new("Review")).unwrap();
    let mut candidate = original.clone();
    candidate.prompt = "New instructions".to_string();
    candidate.slots.push("context".to_string());

    let changes = store.preview_save(&candidate).unwrap();
    assert_eq!(
        changes
            .iter()
            .map(|change| change.field.as_str())
            .collect::<Vec<_>>(),
        vec!["prompt", "slots"]
    );
    assert_eq!(store.list_v2().unwrap(), vec![original]);
}

#[test]
fn legacy_save_preserves_v2_metadata_for_same_template() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionTemplates::open(tmp.path());
    let mut model = TemplateModel::new("Shared");
    model.slots.push("context".to_string());
    let saved = store.save_v2(model).unwrap();
    let mut legacy = saved.to_v1();
    legacy.prompt = "Updated prompt".to_string();
    store.save(legacy).unwrap();

    let reopened = store.list_v2().unwrap();
    assert_eq!(reopened[0].slots, vec!["context"]);
    assert_eq!(reopened[0].prompt, "Updated prompt");
}
