//! Roadmap 196: local turn metrics.
//!
//! `rigel stats --json` reads the project database through the same `SessionService` the window
//! uses and prints one aggregate object. Every number comes from recorded turns and items; this
//! module owns no client, opens no socket and makes no request. The shape is pinned in
//! `docs/stats.md` and exercised through the real CLI in `crates/rigel-cli/tests/stats.rs`.

use std::path::Path;
use std::process::ExitCode;

use rigel_core::{SessionService, TurnTranscript, Usage};
use serde::Serialize;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Latency {
    /// Sum of wall time for completed turns, in milliseconds.
    pub total_ms: u64,
    /// `total_ms / completed_turns`, or 0 when nothing completed.
    pub mean_ms: u64,
    /// Longest single completed turn, in milliseconds.
    pub max_ms: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    /// `input + output + reasoning`; cached reads are a subset of `input`, not extra tokens.
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Cache {
    /// Usage rows where the provider reported at least one cached input token.
    pub hits: u64,
    pub read_tokens: u64,
    pub write_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Stats {
    pub schema: u32,
    pub sessions: usize,
    pub turns: usize,
    pub completed_turns: usize,
    pub steps: u64,
    pub latency_ms: Latency,
    /// Wall time inside recorded model calls, summed from the usage rows.
    pub model_ms: u64,
    /// Time between step boundaries (pause, tools and persistence), summed the same way.
    pub queue_ms: u64,
    pub tokens: Tokens,
    pub cache: Cache,
    /// Count of recorded retry rows, not a count of failed turns.
    pub retries: u64,
    pub cost_micros: i64,
}

/** Aggregate recorded turns into one local report. Pure: no I/O, clock or network. */
pub fn collect(transcripts: &[TurnTranscript], sessions: usize) -> Stats {
    let mut stats = Stats {
        schema: SCHEMA,
        sessions,
        turns: transcripts.len(),
        ..Stats::default()
    };
    let mut completed = 0u64;

    for transcript in transcripts {
        let turn = &transcript.turn;
        if turn.status == "completed" {
            completed += 1;
            if let Some(ended) = turn.completed_at {
                let elapsed = ended.saturating_sub(turn.started_at).max(0) as u64;
                stats.latency_ms.total_ms = stats.latency_ms.total_ms.saturating_add(elapsed);
                stats.latency_ms.max_ms = stats.latency_ms.max_ms.max(elapsed);
            }
        }
        for item in &transcript.items {
            match item.kind.as_str() {
                "usage" => {
                    let Ok(usage) = serde_json::from_value::<Usage>(item.body.clone()) else {
                        continue;
                    };
                    stats.steps += 1;
                    stats.tokens.input = stats.tokens.input.saturating_add(usage.input_tokens);
                    stats.tokens.output = stats.tokens.output.saturating_add(usage.output_tokens);
                    stats.tokens.reasoning = stats
                        .tokens
                        .reasoning
                        .saturating_add(usage.reasoning_tokens);
                    stats.cost_micros = stats.cost_micros.saturating_add(usage.cost_micros);
                    stats.cache.read_tokens = stats
                        .cache
                        .read_tokens
                        .saturating_add(usage.cached_input_tokens);
                    stats.cache.write_tokens = stats
                        .cache
                        .write_tokens
                        .saturating_add(usage.cache_write_tokens);
                    if usage.cached_input_tokens > 0 {
                        stats.cache.hits += 1;
                    }
                    stats.model_ms = stats.model_ms.saturating_add(
                        item.body
                            .get("duration_ms")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    );
                    stats.queue_ms = stats.queue_ms.saturating_add(
                        item.body
                            .get("queue_ms")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    );
                }
                "retry" => stats.retries += 1,
                _ => {}
            }
        }
    }
    stats.tokens.total = stats
        .tokens
        .input
        .saturating_add(stats.tokens.output)
        .saturating_add(stats.tokens.reasoning);
    if let Some(mean) = stats.latency_ms.total_ms.checked_div(completed) {
        stats.latency_ms.mean_ms = mean;
    }
    stats.completed_turns = completed as usize;
    stats
}

/** The command itself: local reads only, and one stable shape on stdout. */
pub async fn run(project: &Path, as_json: bool) -> ExitCode {
    let service = match SessionService::open(project).await {
        Ok(service) => service,
        Err(error) => {
            eprintln!("rigel stats: {error}");
            return ExitCode::from(2);
        }
    };
    let sessions = match service.list().await {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("rigel stats: {error}");
            return ExitCode::from(2);
        }
    };
    let mut transcripts = Vec::new();
    for session in &sessions {
        match service.transcript(&session.id).await {
            Ok(mut rows) => transcripts.append(&mut rows),
            Err(error) => eprintln!("rigel stats: session {}: {error}", session.id),
        }
    }
    let stats = collect(&transcripts, sessions.len());

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&stats).unwrap_or_default()
        );
    } else {
        println!("schema     {}", stats.schema);
        println!("sessions   {}", stats.sessions);
        println!(
            "turns      {} ({} completed)",
            stats.turns, stats.completed_turns
        );
        println!("steps      {}", stats.steps);
        println!(
            "latency    total {} ms · mean {} ms · max {} ms",
            stats.latency_ms.total_ms, stats.latency_ms.mean_ms, stats.latency_ms.max_ms
        );
        println!("model/queue {} ms / {} ms", stats.model_ms, stats.queue_ms);
        println!(
            "tokens     {} ({} in, {} out, {} reasoning)",
            stats.tokens.total, stats.tokens.input, stats.tokens.output, stats.tokens.reasoning
        );
        println!(
            "cache      {} hits · {} read · {} write",
            stats.cache.hits, stats.cache.read_tokens, stats.cache.write_tokens
        );
        println!("retries    {}", stats.retries);
        println!("spend      {} micros", stats.cost_micros);
        println!("local only · no request was made");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use rigel_core::{Item, Turn};
    use serde_json::json;

    use super::*;

    fn turn(id: &str, status: &str, started: i64, ended: Option<i64>) -> Turn {
        Turn {
            id: id.to_string(),
            agent_id: "agent-1".to_string(),
            parent_turn_id: None,
            seq: 1,
            status: status.to_string(),
            prompt: Some("fix the parser".to_string()),
            images: Vec::new(),
            started_at: started,
            completed_at: ended,
            usage: None,
            stop_reason: None,
        }
    }

    fn item(kind: &str, body: serde_json::Value) -> Item {
        Item {
            id: format!("i-{kind}"),
            turn_id: "t-1".to_string(),
            seq: 1,
            kind: kind.to_string(),
            status: "completed".to_string(),
            body,
            created_at: 1,
            raw: None,
        }
    }

    #[test]
    fn an_empty_ledger_is_zero_not_an_error() {
        let stats = collect(&[], 0);
        assert_eq!(stats.schema, SCHEMA);
        assert_eq!(stats.turns, 0);
        assert_eq!(stats.latency_ms.mean_ms, 0);
        assert_eq!(stats.tokens.total, 0);
        assert_eq!(stats.retries, 0);
    }

    #[test]
    fn usage_rows_supply_tokens_latency_retries_and_only_reported_cache_hits() {
        let usage = json!({
            "input_tokens": 100,
            "output_tokens": 40,
            "reasoning_tokens": 10,
            "cached_input_tokens": 20,
            "cache_write_tokens": 5,
            "cost_micros": 200,
            "step": 1,
            "duration_ms": 300,
            "queue_ms": 50,
        });
        let transcripts = vec![TurnTranscript {
            turn: turn("t-1", "completed", 1_000, Some(1_450)),
            items: vec![
                item("message", json!({})),
                item("usage", usage.clone()),
                item("retry", json!({ "reason": "429" })),
                item(
                    "usage",
                    json!({
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "cost_micros": 30,
                        "duration_ms": 120,
                        "queue_ms": 20,
                    }),
                ),
            ],
        }];
        let stats = collect(&transcripts, 1);
        assert_eq!(stats.schema, 1);
        assert_eq!(stats.sessions, 1);
        assert_eq!(stats.turns, 1);
        assert_eq!(stats.completed_turns, 1);
        assert_eq!(stats.steps, 2);
        assert_eq!(stats.latency_ms.total_ms, 450);
        assert_eq!(stats.latency_ms.mean_ms, 450);
        assert_eq!(stats.latency_ms.max_ms, 450);
        assert_eq!(stats.model_ms, 420);
        assert_eq!(stats.queue_ms, 70);
        assert_eq!(stats.tokens.input, 110);
        assert_eq!(stats.tokens.output, 45);
        assert_eq!(stats.tokens.reasoning, 10);
        assert_eq!(stats.tokens.total, 165);
        assert_eq!(stats.cache.hits, 1, "a silent provider is not a cache hit");
        assert_eq!(stats.cache.read_tokens, 20);
        assert_eq!(stats.cache.write_tokens, 5);
        assert_eq!(stats.retries, 1);
        assert_eq!(stats.cost_micros, 230);
    }

    #[test]
    fn a_running_turn_has_no_latency_but_its_steps_still_count() {
        let transcripts = vec![TurnTranscript {
            turn: turn("t-2", "running", 1_000, None),
            items: vec![item("usage", json!({ "input_tokens": 7 }))],
        }];
        let stats = collect(&transcripts, 1);
        assert_eq!(stats.completed_turns, 0);
        assert_eq!(stats.latency_ms.total_ms, 0);
        assert_eq!(stats.latency_ms.mean_ms, 0);
        assert_eq!(stats.steps, 1);
        assert_eq!(stats.tokens.total, 7);
    }

    #[test]
    fn latency_uses_the_longest_completed_turn_and_never_goes_negative() {
        let transcripts = vec![
            TurnTranscript {
                turn: turn("t-1", "completed", 1_000, Some(1_300)),
                items: vec![],
            },
            TurnTranscript {
                turn: turn("t-2", "completed", 2_000, Some(2_900)),
                items: vec![],
            },
            TurnTranscript {
                turn: turn("t-3", "completed", 3_000, Some(2_000)),
                items: vec![],
            },
        ];
        let stats = collect(&transcripts, 1);
        assert_eq!(stats.completed_turns, 3);
        assert_eq!(stats.latency_ms.total_ms, 1_200);
        assert_eq!(stats.latency_ms.mean_ms, 400);
        assert_eq!(stats.latency_ms.max_ms, 900);
    }

    #[test]
    fn the_json_shape_names_every_group() {
        let stats = collect(&[], 0);
        let value = serde_json::to_value(&stats).unwrap();
        for key in [
            "schema",
            "sessions",
            "turns",
            "completed_turns",
            "steps",
            "latency_ms",
            "model_ms",
            "queue_ms",
            "tokens",
            "cache",
            "retries",
            "cost_micros",
        ] {
            assert!(value.get(key).is_some(), "{key} is in the documented shape");
        }
        assert!(value["latency_ms"].get("mean_ms").is_some());
        assert!(value["tokens"].get("total").is_some());
        assert!(value["cache"].get("hits").is_some());
    }
}
