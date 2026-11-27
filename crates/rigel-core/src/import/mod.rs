//! Import Claude and Codex line-oriented conversation histories.
//!
//! Parsing and reporting are deliberately separate from persistence: callers can inspect every
//! source row in a dry run, and no database write occurs until `import_history` is called.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Value, json};

use crate::session::{SessionError, SessionService};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistorySource {
    Claude,
    Codex,
}

impl HistorySource {
    fn harness(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowDisposition {
    Import,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RowReport {
    pub row: usize,
    pub disposition: RowDisposition,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub source: HistorySource,
    pub dry_run: bool,
    pub session_id: Option<String>,
    pub rows: Vec<RowReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("history contains no valid message rows to import")]
    NoImportableRows,
    #[error("failed to initialize imported turn")]
    TurnNotInitialized,
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug)]
struct ParsedRow {
    source_id: Option<String>,
    role: String,
    kind: String,
    body: Value,
    text: Option<String>,
    timestamp: Option<i64>,
}

/// Report all importable and skipped source rows without touching the database.
pub fn dry_run(input: &str, source: HistorySource) -> ImportReport {
    let (_, rows) = parse(input, source);
    ImportReport {
        source,
        dry_run: true,
        session_id: None,
        rows,
    }
}

/// Import supported rows into a newly created local session.
pub async fn import_history(
    sessions: &SessionService,
    input: &str,
    source: HistorySource,
    title: Option<&str>,
) -> Result<ImportReport, ImportError> {
    let (events, rows) = parse(input, source);
    if events.is_empty() {
        return Err(ImportError::NoImportableRows);
    }
    let source_start = events.iter().filter_map(|event| event.timestamp).min();
    let source_end = events.iter().filter_map(|event| event.timestamp).max();
    let session = sessions
        .start(title.or(Some("Imported history")), None)
        .await?;
    let agent = sessions
        .register_agent(&session.id, source.harness(), Some("researcher"))
        .await?;
    let mut active_turn: Option<String> = None;
    let mut active_completion_at: Option<i64> = None;

    for event in events {
        if event.role == "user" {
            if let Some(turn_id) = active_turn.take() {
                sessions.finish_turn(&turn_id, "completed").await?;
                if let Some(timestamp) = active_completion_at.take() {
                    sqlx::query("UPDATE turns SET completed_at = ? WHERE id = ?")
                        .bind(timestamp)
                        .bind(&turn_id)
                        .execute(sessions.pool())
                        .await?;
                }
            }
            let turn = sessions
                .append_turn(&agent.id, event.text.as_deref())
                .await?;
            if let Some(timestamp) = event.timestamp {
                sqlx::query("UPDATE turns SET started_at = ? WHERE id = ?")
                    .bind(timestamp)
                    .bind(&turn.id)
                    .execute(sessions.pool())
                    .await?;
            }
            active_turn = Some(turn.id);
            active_completion_at = event.timestamp;
        } else {
            if active_turn.is_none() {
                active_turn = Some(sessions.append_turn(&agent.id, None).await?.id);
            }
            let turn_id = active_turn
                .as_deref()
                .ok_or(ImportError::TurnNotInitialized)?;
            let status = if event.kind == "error" { "error" } else { "ok" };
            let item = sessions
                .append_item(turn_id, &event.kind, status, &event.body, &event.body)
                .await?;
            if let Some(timestamp) = event.timestamp {
                sqlx::query("UPDATE items SET created_at = ? WHERE id = ?")
                    .bind(timestamp)
                    .bind(&item.id)
                    .execute(sessions.pool())
                    .await?;
                active_completion_at = Some(timestamp);
            }
        }
    }
    if let Some(turn_id) = active_turn {
        sessions.finish_turn(&turn_id, "completed").await?;
        if let Some(timestamp) = active_completion_at {
            sqlx::query("UPDATE turns SET completed_at = ? WHERE id = ?")
                .bind(timestamp)
                .bind(&turn_id)
                .execute(sessions.pool())
                .await?;
        }
    }
    if source_start.is_some() || source_end.is_some() {
        sqlx::query("UPDATE sessions SET created_at = COALESCE(?, created_at), status = 'ended', ended_at = COALESCE(?, ended_at) WHERE id = ?")
            .bind(source_start)
            .bind(source_end)
            .bind(&session.id)
            .execute(sessions.pool())
            .await?;
    } else {
        sessions.end(&session.id).await?;
    }
    Ok(ImportReport {
        source,
        dry_run: false,
        session_id: Some(session.id),
        rows,
    })
}

fn parse(input: &str, source: HistorySource) -> (Vec<ParsedRow>, Vec<RowReport>) {
    let mut events = Vec::new();
    let mut reports = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in input.lines().enumerate() {
        let row_number = index + 1;
        if line.trim().is_empty() {
            reports.push(skip(row_number, "blank row"));
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                reports.push(skip(row_number, format!("invalid JSON: {error}")));
                continue;
            }
        };
        let candidates = normalize_row(&value, source);
        if candidates.is_empty() {
            reports.push(skip(row_number, "unsupported or non-message row"));
            continue;
        }
        let mut accepted = 0;
        let mut duplicate = 0;
        for mut candidate in candidates {
            if candidate.role != "user" && candidate.role != "assistant" && candidate.role != "tool"
            {
                continue;
            }
            if let Some(id) = &candidate.source_id
                && !seen.insert(id.clone())
            {
                duplicate += 1;
                continue;
            }
            if candidate.role == "tool" {
                candidate.kind = "tool".to_string();
            }
            events.push(candidate);
            accepted += 1;
        }
        if accepted > 0 {
            reports.push(RowReport {
                row: row_number,
                disposition: RowDisposition::Import,
                reason: format!("{accepted} message(s) accepted"),
            });
        } else if duplicate > 0 {
            reports.push(skip(
                row_number,
                format!("duplicate source row ({duplicate} message(s))"),
            ));
        } else {
            reports.push(skip(row_number, "invalid or unsupported message role"));
        }
    }
    (events, reports)
}

