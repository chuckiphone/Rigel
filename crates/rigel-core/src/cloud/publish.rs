//! Roadmap 164: publication redaction preview.
//!
//! Publishing a session exposes a fixed set of rows. This module names every one of them before
//! the click, and makes tool output and file diffs *opt in*: the default scope redacts both. The
//! chosen scope is stored per session, so one session cannot silently change another's exposure.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::session::TurnTranscript;

/// What a session allows a publication to carry beyond its prompt and reply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishScope {
    /// Raw command and tool output. Off by default: it often holds paths and credentials.
    #[serde(default)]
    pub tool_output: bool,
    /// File diffs from `file_change` rows. Off by default: source is not for the phone.
    #[serde(default)]
    pub diffs: bool,
}

impl PublishScope {
    pub fn redaction(enabled: bool) -> &'static str {
        if enabled {
            "included"
        } else {
            "redacted by default"
        }
    }
}

/// One row a publication would write. A redacted row is still listed: the point of the preview is
/// that nothing is hidden from the reviewer, only from the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRow {
    pub kind: String,
    pub label: String,
    pub value: String,
    pub redacted: bool,
}

/// The complete, ordered list of rows for one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishPreview {
    pub session_id: String,
    pub scope: PublishScope,
    pub rows: Vec<PublishRow>,
}

impl PublishPreview {
    /// The names a reviewer can turn on. Both start off, so an unconfigured session is safe.
    pub fn redacted_kinds(&self) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|row| row.redacted)
            .map(|row| row.kind.as_str())
            .collect()
    }
}

