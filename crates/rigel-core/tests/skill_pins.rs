//! Skill pins in templates (roadmap 99) integration tests.

use rigel_core::templates::skill_pins::{SkillPin, SkillPins};

#[test]
fn pins_report_changed_and_inherit() {
    let mut pins = SkillPins::new();
    pins.pin("pdf", "abc");
    assert!(SkillPin::new("pdf", "abc").changed("def"));
    assert!(!SkillPin::new("pdf", "abc").changed("abc"));

    let inherited = pins.inherit();
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].body_sha, "abc");
}
