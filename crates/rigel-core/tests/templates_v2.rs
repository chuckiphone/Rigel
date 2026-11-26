//! Templates v2 (roadmap 94) integration tests.

use rigel_core::templates::model::{TemplateModel, diff};

#[test]
fn v1_loads_and_diff_works() {
    let v1_json = r#"{"id":"1","name":"t","model":null,"effort":null,"max_steps":null,"agents":[],"skills":[],"prompt":"hi","created_at":1}"#;
    let m: TemplateModel = serde_json::from_str(v1_json).unwrap();
    assert!(m.variables.is_empty());

    let mut a = TemplateModel::new("t");
    let mut b = a.clone();
    b.slots.push("intro".to_string());
    let d = diff(&a, &b);
    assert!(!d.is_empty());
    assert_eq!(d[0].field, "slots");
}
