//! Roadmap 179: three-way metadata conflicts and the decisions that resolve them.
//!
//! Cloud metadata (the shelf, the session scope and the publish preferences) can be edited on two
//! machines between syncs. Each field keeps the value both sides started from (`base`), the local
//! value (`ours`) and the cloud value (`theirs`). A field is resolved only by an explicit choice
//! recorded with the author and the time; publishing refuses to run while any field is unresolved.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::migrate::now_ms;

/// One field with the three values a reviewer can compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataField {
    pub name: String,
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

/// The side a reviewer chose for one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataChoice {
    Base,
    Ours,
    Theirs,
}

impl MetadataChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Ours => "ours",
            Self::Theirs => "theirs",
        }
    }
}

/// A decision is evidence: who made it, when, and which side they picked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeDecision {
    pub field: String,
    pub choice: MetadataChoice,
    pub author: String,
    /// Unix milliseconds, from the same clock the other ledgers use.
    pub decided_at: i64,
}

/// The persisted state: the fields under review and the decisions made so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataView {
    #[serde(default)]
    pub fields: Vec<MetadataField>,
    #[serde(default)]
    pub decisions: Vec<MergeDecision>,
}

impl MetadataView {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&text).map_err(|error| {
            format!(
                "the stored metadata decisions are unreadable; clear them and try again: {error}"
            )
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, text).map_err(|error| error.to_string())
    }

    /// Fields that actually disagree. A field only changed on one side needs no decision.
    pub fn conflicts(&self) -> Vec<&MetadataField> {
        self.fields
            .iter()
            .filter(|field| field.ours != field.theirs)
            .collect()
    }

    /// The names of the conflicts that still need a decision.
    pub fn unresolved(&self) -> Vec<String> {
        self.conflicts()
            .into_iter()
            .filter(|field| {
                !self
                    .decisions
                    .iter()
                    .any(|decision| decision.field == field.name)
            })
            .map(|field| field.name.clone())
            .collect()
    }

    /// Publishing is allowed only when every conflicted field has a recorded choice.
    pub fn can_publish(&self) -> Result<(), Vec<String>> {
        let unresolved = self.unresolved();
        if unresolved.is_empty() {
            Ok(())
        } else {
            Err(unresolved)
        }
    }

    /// Record or replace a decision. The author must be a real identity, never a blank string.
    pub fn decide(
        &mut self,
        field: &str,
        choice: MetadataChoice,
        author: &str,
    ) -> Result<MergeDecision, String> {
        let author = author.trim();
        if author.is_empty() {
            return Err("a merge decision needs the author's identity".to_string());
        }
        if !self.fields.iter().any(|candidate| candidate.name == field) {
            return Err(format!("{field} is not a field under review"));
        }
        let decision = MergeDecision {
            field: field.to_string(),
            choice,
            author: author.to_string(),
            decided_at: now_ms(),
        };
        self.decisions.retain(|current| current.field != field);
        self.decisions.push(decision.clone());
        Ok(decision)
    }

    /// The value a resolved field will carry, for the UI and the publish path to agree on.
    pub fn resolved_value(&self, field: &MetadataField) -> Option<String> {
        let choice = self
            .decisions
            .iter()
            .find(|decision| decision.field == field.name)?
            .choice;
        Some(match choice {
            MetadataChoice::Base => field.base.clone(),
            MetadataChoice::Ours => field.ours.clone(),
            MetadataChoice::Theirs => field.theirs.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, base: &str, ours: &str, theirs: &str) -> MetadataField {
        MetadataField {
            name: name.to_string(),
            base: base.to_string(),
            ours: ours.to_string(),
            theirs: theirs.to_string(),
        }
    }

    fn view() -> MetadataView {
        MetadataView {
            fields: vec![
                field("publish_scope", "turns", "all", "sessions"),
                field("shelf_scope", "skills", "skills", "skills"),
            ],
            decisions: Vec::new(),
        }
    }

    #[test]
    fn only_disagreeing_fields_are_conflicts_and_unresolved_fields_block_publishing() {
        let mut view = view();
        assert_eq!(view.conflicts().len(), 1);
        assert_eq!(
            view.can_publish().unwrap_err(),
            vec!["publish_scope".to_string()]
        );
        let decision = view
            .decide(
                "publish_scope",
                MetadataChoice::Theirs,
                " reviewer@example.com ",
            )
            .unwrap();
        assert_eq!(decision.author, "reviewer@example.com");
        assert!(decision.decided_at > 0);
        assert!(view.can_publish().is_ok());
        assert_eq!(
            view.resolved_value(&view.fields[0]),
            Some("sessions".to_string())
        );
    }

    #[test]
    fn a_decision_needs_a_real_author_and_a_known_field() {
        let mut view = view();
        assert!(
            view.decide("publish_scope", MetadataChoice::Ours, "  ")
                .is_err()
        );
        assert!(
            view.decide("unknown", MetadataChoice::Ours, "reviewer")
                .is_err()
        );
        assert!(view.decisions.is_empty());
    }

    #[test]
    fn decisions_persist_with_author_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloud-metadata-decisions.json");
        let mut view = view();
        view.decide(
            "publish_scope",
            MetadataChoice::Ours,
            "reviewer@example.com",
        )
        .unwrap();
        view.save(&path).unwrap();

        let stored = MetadataView::load(&path).unwrap();
        assert_eq!(stored, view);
        assert_eq!(stored.decisions[0].author, "reviewer@example.com");
        assert_eq!(stored.decisions[0].choice.as_str(), "ours");
        assert!(stored.can_publish().is_ok());
        assert!(
            MetadataView::load(&dir.path().join("absent.json"))
                .unwrap()
                .fields
                .is_empty()
        );
    }
}
