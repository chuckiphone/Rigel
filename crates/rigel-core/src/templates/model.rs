//! Templates v2 (roadmap 94): variables, slots and per-agent sections.
//!
//! A v2 template extends the v1 shape with named variables, named slots that a
//! session can fill in, and a section of text per agent. A v1 template still
//! loads: every new field defaults, so an older file upgrades without a
//! migration. Changing a template produces a field-level diff so the change is
//! reviewable before it is used.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateAgentSection {
    pub agent_id: String,
    pub section: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateVariable {
    pub name: String,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateModel {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_steps: Option<u16>,
    #[serde(default)]
    pub agents: Vec<crate::templates::TemplateAgent>,
    #[serde(default)]
    pub skills: Vec<crate::templates::TemplateSkill>,
    #[serde(default)]
    pub prompt: String,
    pub created_at: i64,
    /// Named variables a session can substitute.
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// Named slots a session fills in.
    #[serde(default)]
    pub slots: Vec<String>,
    /// Per-agent sections of text.
    #[serde(default)]
    pub agent_sections: Vec<TemplateAgentSection>,
    /// Declared variables with defaults (v2 metadata).
    #[serde(default)]
    pub variable_defs: Vec<TemplateVariable>,
}

impl TemplateModel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            name: name.into(),
            model: None,
            effort: None,
            max_steps: None,
            agents: Vec::new(),
            skills: Vec::new(),
            prompt: String::new(),
            created_at: 0,
            variables: BTreeMap::new(),
            slots: Vec::new(),
            agent_sections: Vec::new(),
            variable_defs: Vec::new(),
        }
    }

    /// Upgrade a v1 template to a v2 model without losing data.
    pub fn from_v1(v1: &crate::templates::SessionTemplate) -> Self {
        Self {
            id: v1.id.clone(),
            name: v1.name.clone(),
            model: v1.model.clone(),
            effort: v1.effort.clone(),
            max_steps: v1.max_steps,
            agents: v1.agents.clone(),
            skills: v1.skills.clone(),
            prompt: v1.prompt.clone(),
            created_at: v1.created_at,
            variables: BTreeMap::new(),
            slots: Vec::new(),
            agent_sections: Vec::new(),
            variable_defs: Vec::new(),
        }
    }

    /// Project a v2 template back to the legacy shape for existing callers.
    pub fn to_v1(&self) -> crate::templates::SessionTemplate {
        crate::templates::SessionTemplate {
            id: self.id.clone(),
            name: self.name.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            max_steps: self.max_steps,
            agents: self.agents.clone(),
            skills: self.skills.clone(),
            prompt: self.prompt.clone(),
            created_at: self.created_at,
        }
    }

    /// Fill variables into the prompt, replacing `{{name}}` with the value.
    pub fn render_prompt(&self) -> String {
        let mut out = self.prompt.clone();
        for (name, value) in &self.variables {
            out = out.replace(&format!("{{{{{name}}}}}"), value);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Field-level diff between two templates.
pub fn diff(old: &TemplateModel, new: &TemplateModel) -> Vec<FieldDiff> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(FieldDiff {
            field: "name".to_string(),
            old: Some(old.name.clone()),
            new: Some(new.name.clone()),
        });
    }
    if old.model != new.model {
        out.push(FieldDiff {
            field: "model".to_string(),
            old: old.model.clone(),
            new: new.model.clone(),
        });
    }
    if old.effort != new.effort {
        out.push(FieldDiff {
            field: "effort".to_string(),
            old: old.effort.clone(),
            new: new.effort.clone(),
        });
    }
    if old.max_steps != new.max_steps {
        out.push(FieldDiff {
            field: "max_steps".to_string(),
            old: old.max_steps.map(|v| v.to_string()),
            new: new.max_steps.map(|v| v.to_string()),
        });
    }
    if old.agents != new.agents {
        out.push(FieldDiff {
            field: "agents".to_string(),
            old: Some(format!("{:?}", old.agents)),
            new: Some(format!("{:?}", new.agents)),
        });
    }
    if old.skills != new.skills {
        out.push(FieldDiff {
            field: "skills".to_string(),
            old: Some(format!("{:?}", old.skills)),
            new: Some(format!("{:?}", new.skills)),
        });
    }
    if old.prompt != new.prompt {
        out.push(FieldDiff {
            field: "prompt".to_string(),
            old: Some(old.prompt.clone()),
            new: Some(new.prompt.clone()),
        });
    }
    if old.variables != new.variables {
        out.push(FieldDiff {
            field: "variables".to_string(),
            old: Some(format!("{:?}", old.variables)),
            new: Some(format!("{:?}", new.variables)),
        });
    }
    if old.slots != new.slots {
        out.push(FieldDiff {
            field: "slots".to_string(),
            old: Some(format!("{:?}", old.slots)),
            new: Some(format!("{:?}", new.slots)),
        });
    }
    if old.agent_sections != new.agent_sections {
        out.push(FieldDiff {
            field: "agent_sections".to_string(),
            old: Some(format!("{:?}", old.agent_sections)),
            new: Some(format!("{:?}", new.agent_sections)),
        });
    }
    if old.variable_defs != new.variable_defs {
        out.push(FieldDiff {
            field: "variable_defs".to_string(),
            old: Some(format!("{:?}", old.variable_defs)),
            new: Some(format!("{:?}", new.variable_defs)),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::SessionTemplate;

    #[test]
    fn v1_template_loads_into_v2() {
        let v1 = SessionTemplate {
            id: "1".to_string(),
            name: "t".to_string(),
            model: Some("m".to_string()),
            effort: Some("high".to_string()),
            max_steps: Some(5),
            agents: Vec::new(),
            skills: Vec::new(),
            prompt: "hi".to_string(),
            created_at: 10,
        };
        let v2 = TemplateModel::from_v1(&v1);
        assert_eq!(v2.name, "t");
        assert_eq!(v2.model.as_deref(), Some("m"));
        assert!(v2.variables.is_empty());
        assert!(v2.slots.is_empty());
    }

    #[test]
    fn v2_deserializes_with_defaults_from_v1_json() {
        let json = r#"{"id":"1","name":"t","model":null,"effort":null,"max_steps":null,"agents":[],"skills":[],"prompt":"","created_at":1}"#;
        let m: TemplateModel = serde_json::from_str(json).unwrap();
        assert!(m.variables.is_empty());
        assert!(m.slots.is_empty());
        assert!(m.agent_sections.is_empty());
    }

    #[test]
    fn diff_reports_changed_fields() {
        let mut a = TemplateModel::new("t");
        a.model = Some("m1".to_string());
        let mut b = a.clone();
        b.model = Some("m2".to_string());
        b.prompt = "new".to_string();
        let d = diff(&a, &b);
        let fields: Vec<&str> = d.iter().map(|f| f.field.as_str()).collect();
        assert!(fields.contains(&"model"));
        assert!(fields.contains(&"prompt"));
        assert!(!fields.contains(&"name"));
    }

    #[test]
    fn no_diff_when_identical() {
        let a = TemplateModel::new("t");
        let b = a.clone();
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn variables_render_into_prompt() {
        let mut m = TemplateModel::new("t");
        m.prompt = "Hello {{name}}".to_string();
        m.variables.insert("name".to_string(), "world".to_string());
        assert_eq!(m.render_prompt(), "Hello world");
    }

    #[test]
    fn slots_and_agent_sections_round_trip() {
        let mut m = TemplateModel::new("t");
        m.slots.push("intro".to_string());
        m.agent_sections.push(TemplateAgentSection {
            agent_id: "a".to_string(),
            section: "do work".to_string(),
        });
        let json = serde_json::to_string(&m).unwrap();
        let back: TemplateModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }
}
