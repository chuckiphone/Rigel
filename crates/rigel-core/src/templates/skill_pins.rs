//! Skill pins in templates (roadmap 99).
//!
//! A template pins the body hash of every skill it uses, so a session created from
//! the template inherits the exact version the original session read. When the
//! skill's body later moves, the pin reports `changed` instead of silently
//! resolving to the new body.

use serde::{Deserialize, Serialize};

/// A skill pinned to a template by its body hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPin {
    pub skill_id: String,
    pub body_sha: String,
}

impl SkillPin {
    pub fn new(skill_id: impl Into<String>, body_sha: impl Into<String>) -> Self {
        Self {
            skill_id: skill_id.into(),
            body_sha: body_sha.into(),
        }
    }

    /// `true` when the skill's current body hash differs from the pinned one.
    pub fn changed(&self, current_hash: &str) -> bool {
        self.body_sha != current_hash
    }
}

/// A collection of skill pins, keyed by skill id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPins {
    pub pins: Vec<SkillPin>,
}

impl SkillPins {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_template_skills(skills: &[crate::templates::TemplateSkill]) -> Self {
        Self {
            pins: skills
                .iter()
                .map(|s| SkillPin {
                    skill_id: s.skill_id.clone(),
                    body_sha: s.body_sha.clone(),
                })
                .collect(),
        }
    }

    /// Validate and normalise the pins a template is about to store. One pin per skill id, with
    /// the exact 64-character lowercase SHA-256 the index writes; a bad hash is an error, not a
    /// pin that silently resolves to whatever body happens to be current later.
    pub fn try_from_skills(skills: &[crate::templates::TemplateSkill]) -> Result<Self, String> {
        let mut pins = Self::new();
        for skill in skills {
            if skill.skill_id.trim().is_empty() {
                return Err("a template pin needs a skill id".to_string());
            }
            if skill.body_sha.len() != 64
                || !skill
                    .body_sha
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(format!(
                    "{} has an invalid body hash; expected 64 lowercase hex characters",
                    skill.skill_id
                ));
            }
            if pins.get(&skill.skill_id).is_some() {
                return Err(format!("duplicate template pin for {}", skill.skill_id));
            }
            pins.pin(skill.skill_id.clone(), skill.body_sha.clone());
        }
        pins.pins
            .sort_by(|left, right| left.skill_id.cmp(&right.skill_id));
        Ok(pins)
    }

    /// The pins as template skills, so a new session can inherit the exact id/hash pairs.
    pub fn skills(&self) -> Vec<crate::templates::TemplateSkill> {
        self.pins
            .iter()
            .map(|pin| crate::templates::TemplateSkill {
                skill_id: pin.skill_id.clone(),
                body_sha: pin.body_sha.clone(),
            })
            .collect()
    }

    pub fn pin(&mut self, skill_id: impl Into<String>, body_sha: impl Into<String>) {
        let skill_id = skill_id.into();
        let body_sha = body_sha.into();
        if let Some(existing) = self.pins.iter_mut().find(|p| p.skill_id == skill_id) {
            existing.body_sha = body_sha;
        } else {
            self.pins.push(SkillPin { skill_id, body_sha });
        }
    }

    pub fn unpin(&mut self, skill_id: &str) {
        self.pins.retain(|p| p.skill_id != skill_id);
    }

    pub fn get(&self, skill_id: &str) -> Option<&SkillPin> {
        self.pins.iter().find(|p| p.skill_id == skill_id)
    }

    /// Report every pin whose current hash differs from the pinned hash.
    pub fn changed_pins(
        &self,
        current: &std::collections::BTreeMap<String, String>,
    ) -> Vec<String> {
        self.pins
            .iter()
            .filter(|p| {
                current
                    .get(&p.skill_id)
                    .map(|h| p.changed(h))
                    .unwrap_or(false)
            })
            .map(|p| p.skill_id.clone())
            .collect()
    }

    /// Inherit pins into a new session: returns the pinned skill ids and hashes.
    pub fn inherit(&self) -> Vec<SkillPin> {
        self.pins.clone()
    }
}

