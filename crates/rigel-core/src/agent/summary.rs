//! One compact summary per finished turn, with provenance to the items it covers (roadmap 57).
//!
//! A turn summary is derived state: the transcript remains the source of truth. The coordinator
//! writes it when the turn closes, and two readers consume it instead of walking the transcript
//! again — compaction joins the stored text for the turns it drops, and the palette searches the
//! same row, so a long tool result is findable through its summary without scanning every item.
//!
//! The summary is extractive and deterministic: the prompt, each readable assistant or user
//! message, command, file change, MCP call and error, one line each and bounded. No model call is
//! involved, which keeps a turn's close independent of a provider and makes the row testable.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::migrate::now_ms;
use crate::session::{Item, Turn, TurnTranscript};

/// The whole summary is bounded so it cannot recreate the transcript it stands for.
pub const MAX_SUMMARY_CHARS: usize = 800;
/// One prompt line is bounded; the full prompt stays on the turn.
pub const MAX_PROMPT_CHARS: usize = 240;
/// One item line is bounded; the full body stays in `items`.
pub const MAX_LINE_CHARS: usize = 220;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TurnSummary {
    pub turn_id: String,
    pub session_id: String,
    pub summary: String,
    /// The items that contributed a line, in transcript order.
    pub item_ids: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryDraft {
    pub summary: String,
    pub item_ids: Vec<String>,
}

/// Build one bounded, single-line summary from a turn and its items.
pub fn build(turn: &Turn, items: &[Item]) -> SummaryDraft {
    let mut lines = Vec::new();
    let mut item_ids = Vec::new();
    let mut used = 0usize;

    if let Some(prompt) = turn
        .prompt
        .as_deref()
        .map(one_line)
        .filter(|p| !p.is_empty())
    {
        let line = format!("User: {}", clip(&prompt, MAX_PROMPT_CHARS));
        used += line.chars().count();
        lines.push(line);
    }

    for item in items {
        let Some(line) = line_for(item) else {
            continue;
        };
        if lines.last() == Some(&line) {
            continue;
        }
        let addition = line.chars().count() + usize::from(!lines.is_empty()) * 3;
        if used + addition > MAX_SUMMARY_CHARS {
            break;
        }
        used += addition;
        lines.push(line);
        item_ids.push(item.id.clone());
    }

    let summary = if lines.is_empty() {
        format!(
            "Turn {} (seq {}) recorded no prompt or readable items.",
            turn.id, turn.seq
        )
    } else {
        lines.join(" · ")
    };
    SummaryDraft { summary, item_ids }
}

fn line_for(item: &Item) -> Option<String> {
    let body = &item.body;
    match item.kind.as_str() {
        "message" => {
            let text = text(body, "text")?;
            let role = body
                .get("role")
                .and_then(|role| role.as_str())
                .unwrap_or("assistant");
            let who = if role == "user" { "User" } else { "Assistant" };
            Some(format!("{who}: {}", clip(&text, MAX_LINE_CHARS)))
        }
        "error" => {
            let message = text(body, "message")?;
            Some(format!("Error: {}", clip(&message, MAX_LINE_CHARS)))
        }
        "note" => {
            let note = text(body, "text")?;
            Some(format!("User note: {}", clip(&note, MAX_LINE_CHARS)))
        }
        "command" => {
            let command = text(body, "command")?;
            let result = result_text(body)
                .map(|result| format!(" -> {}", clip(&result, 120)))
                .unwrap_or_default();
            Some(format!("Ran: {}{result}", clip(&command, 120)))
        }
        "file_change" => {
            let path = text(body, "path")?;
            Some(format!("Changed: {}", clip(&path, MAX_LINE_CHARS)))
        }
        "mcp_call" => {
            let tool = text(body, "tool").or_else(|| text(body, "name"))?;
            let result = result_text(body)
                .map(|result| format!(" -> {}", clip(&result, 120)))
                .unwrap_or_default();
            Some(format!("Called: {}{result}", clip(&tool, 120)))
        }
        _ => None,
    }
}

fn text(body: &serde_json::Value, key: &str) -> Option<String> {
    let value = body.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A bounded, one-line window into a tool result. `content` is the common MCP shape; a plain
/// string and a JSON body are both readable, so a search for an outcome still finds the turn.
fn result_text(body: &serde_json::Value) -> Option<String> {
    let result = body.get("result").or_else(|| body.get("output"))?;
    if let Some(value) = result.as_str() {
        let value = value.trim();
        return (!value.is_empty()).then(|| value.to_string());
    }
    if let Some(value) = result.get("content").and_then(|value| value.as_str()) {
        let value = value.trim();
        return (!value.is_empty()).then(|| value.to_string());
    }
    serde_json::to_string(result).ok()
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(value: &str, limit: usize) -> String {
    let flat = one_line(value);
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut out = flat
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

/// Store a turn's summary, replacing the row if the turn is closed again. One turn, one row.
pub async fn store(
    pool: &SqlitePool,
    session_id: &str,
    turn: &Turn,
    items: &[Item],
) -> Result<TurnSummary, sqlx::Error> {
    let draft = build(turn, items);
    let created_at = now_ms();
    let item_ids_json = serde_json::to_string(&draft.item_ids).unwrap_or_else(|_| "[]".to_string());
    sqlx::query(
        "INSERT INTO turn_summaries (turn_id, session_id, summary, item_ids_json, created_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(turn_id) DO UPDATE SET
             session_id = excluded.session_id,
             summary = excluded.summary,
             item_ids_json = excluded.item_ids_json,
             created_at = excluded.created_at",
    )
    .bind(&turn.id)
    .bind(session_id)
    .bind(&draft.summary)
    .bind(&item_ids_json)
    .bind(created_at)
    .execute(pool)
    .await?;

    Ok(TurnSummary {
        turn_id: turn.id.clone(),
        session_id: session_id.to_string(),
        summary: draft.summary,
        item_ids: draft.item_ids,
        created_at,
    })
}

pub async fn for_turn(
    pool: &SqlitePool,
    turn_id: &str,
) -> Result<Option<TurnSummary>, sqlx::Error> {
    let row: Option<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT turn_id, session_id, summary, item_ids_json, created_at
         FROM turn_summaries WHERE turn_id = ?",
    )
    .bind(turn_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(summary_from_row))
}

pub async fn for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TurnSummary>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT s.turn_id, s.session_id, s.summary, s.item_ids_json, s.created_at
         FROM turn_summaries s JOIN turns t ON t.id = s.turn_id
         WHERE s.session_id = ? ORDER BY t.started_at, t.seq",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(summary_from_row).collect())
}

/// Read a turn's stored summary, building and storing it once for a turn that predates the table.
pub async fn ensure(
    pool: &SqlitePool,
    session_id: &str,
    turn: &Turn,
    items: &[Item],
) -> Result<TurnSummary, sqlx::Error> {
    if let Some(found) = for_turn(pool, &turn.id).await? {
        return Ok(found);
    }
    store(pool, session_id, turn, items).await
}

/// The summaries compaction needs, in one session read plus one write per pre-migration turn.
/// A finished turn normally already has its row, so this is a lookup, not a second derivation.
pub async fn ensure_history(
    pool: &SqlitePool,
    session_id: &str,
    history: &[TurnTranscript],
) -> Result<HashMap<String, TurnSummary>, sqlx::Error> {
    let mut summaries = for_session(pool, session_id)
        .await?
        .into_iter()
        .map(|summary| (summary.turn_id.clone(), summary))
        .collect::<HashMap<_, _>>();
    for transcript in history {
        if !summaries.contains_key(&transcript.turn.id) {
            let stored = store(pool, session_id, &transcript.turn, &transcript.items).await?;
            summaries.insert(transcript.turn.id.clone(), stored);
        }
    }
    Ok(summaries)
}

fn summary_from_row(row: (String, String, String, String, i64)) -> TurnSummary {
    TurnSummary {
        turn_id: row.0,
        session_id: row.1,
        summary: row.2,
        item_ids: serde_json::from_str(&row.3).unwrap_or_default(),
        created_at: row.4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Usage;
    use crate::session::{Item, Turn};
    use tempfile::TempDir;

    fn turn(prompt: Option<&str>) -> Turn {
        Turn {
            id: "t-1".to_string(),
            agent_id: "a-1".to_string(),
            parent_turn_id: None,
            seq: 1,
            status: "completed".to_string(),
            prompt: prompt.map(str::to_string),
            started_at: 1,
            completed_at: Some(2),
            usage: Some(Usage::default()),
            stop_reason: Some("completed".to_string()),
        }
    }

    fn item(id: &str, seq: i64, kind: &str, body: serde_json::Value) -> Item {
        Item {
            id: id.to_string(),
            turn_id: "t-1".to_string(),
            seq,
            kind: kind.to_string(),
            status: "completed".to_string(),
            body,
            created_at: seq,
            raw: None,
        }
    }

    #[test]
    fn a_summary_names_the_prompt_answer_tools_and_provenance() {
        let items = vec![
            item(
                "i-1",
                1,
                "message",
                serde_json::json!({ "role": "assistant", "text": "the crash is in the lexer" }),
            ),
            item(
                "i-2",
                2,
                "command",
                serde_json::json!({ "command": "cargo test -p parser" }),
            ),
            item(
                "i-3",
                3,
                "file_change",
                serde_json::json!({ "path": "crates/parser/src/lexer.rs" }),
            ),
            item(
                "i-4",
                4,
                "error",
                serde_json::json!({ "message": "assertion failed" }),
            ),
            item("i-5", 5, "usage", serde_json::json!({ "input_tokens": 10 })),
        ];
        let draft = build(&turn(Some("fix the parser crash")), &items);
        assert!(draft.summary.contains("User: fix the parser crash"));
        assert!(
            draft
                .summary
                .contains("Assistant: the crash is in the lexer")
        );
        assert!(draft.summary.contains("Ran: cargo test -p parser"));
        assert!(
            draft
                .summary
                .contains("Changed: crates/parser/src/lexer.rs")
        );
        assert!(draft.summary.contains("Error: assertion failed"));
        assert!(!draft.summary.contains('\n'));
        assert_eq!(
            draft.item_ids,
            vec!["i-1", "i-2", "i-3", "i-4"],
            "only items that contributed a line are provenance"
        );
    }

    #[test]
    fn the_summary_is_bounded_and_drops_the_items_it_could_not_name() {
        let items = (0..100)
            .map(|index| {
                item(
                    &format!("i-{index}"),
                    index,
                    "message",
                    serde_json::json!({ "text": "x".repeat(500) }),
                )
            })
            .collect::<Vec<_>>();
        let draft = build(&turn(Some("a very long prompt")), &items);
        assert!(draft.summary.chars().count() <= MAX_SUMMARY_CHARS);
        assert!(!draft.summary.contains('\n'));
        assert!(!draft.item_ids.is_empty());
        assert!(draft.item_ids.len() < items.len());
    }

    #[tokio::test]
    async fn a_finished_turn_stores_one_summary_and_provenance() {
        let tmp = TempDir::new().unwrap();
        let service = crate::session::SessionService::open(tmp.path())
            .await
            .unwrap();
        let session = service.start(None, None).await.unwrap();
        let agent = service
            .register_agent(&session.id, "native", None)
            .await
            .unwrap();
        let turn = service
            .append_turn(&agent.id, Some("fix the parser crash"))
            .await
            .unwrap();
        let message = service
            .append_item(
                &turn.id,
                "message",
                "completed",
                &serde_json::json!({ "role": "assistant", "text": "the crash is in the lexer" }),
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        let command = service
            .append_item(
                &turn.id,
                "command",
                "completed",
                &serde_json::json!({ "command": "cargo test -p parser" }),
                &serde_json::json!({}),
            )
            .await
            .unwrap();

        service.finish_turn(&turn.id, "completed").await.unwrap();

        let stored = for_turn(service.pool(), &turn.id)
            .await
            .unwrap()
            .expect("closing a turn stores its summary");
        assert!(stored.summary.contains("the crash is in the lexer"));
        assert!(stored.summary.contains("cargo test -p parser"));
        assert_eq!(stored.item_ids, vec![message.id.clone(), command.id]);

        // Closing again replaces the one row rather than adding a second.
        service.finish_turn(&turn.id, "completed").await.unwrap();
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM turn_summaries WHERE turn_id = ?")
            .bind(&turn.id)
            .fetch_one(service.pool())
            .await
            .unwrap();
        assert_eq!(count.0, 1);
        assert_eq!(
            for_session(service.pool(), &session.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