fn body_text(value: &serde_json::Value, key: &str) -> String {
    value
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn reply_text(transcript: &TurnTranscript) -> String {
    transcript
        .items
        .iter()
        .filter(|item| item.kind == "message" || item.kind == "note")
        .map(|item| body_text(&item.body, "text"))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the preview from the same transcripts the publish path reads. Tool output and diffs are
/// always named; their values are placeholders until the session's scope opts in.
pub fn preview_rows(
    session_id: &str,
    transcripts: &[TurnTranscript],
    scope: PublishScope,
) -> PublishPreview {
    let mut rows = vec![
        PublishRow {
            kind: "session".to_string(),
            label: "Session header".to_string(),
            value: format!("session {session_id} Â· {} turn(s)", transcripts.len()),
            redacted: false,
        },
        PublishRow {
            kind: "scope".to_string(),
            label: "Project path and local files".to_string(),
            value: "not published".to_string(),
            redacted: true,
        },
    ];
    for transcript in transcripts {
        let turn = &transcript.turn;
        let prefix = format!("turn {}", turn.seq);
        rows.push(PublishRow {
            kind: "prompt".to_string(),
            label: format!("{prefix} Â· prompt"),
            value: turn.prompt.clone().unwrap_or_default(),
            redacted: false,
        });
        rows.push(PublishRow {
            kind: "reply".to_string(),
            label: format!("{prefix} Â· assistant reply"),
            value: reply_text(transcript),
            redacted: false,
        });
        let tool_items = transcript.items.iter().filter(|item| {
            matches!(
                item.kind.as_str(),
                "command" | "mcp_call" | "web_search" | "usage"
            )
        });
        for item in tool_items {
            let value = ["text", "tool", "command", "name"]
                .iter()
                .find_map(|key| {
                    let found = body_text(&item.body, key);
                    (!found.is_empty()).then_some(found)
                })
                .unwrap_or_else(|| item.kind.clone());
            rows.push(PublishRow {
                kind: "tool_output".to_string(),
                label: format!("{prefix} Â· {} output", item.kind),
                value: if scope.tool_output {
                    value
                } else {
                    "[tool output withheld]".to_string()
                },
                redacted: !scope.tool_output,
            });
        }
        for item in transcript
            .items
            .iter()
            .filter(|item| item.kind == "file_change")
        {
            let value = body_text(&item.body, "path");
            rows.push(PublishRow {
                kind: "diff".to_string(),
                label: format!(
                    "{prefix} Â· diff for {}",
                    if value.is_empty() { "a file" } else { &value }
                ),
                value: if scope.diffs {
                    value
                } else {
                    "[diff withheld]".to_string()
                },
                redacted: !scope.diffs,
            });
        }
        if let Some(usage) = &turn.usage {
            rows.push(PublishRow {
                kind: "usage".to_string(),
                label: format!("{prefix} Â· cost and tokens"),
                value: format!(
                    "{} micros Â· {} in Â· {} out",
                    usage.cost_micros, usage.input_tokens, usage.output_tokens
                ),
                redacted: false,
            });
        }
    }
    PublishPreview {
        session_id: session_id.to_string(),
        scope,
        rows,
    }
}

/// Scopes are keyed by session id, with an explicit default for sessions never configured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishScopes {
    #[serde(default)]
    scopes: BTreeMap<String, PublishScope>,
}

impl PublishScopes {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&text).map_err(|error| {
            format!(
                "the stored publication scopes are unreadable; reset them and try again: {error}"
            )
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(
            path,
            serde_json::to_string_pretty(self).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
    }

    /// An unconfigured session is safe: both switches are off.
    pub fn for_session(&self, session_id: &str) -> PublishScope {
        self.scopes.get(session_id).copied().unwrap_or_default()
    }

    pub fn set(&mut self, session_id: &str, scope: PublishScope) {
        self.scopes.insert(session_id.to_string(), scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Usage;
    use crate::session::{Item, Turn, TurnTranscript};

    fn transcript(seq: i64, with_output: bool) -> TurnTranscript {
        let mut items = vec![Item {
            id: format!("m{seq}"),
            turn_id: format!("t{seq}"),
            seq: 1,
            kind: "message".to_string(),
            status: "ok".to_string(),
            body: serde_json::json!({ "text": "the assistant's own reply" }),
            created_at: 0,
            raw: None,
        }];
        if with_output {
            items.push(Item {
                id: format!("c{seq}"),
                turn_id: format!("t{seq}"),
                seq: 2,
                kind: "command".to_string(),
                status: "ok".to_string(),
                body: serde_json::json!({ "text": "secret command output", "arguments": { "path": "private.txt" } }),
                created_at: 0,
                raw: None,
            });
            items.push(Item {
                id: format!("f{seq}"),
                turn_id: format!("t{seq}"),
                seq: 3,
                kind: "file_change".to_string(),
                status: "ok".to_string(),
                body: serde_json::json!({ "path": "src/private.rs", "diff": "private source" }),
                created_at: 0,
                raw: None,
            });
        }
        TurnTranscript {
            turn: Turn {
                id: format!("t{seq}"),
                agent_id: "a".to_string(),
                parent_turn_id: None,
                seq,
                status: "completed".to_string(),
                prompt: Some("why is the build slow?".to_string()),
                started_at: 0,
                completed_at: Some(1),
                usage: Some(Usage {
                    cost_micros: 123,
                    ..Usage::default()
                }),
                stop_reason: None,
                images: Vec::new(),
            },
            items,
        }
    }

    #[test]
    fn the_preview_lists_every_row_and_defaults_tool_output_and_diffs_off() {
        let preview = preview_rows("s1", &[transcript(1, true)], PublishScope::default());
        assert_eq!(preview.scope, PublishScope::default());
        assert!(preview.rows.iter().any(|row| row.kind == "session"));
        assert!(preview.rows.iter().any(|row| row.kind == "scope"));
        assert!(
            preview
                .rows
                .iter()
                .any(|row| row.kind == "prompt" && row.value.contains("build slow"))
        );
        assert!(
            preview
                .rows
                .iter()
                .any(|row| row.kind == "reply" && row.value.contains("assistant's own reply"))
        );
        assert!(preview.rows.iter().any(|row| row.kind == "usage"));
        let tool = preview
            .rows
            .iter()
            .find(|row| row.kind == "tool_output")
            .unwrap();
        assert_eq!(tool.value, "[tool output withheld]");
        assert!(tool.redacted);
        let diff = preview.rows.iter().find(|row| row.kind == "diff").unwrap();
        assert_eq!(diff.value, "[diff withheld]");
        assert!(diff.redacted);
        assert_eq!(
            preview.redacted_kinds(),
            vec!["scope", "tool_output", "diff"]
        );
    }

    #[test]
    fn opting_in_reveals_only_the_requested_kind() {
        let scope = PublishScope {
            tool_output: true,
            diffs: false,
        };
        let preview = preview_rows("s1", &[transcript(1, true)], scope);
        let tool = preview
            .rows
            .iter()
            .find(|row| row.kind == "tool_output")
            .unwrap();
        assert_eq!(tool.value, "secret command output");
        assert!(!tool.redacted);
        let diff = preview.rows.iter().find(|row| row.kind == "diff").unwrap();
        assert_eq!(diff.value, "[diff withheld]");
        assert!(diff.redacted);
    }

    #[test]
    fn scopes_persist_per_session_without_leaking_to_another() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("publish-scopes.json");
        let mut scopes = PublishScopes::default();
        assert_eq!(scopes.for_session("one"), PublishScope::default());
        scopes.set(
            "one",
            PublishScope {
                tool_output: true,
                diffs: false,
            },
        );
        scopes.save(&path).unwrap();
        let stored = PublishScopes::load(&path).unwrap();
        assert!(stored.for_session("one").tool_output);
        assert_eq!(stored.for_session("two"), PublishScope::default());
        assert_eq!(stored, scopes);
    }
}