/// Alias for callers that expect the template skill type name.
pub type TemplateSkillPin = SkillPin;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn pinning_records_the_body_hash() {
        let mut pins = SkillPins::new();
        pins.pin("pdf", "abc123");
        assert_eq!(pins.get("pdf").unwrap().body_sha, "abc123");
    }

    #[test]
    fn a_moved_hash_reports_changed() {
        let pin = SkillPin::new("pdf", "abc123");
        assert!(pin.changed("def456"));
        assert!(!pin.changed("abc123"));
    }

    #[test]
    fn changed_pins_lists_only_moved_hashes() {
        let mut pins = SkillPins::new();
        pins.pin("pdf", "abc");
        pins.pin("json", "xyz");
        let mut current = BTreeMap::new();
        current.insert("pdf".to_string(), "abc".to_string()); // unchanged
        current.insert("json".to_string(), "moved".to_string()); // changed
        let changed = pins.changed_pins(&current);
        assert_eq!(changed, vec!["json".to_string()]);
    }

    #[test]
    fn a_new_session_inherits_the_pins() {
        let mut pins = SkillPins::new();
        pins.pin("pdf", "abc");
        let inherited = pins.inherit();
        assert_eq!(inherited.len(), 1);
        assert_eq!(inherited[0].skill_id, "pdf");
        assert_eq!(inherited[0].body_sha, "abc");
    }

    #[test]
    fn from_template_skills_copies_pins() {
        let skills = vec![crate::templates::TemplateSkill {
            skill_id: "pdf".to_string(),
            body_sha: "abc".to_string(),
        }];
        let pins = SkillPins::from_template_skills(&skills);
        assert_eq!(pins.pins.len(), 1);
    }

    #[test]
    fn try_from_skills_validates_and_sorts_pins() {
        let hash = "a".repeat(64);
        let skills = vec![
            crate::templates::TemplateSkill {
                skill_id: "z".to_string(),
                body_sha: hash.clone(),
            },
            crate::templates::TemplateSkill {
                skill_id: "a".to_string(),
                body_sha: hash.clone(),
            },
        ];
        let pins = SkillPins::try_from_skills(&skills).unwrap();
        assert_eq!(
            pins.skills()
                .iter()
                .map(|s| s.skill_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert_eq!(pins.skills()[0].body_sha, hash);

        let bad = vec![crate::templates::TemplateSkill {
            skill_id: "pdf".to_string(),
            body_sha: "ABC".to_string(),
        }];
        assert!(SkillPins::try_from_skills(&bad).is_err());
        let duplicate = vec![
            crate::templates::TemplateSkill {
                skill_id: "pdf".to_string(),
                body_sha: hash.clone(),
            },
            crate::templates::TemplateSkill {
                skill_id: "pdf".to_string(),
                body_sha: hash,
            },
        ];
        assert!(SkillPins::try_from_skills(&duplicate).is_err());
    }

    #[test]
    fn a_template_hands_its_pins_to_the_new_session() {
        let hash = "b".repeat(64);
        let template = crate::templates::TemplateModel {
            id: "t".to_string(),
            name: "setup".to_string(),
            model: None,
            effort: None,
            max_steps: None,
            agents: Vec::new(),
            skills: vec![crate::templates::TemplateSkill {
                skill_id: "pdf".to_string(),
                body_sha: hash.clone(),
            }],
            prompt: String::new(),
            created_at: 1,
            variables: Default::default(),
            slots: Vec::new(),
            agent_sections: Vec::new(),
            variable_defs: Vec::new(),
        };
        let pins = template.skill_pins();
        let inherited = pins.skills();
        assert_eq!(inherited.len(), 1);
        assert_eq!(inherited[0].skill_id, "pdf");
        assert_eq!(inherited[0].body_sha, hash);
        let mut current = std::collections::BTreeMap::new();
        current.insert("pdf".to_string(), "c".repeat(64));
        assert_eq!(pins.changed_pins(&current), vec!["pdf".to_string()]);
    }

    #[test]
    fn unpin_removes_a_pin() {
        let mut pins = SkillPins::new();
        pins.pin("pdf", "abc");
        pins.unpin("pdf");
        assert!(pins.get("pdf").is_none());
    }
}
