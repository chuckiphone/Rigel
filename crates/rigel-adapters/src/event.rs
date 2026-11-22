//! One event vocabulary for every harness adapter.
//!
//! Each adapter still decodes its own wire protocol, but a renderer must not care whether a line
//! came from Claude, Codex, Muse or one of the three ACP agents. [`HarnessEvent`] is the single
//! surface every adapter can hand a transcript, and [`StreamRenderer`] is the one renderer the
//! CLI uses for the streaming harnesses. Neither has a branch on a harness name: the event says
//! what happened, and the renderer says how that kind of event is shown.
//!
//! Conversions are deliberately lossless:
//!
//! - Claude and Codex `Started` become a session id plus the transport that reported it;
//! - message and thought deltas become [`HarnessEvent::Delta`] with an [`ItemKind`];
//! - ACP tool calls become ordinary [`NormalItem`]s, and their later status update becomes
//!   [`HarnessEvent::ToolCallUpdate`];
//! - an approval is either requested, or resolved with the request when the adapter has it;
//! - every adapter keeps its `Notice`, `Item`, `Usage` and unmapped-event behaviour.

use std::collections::HashMap;

use rigel_core::ItemKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::acp::{AcpEvent, ApprovalDecision, ApprovalRequest};
use crate::claude::ClaudeEvent;
use crate::codex::CodexEvent;
use crate::muse::MuseEvent;
use crate::normalise::NormalItem;

/// One normalised harness event.
///
/// The variant names are the stable fixture vocabulary. A field may be absent because the
/// protocol does not report it (Claude has no transport event, Muse has no start event), but a
/// renderer never has to guess which harness produced the event to understand its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HarnessEvent {
    /// The harness opened its own conversation. `session_id` is the vendor's id (Claude's
    /// session, Codex's thread), not Rigel's session id.
    Started {
        session_id: String,
        model: Option<String>,
        transport: Option<String>,
    },
    /// A streamed piece of text. `kind` is `Message` for answer text and `Reasoning` for a
    /// thought stream; other kinds are not emitted as deltas.
    Delta { kind: ItemKind, text: String },
    /// The final assistant message, when the harness reports one separately from its deltas.
    Message { text: String },
    /// A warning or status line. Not fatal.
    Notice { message: String },
    /// A tool call, file change, plan, search or other item already sorted into one of the ten
    /// kinds. `status` is the protocol's status when it has one (ACP tool calls).
    Item {
        item: NormalItem,
        status: Option<String>,
    },
    /// A later status/text update for a tool call. The item kind is recovered from the
    /// `tool_call_id` seen on the matching [`HarnessEvent::Item`].
    ToolCallUpdate {
        tool_call_id: String,
        status: Option<String>,
        text: Option<String>,
    },
    /// A plan reported as its own protocol update rather than a tool item.
    Plan { entries: Vec<String> },
    /// ACP's context-window usage update. It is not a billed input/output split.
    Usage {
        used: u64,
        size: u64,
        cost_micros: Option<i64>,
    },
    /// A harness paused and asked for consent.
    ApprovalRequested { request: ApprovalRequest },
    /// A decision was recorded. Codex attaches the request it answered; ACP's permission
    /// resolution only carries the id, because its request was already emitted.
    ApprovalResolved {
        request: Option<ApprovalRequest>,
        tool_call_id: String,
        decision: ApprovalDecision,
    },
    /// A valid protocol update Rigel does not model yet. Kept, never silently dropped.
    Unmapped { kind: String },
}

impl From<ClaudeEvent> for HarnessEvent {
    fn from(event: ClaudeEvent) -> Self {
        match event {
            ClaudeEvent::Started { session_id, model } => Self::Started {
                session_id,
                model,
                transport: None,
            },
            ClaudeEvent::Delta { text } => Self::Delta {
                kind: ItemKind::Message,
                text,
            },
            ClaudeEvent::Message { text } => Self::Message { text },
            ClaudeEvent::Notice { message } => Self::Notice { message },
            ClaudeEvent::Item(item) => Self::Item { item, status: None },
        }
    }
}

impl From<CodexEvent> for HarnessEvent {
    fn from(event: CodexEvent) -> Self {
        match event {
            CodexEvent::Started {
                thread_id,
                transport,
            } => Self::Started {
                session_id: thread_id,
                model: None,
                transport: Some(transport.as_str().to_string()),
            },
            CodexEvent::Delta { text } => Self::Delta {
                kind: ItemKind::Message,
                text,
            },
            CodexEvent::Message { text } => Self::Message { text },
            CodexEvent::Notice { message } => Self::Notice { message },
            CodexEvent::Item(item) => Self::Item { item, status: None },
            CodexEvent::Approval { request, decision } => Self::ApprovalResolved {
                tool_call_id: request.tool_call_id.clone(),
                request: Some(request),
                decision,
            },
        }
    }
}

impl From<MuseEvent> for HarnessEvent {
    fn from(event: MuseEvent) -> Self {
        match event {
            MuseEvent::Delta { text } => Self::Delta {
                kind: ItemKind::Message,
                text,
            },
            MuseEvent::Item(item) => Self::Item { item, status: None },
            MuseEvent::Notice { message } => Self::Notice { message },
        }
    }
}

