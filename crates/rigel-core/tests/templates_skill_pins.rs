//! Roadmap 99: a template captures the body hash of each pinned skill, and a session created
//! from it inherits the exact id/hash pairs. A moved body is reported as changed, never resolved
//! to the current one in silence.

use std::collections::BTreeMap;

use rigel_core::{SessionTemplates, TemplateError, TemplateModel, TemplateSkill};

#[test]
fn a_template_captures_validated_pins_and_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    let templates = SessionTemplates::open(dir.path());
    let pdf_hash = "a".repeat(64);
    let json_hash = "b".repeat(64);

    let mut model = TemplateModel::new("pinned setup");
    model.skills = vec![
        TemplateSkill {
            skill_id: "z-pdf".to_string(),
            body_sha: pdf_hash.clone(),
        },
        TemplateSkill {
            skill_id: "a-json".to_string(),
            body_sha: json_hash.clone(),
        },
    ];
    let saved = templates.save_v2(model).unwrap();
    assert_eq!(saved.skills.len(), 2);
    assert_eq!(
        saved.skills[0].skill_id, "a-json",
        "pins are stored in stable order"
    );

    let loaded = templates.list_v2().unwrap();
    assert_eq!(loaded.len(), 1);
    let pins = loaded[0].skill_pins();
    let inherited = pins.skills();
    assert_eq!(inherited[0].skill_id, "a-json");
    assert_eq!(inherited[0].body_sha, json_hash);
    assert_eq!(inherited[1].skill_id, "z-pdf");
    assert_eq!(inherited[1].body_sha, pdf_hash);

    let mut current = BTreeMap::new();
    current.insert("a-json".to_string(), json_hash.clone());
    current.insert("z-pdf".to_string(), "c".repeat(64));
    assert_eq!(pins.changed_pins(&current), vec!["z-pdf".to_string()]);
}

#[test]
fn an_invalid_pin_is_refused_before_the_template_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let templates = SessionTemplates::open(dir.path());
    let mut model = TemplateModel::new("broken pin");
    model.skills = vec![TemplateSkill {
        skill_id: "pdf".to_string(),
        body_sha: "not-a-sha256".to_string(),
    }];
    assert!(matches!(
        templates.save_v2(model),
        Err(TemplateError::InvalidPin(_))
    ));
    assert!(
        templates.list_v2().unwrap().is_empty(),
        "nothing was persisted"
    );
}