fn normalize_row(value: &Value, source: HistorySource) -> Vec<ParsedRow> {
    let row_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let payload = value.get("payload").unwrap_or(value);
    if source == HistorySource::Codex && row_type == "response_item" {
        return normalize_codex_payload(payload, value);
    }

    // Claude Code stores one `user` or `assistant` message per JSONL row.
    let message = value.get("message").unwrap_or(value);
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");
    let id = message
        .get("id")
        .or_else(|| value.get("uuid"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let timestamp = value
        .get("timestamp")
        .and_then(parse_timestamp)
        .or_else(|| message.get("timestamp").and_then(parse_timestamp));
    let content = message.get("content").unwrap_or(&Value::Null);
    if let Some(blocks) = content.as_array() {
        let mut out = Vec::new();
        for (block_index, block) in blocks.iter().enumerate() {
            let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
            let (mapped_role, kind, text) = match block_type {
                "text" => (
                    role.to_string(),
                    if role == "assistant" {
                        "answer"
                    } else {
                        "message"
                    },
                    block.get("text").and_then(Value::as_str).map(str::to_owned),
                ),
                "tool_use" => ("tool".to_string(), "tool", None),
                "tool_result" => (
                    "tool".to_string(),
                    "tool_result",
                    block
                        .get("content")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                ),
                _ => continue,
            };
            out.push(ParsedRow {
                source_id: id.as_ref().map(|id| format!("{id}:{block_index}")),
                role: mapped_role,
                kind: kind.to_string(),
                body: block.clone(),
                text,
                timestamp,
            });
        }
        return out;
    }
    let text = content
        .as_str()
        .or_else(|| message.get("text").and_then(Value::as_str))
        .map(str::to_owned);
    if role.is_empty() || text.as_deref().is_none_or(str::is_empty) {
        return Vec::new();
    }
    vec![ParsedRow {
        source_id: id,
        role: role.to_string(),
        kind: if role == "assistant" {
            "answer"
        } else {
            "message"
        }
        .to_string(),
        body: json!({"role": role, "content": content, "text": text}),
        text,
        timestamp,
    }]
}

fn normalize_codex_payload(payload: &Value, outer: &Value) -> Vec<ParsedRow> {
    match payload.get("type").and_then(Value::as_str).unwrap_or("") {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let content = payload.get("content").unwrap_or(&Value::Null);
            let text = content.as_str().map(str::to_owned).or_else(|| {
                content.as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            });
            let Some(text) = text.filter(|text| !text.is_empty()) else {
                return Vec::new();
            };
            vec![ParsedRow {
                source_id: outer.get("id").and_then(Value::as_str).map(str::to_owned),
                role: role.to_string(),
                kind: if role == "assistant" {
                    "answer"
                } else {
                    "message"
                }
                .to_string(),
                body: payload.clone(),
                text: Some(text),
                timestamp: outer.get("timestamp").and_then(parse_timestamp),
            }]
        }
        "function_call" | "function_call_output" => vec![ParsedRow {
            source_id: outer.get("id").and_then(Value::as_str).map(str::to_owned),
            role: "tool".to_string(),
            kind: "tool".to_string(),
            body: payload.clone(),
            text: None,
            timestamp: outer.get("timestamp").and_then(parse_timestamp),
        }],
        _ => Vec::new(),
    }
}

fn parse_timestamp(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let text = value.as_str()?;
        text.parse::<i64>().ok().or_else(|| parse_rfc3339_ms(text))
    })
}

fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        std::str::from_utf8(bytes.get(range)?)
            .ok()?
            .parse::<i64>()
            .ok()
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if bytes.get(10) != Some(&b'T') || month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let mut offset = 0i64;
    let mut pos = 19;
    let mut millis = 0i64;
    if bytes.get(pos) == Some(&b'.') {
        pos += 1;
        let start = pos;
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        let fraction = std::str::from_utf8(bytes.get(start..pos)?).ok()?;
        let digits = fraction.chars().take(3).collect::<String>();
        millis = digits.parse::<i64>().ok()? * 10i64.pow(3u32.saturating_sub(digits.len() as u32));
    }
    match bytes.get(pos).copied()? {
        b'Z' | b'z' => {}
        sign @ (b'+' | b'-') => {
            let hours = number(pos + 1..pos + 3)?;
            let minutes = number(pos + 4..pos + 6)?;
            if bytes.get(pos + 3) != Some(&b':') {
                return None;
            }
            offset = (hours * 60 + minutes) * 60 * 1000;
            if sign == b'-' {
                offset = -offset;
            }
        }
        _ => return None,
    }
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1000 + millis - offset)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

fn skip(row: usize, reason: impl Into<String>) -> RowReport {
    RowReport {
        row,
        disposition: RowDisposition::Skip,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_reports_valid_duplicate_invalid_and_unsupported_rows() {
        let input = concat!(
            "{\"type\":\"user\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
            "{\"type\":\"user\",\"uuid\":\"m1\",\"message\":{\"role\":\"user\",\"content\":\"duplicate\"}}\n",
            "{invalid json}\n",
            "{\"type\":\"progress\",\"data\":{}}\n"
        );
        let report = dry_run(input, HistorySource::Claude);
        assert!(report.dry_run);
        assert_eq!(report.rows.len(), 4);
        assert_eq!(report.rows[0].disposition, RowDisposition::Import);
        assert!(report.rows[1].reason.contains("duplicate"));
        assert!(report.rows[2].reason.contains("invalid JSON"));
        assert_eq!(report.rows[3].disposition, RowDisposition::Skip);
    }
}