impl From<AcpEvent> for HarnessEvent {
    fn from(event: AcpEvent) -> Self {
        match event {
            AcpEvent::MessageDelta { text } => Self::Delta {
                kind: ItemKind::Message,
                text,
            },
            AcpEvent::ThoughtDelta { text } => Self::Delta {
                kind: ItemKind::Reasoning,
                text,
            },
            AcpEvent::ToolCall {
                tool_call_id,
                title,
                kind,
                item_kind,
                operation,
                status,
                raw_input,
            } => Self::Item {
                item: NormalItem::new(
                    item_kind,
                    json!({
                        "tool_call_id": tool_call_id,
                        "title": title,
                        "kind": kind,
                        "operation": operation,
                        "raw_input": raw_input,
                    }),
                ),
                status,
            },
            AcpEvent::ToolCallUpdate {
                tool_call_id,
                status,
                text,
            } => Self::ToolCallUpdate {
                tool_call_id,
                status,
                text,
            },
            AcpEvent::Plan { entries } => Self::Plan { entries },
            AcpEvent::Usage {
                used,
                size,
                cost_micros,
            } => Self::Usage {
                used,
                size,
                cost_micros,
            },
            AcpEvent::Notice { message } => Self::Notice { message },
            AcpEvent::PermissionRequested { request } => Self::ApprovalRequested { request },
            AcpEvent::PermissionResolved {
                tool_call_id,
                decision,
            } => Self::ApprovalResolved {
                request: None,
                tool_call_id,
                decision,
            },
            AcpEvent::Unmapped { kind } => Self::Unmapped { kind },
        }
    }
}

/// The transcript fields a streaming renderer needs. It is stateful because a tool-call update
/// inherits the kind of the call that opened it, exactly as the ACP renderer does.
#[derive(Debug, Default)]
pub struct StreamRenderer {
    /// True once answer text has been emitted, so a caller can add one final newline.
    pub streamed: bool,
    pub text: String,
    pub reasoning: String,
    pub items: Vec<NormalItem>,
    pub notices: Vec<String>,
    pub approvals: Vec<ApprovalRecord>,
    pub usage: Option<HarnessUsage>,
    kinds: HashMap<String, ItemKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub tool_call_id: String,
    pub title: String,
    pub kind: Option<String>,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessUsage {
    pub used: u64,
    pub size: u64,
    pub cost_micros: Option<i64>,
}

/// One piece of output. `Text` is written without a newline so deltas read as a stream; `Line` is
/// a complete status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderLine {
    Text(String),
    Line(String),
    /// An approval the caller should format with its own decision vocabulary. Keeping the
    /// record rather than a preformatted string preserves the CLI's existing `allow_session`
    /// wording while the event itself stays harness-neutral.
    Approval(ApprovalRecord),
}

impl StreamRenderer {
    /// Add a notice the adapter did not emit itself (for example Muse's usage caveat), keeping
    /// the same de-duplication as the event path.
    pub fn push_notice(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.notices.contains(&message) {
            self.notices.push(message);
        }
    }

