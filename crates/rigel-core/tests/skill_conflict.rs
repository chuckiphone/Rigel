//! Skill conflict detection (roadmap 151) integration tests.

use rigel_core::skills::conflict::{SkillSpec, detect, detect_all};

#[test]
fn duplicate_trigger_and_contradictory_rule() {
    let a = SkillSpec::new("a").trigger("pdf").rule("always log");
    let b = SkillSpec::new("b").trigger("pdf").rule("never log");
    let conflicts = detect(&a, &b);
    assert_eq!(conflicts.len(), 2);
    assert!(
        conflicts
            .iter()
            .any(|c| c.kind == rigel_core::skills::conflict::ConflictKind::DuplicateTrigger)
    );
    assert!(
        conflicts
            .iter()
            .any(|c| c.kind == rigel_core::skills::conflict::ConflictKind::ContradictoryRule)
    );

    let all = detect_all(&[a, b]);
    assert_eq!(all.len(), 2);
}
