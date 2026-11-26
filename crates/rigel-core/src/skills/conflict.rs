//! Skill conflict detection (roadmap 151).
//!
//! Before two skills are pinned together their triggers and rules are compared.
//! A trigger claimed by both skills is a `DuplicateTrigger`, and two rules that
//! say opposite things about the same subject are a `ContradictoryRule`. Each
//! conflict names the skills and the offending trigger or rule so it can be
//! resolved by a person rather than merged silently.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSpec {
    pub id: String,
    pub triggers: Vec<String>,
    pub rules: Vec<String>,
}

impl SkillSpec {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            triggers: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn trigger(mut self, trigger: impl Into<String>) -> Self {
        self.triggers.push(trigger.into());
        self
    }

    pub fn rule(mut self, rule: impl Into<String>) -> Self {
        self.rules.push(rule.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    DuplicateTrigger,
    ContradictoryRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub left_skill: String,
    pub right_skill: String,
    pub detail: String,
}

impl Conflict {
    pub fn duplicate_trigger(left: &str, right: &str, trigger: &str) -> Self {
        Self {
            kind: ConflictKind::DuplicateTrigger,
            left_skill: left.to_string(),
            right_skill: right.to_string(),
            detail: format!("duplicate trigger: {trigger}"),
        }
    }

    pub fn contradictory_rule(left: &str, right: &str, left_rule: &str, right_rule: &str) -> Self {
        Self {
            kind: ConflictKind::ContradictoryRule,
            left_skill: left.to_string(),
            right_skill: right.to_string(),
            detail: format!("contradictory rules: {left_rule:?} vs {right_rule:?}"),
        }
    }
}

/// Detect conflicts between two skills.
pub fn detect(a: &SkillSpec, b: &SkillSpec) -> Vec<Conflict> {
    let mut out = Vec::new();

    // Duplicate triggers (case-insensitive, trimmed).
    for ta in &a.triggers {
        for tb in &b.triggers {
            if ta.trim().eq_ignore_ascii_case(tb.trim()) {
                out.push(Conflict::duplicate_trigger(&a.id, &b.id, ta.trim()));
            }
        }
    }

    // Contradictory rules.
    for ra in &a.rules {
        for rb in &b.rules {
            if rules_contradict(ra, rb) {
                out.push(Conflict::contradictory_rule(&a.id, &b.id, ra, rb));
            }
        }
    }

    out
}

/// Detect conflicts across a set of skills (every pair).
pub fn detect_all(skills: &[SkillSpec]) -> Vec<Conflict> {
    let mut out = Vec::new();
    for i in 0..skills.len() {
        for j in (i + 1)..skills.len() {
            out.extend(detect(&skills[i], &skills[j]));
        }
    }
    out
}

fn strip_polarity(rule: &str) -> (bool, String) {
    let r = rule.trim().to_lowercase();
    let prefixes = ["not ", "no ", "never ", "deny ", "disallow ", "forbid "];
    for p in prefixes {
        if let Some(rest) = r.strip_prefix(p) {
            return (true, rest.trim().to_string());
        }
    }
    let positive_prefixes = ["always ", "allow ", "permit ", "require "];
    for p in positive_prefixes {
        if let Some(rest) = r.strip_prefix(p) {
            return (false, rest.trim().to_string());
        }
    }
    (false, r)
}

fn rules_contradict(a: &str, b: &str) -> bool {
    let (pa, ba) = strip_polarity(a);
    let (pb, bb) = strip_polarity(b);
    // Same subject, opposite polarity.
    !ba.is_empty() && ba == bb && pa != pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_trigger_is_a_named_conflict() {
        let a = SkillSpec::new("a").trigger("pdf");
        let b = SkillSpec::new("b").trigger("pdf");
        let conflicts = detect(&a, &b);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::DuplicateTrigger);
        assert!(conflicts[0].detail.contains("pdf"));
    }

    #[test]
    fn triggers_are_case_insensitive() {
        let a = SkillSpec::new("a").trigger("PDF");
        let b = SkillSpec::new("b").trigger("pdf");
        assert!(!detect(&a, &b).is_empty());
    }

    #[test]
    fn distinct_triggers_do_not_conflict() {
        let a = SkillSpec::new("a").trigger("pdf");
        let b = SkillSpec::new("b").trigger("json");
        assert!(detect(&a, &b).is_empty());
    }

    #[test]
    fn a_contradictory_rule_is_a_named_conflict() {
        let a = SkillSpec::new("a").rule("always log errors");
        let b = SkillSpec::new("b").rule("never log errors");
        let conflicts = detect(&a, &b);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::ContradictoryRule);
    }

    #[test]
    fn negation_contradiction_is_detected() {
        let a = SkillSpec::new("a").rule("allow network");
        let b = SkillSpec::new("b").rule("deny network");
        assert!(!detect(&a, &b).is_empty());
    }

    #[test]
    fn same_polarity_rules_do_not_conflict() {
        let a = SkillSpec::new("a").rule("always log errors");
        let b = SkillSpec::new("b").rule("always log errors");
        assert!(detect(&a, &b).is_empty());
    }

    #[test]
    fn detect_all_covers_every_pair() {
        let skills = vec![
            SkillSpec::new("a").trigger("x"),
            SkillSpec::new("b").trigger("x"),
            SkillSpec::new("c").trigger("y"),
        ];
        let conflicts = detect_all(&skills);
        assert_eq!(conflicts.len(), 1);
    }
}