    /// Consume one event, update the transcript, and return the output a terminal should print.
    ///
    /// The match is over event variants only. There is no harness parameter and no harness-name
    /// branch, so a new adapter that emits these events gets the same rendering for free.
    pub fn apply(&mut self, event: HarnessEvent) -> Vec<RenderLine> {
        match event {
            HarnessEvent::Started { .. }
            | HarnessEvent::Message { .. }
            | HarnessEvent::Plan { .. } => Vec::new(),
            HarnessEvent::Delta { kind, text } => {
                if kind == ItemKind::Message {
                    self.streamed = true;
                    self.text.push_str(&text);
                    vec![RenderLine::Text(text)]
                } else {
                    if kind == ItemKind::Reasoning {
                        self.reasoning.push_str(&text);
                    }
                    Vec::new()
                }
            }
            HarnessEvent::Notice { message } => {
                self.push_notice(message);
                Vec::new()
            }
            HarnessEvent::Item { item, status } => {
                if let Some(id) = item.body.get("tool_call_id").and_then(Value::as_str) {
                    self.kinds.insert(id.to_string(), item.kind);
                }
                let _ = status;
                self.items.push(item);
                Vec::new()
            }
            HarnessEvent::ToolCallUpdate {
                tool_call_id,
                status,
                text,
            } => {
                let label = self
                    .kinds
                    .get(&tool_call_id)
                    .copied()
                    .map(ItemKind::as_str)
                    .unwrap_or("tool");
                vec![RenderLine::Line(format!(
                    "  [{}] {label}: {}",
                    status.as_deref().unwrap_or("update"),
                    text.as_deref().unwrap_or_default()
                ))]
            }
            HarnessEvent::Usage {
                used,
                size,
                cost_micros,
            } => {
                self.usage = Some(HarnessUsage {
                    used,
                    size,
                    cost_micros,
                });
                Vec::new()
            }
            HarnessEvent::ApprovalRequested { .. } => Vec::new(),
            HarnessEvent::ApprovalResolved {
                request,
                tool_call_id,
                decision,
            } => {
                let (title, kind) = match &request {
                    Some(request) => (request.title.clone(), request.kind.clone()),
                    None => (tool_call_id.clone(), None),
                };
                self.approvals.push(ApprovalRecord {
                    tool_call_id: tool_call_id.clone(),
                    title: title.clone(),
                    kind: kind.clone(),
                    decision,
                });
                if request.is_some() {
                    vec![RenderLine::Approval(ApprovalRecord {
                        tool_call_id: tool_call_id.clone(),
                        title,
                        kind,
                        decision,
                    })]
                } else {
                    Vec::new()
                }
            }
            HarnessEvent::Unmapped { kind } => {
                self.push_notice(format!("unmapped event: {kind}"));
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_adapter_converts_into_the_same_vocabulary() {
        let claude: HarnessEvent = ClaudeEvent::Delta {
            text: "hello".into(),
        }
        .into();
        let codex: HarnessEvent = CodexEvent::Delta {
            text: "hello".into(),
        }
        .into();
        let muse: HarnessEvent = MuseEvent::Delta {
            text: "hello".into(),
        }
        .into();
        let acp: HarnessEvent = AcpEvent::MessageDelta {
            text: "hello".into(),
        }
        .into();
        assert_eq!(claude, codex);
        assert_eq!(claude, muse);
        assert_eq!(claude, acp);
        assert_eq!(
            claude,
            HarnessEvent::Delta {
                kind: ItemKind::Message,
                text: "hello".into()
            }
        );
    }

    #[test]
    fn the_renderer_has_no_harness_branch() {
        // The same event from every adapter produces the same line, and the renderer's API has
        // no harness argument at all. This is the property item 49 asks for: rendering is driven
        // by the event, not by who produced it.
        let events = [
            HarnessEvent::from(ClaudeEvent::Delta { text: "one".into() }),
            HarnessEvent::from(CodexEvent::Delta { text: "one".into() }),
            HarnessEvent::from(MuseEvent::Delta { text: "one".into() }),
            HarnessEvent::from(AcpEvent::MessageDelta { text: "one".into() }),
        ];
        for event in events {
            let mut renderer = StreamRenderer::default();
            assert_eq!(
                renderer.apply(event),
                vec![RenderLine::Text("one".to_string())]
            );
            assert_eq!(renderer.text, "one");
            assert!(renderer.streamed);
        }
    }

    #[test]
    fn a_codex_approval_is_a_resolved_event_and_a_renamed_line() {
        let request = ApprovalRequest {
            tool_call_id: "c2".into(),
            title: "git push --force origin main".into(),
            kind: Some("command".into()),
            raw_input: None,
            options: Vec::new(),
        };
        let event: HarnessEvent = CodexEvent::Approval {
            request,
            decision: ApprovalDecision::RejectOnce,
        }
        .into();
        let mut renderer = StreamRenderer::default();
        let lines = renderer.apply(event);
        assert_eq!(
            lines,
            vec![RenderLine::Approval(ApprovalRecord {
                tool_call_id: "c2".into(),
                title: "git push --force origin main".into(),
                kind: Some("command".into()),
                decision: ApprovalDecision::RejectOnce,
            })]
        );
        assert_eq!(
            renderer.approvals,
            vec![ApprovalRecord {
                tool_call_id: "c2".into(),
                title: "git push --force origin main".into(),
                kind: Some("command".into()),
                decision: ApprovalDecision::RejectOnce,
            }]
        );
    }

    #[test]
    fn an_acp_tool_update_inherits_the_kind_of_its_call() {
        let mut renderer = StreamRenderer::default();
        let call: HarnessEvent = AcpEvent::ToolCall {
            tool_call_id: "call-3".into(),
            title: "Run tests".into(),
            kind: Some("execute".into()),
            item_kind: ItemKind::Command,
            operation: "execute",
            status: Some("pending".into()),
            raw_input: Some(json!({ "command": "cargo test" })),
        }
        .into();
        assert!(renderer.apply(call).is_empty());
        let update: HarnessEvent = AcpEvent::ToolCallUpdate {
            tool_call_id: "call-3".into(),
            status: Some("completed".into()),
            text: Some("102 passed".into()),
        }
        .into();
        assert_eq!(
            renderer.apply(update),
            vec![RenderLine::Line("  [completed] command: 102 passed".into())]
        );
    }

    #[test]
    fn an_item_event_keeps_its_body_and_status_boundary() {
        let event: HarnessEvent = AcpEvent::ToolCall {
            tool_call_id: "call-4".into(),
            title: "Delete build".into(),
            kind: Some("delete".into()),
            item_kind: ItemKind::FileChange,
            operation: "delete",
            status: Some("pending".into()),
            raw_input: None,
        }
        .into();
        let HarnessEvent::Item { item, status } = event else {
            panic!("an ACP tool call is an item");
        };
        assert_eq!(item.kind, ItemKind::FileChange);
        assert_eq!(item.body["tool_call_id"], "call-4");
        assert_eq!(item.body["operation"], "delete");
        assert_eq!(status.as_deref(), Some("pending"));
    }
}
