//! Prompt caching (roadmap 52).
//!
//! Two questions live here, and they stay separate:
//!
//! * **What the request asks for.** Anthropic is the only door in this tree that needs an
//!   explicit marker: the Messages API caches a stable prefix only where the request carries
//!   `cache_control`. The OpenAI-shaped doors, Gemini on AI Studio and Vertex cache repeated
//!   prefixes automatically and take no marker, so nothing is added to their requests.
//! * **What the provider actually did.** A cache read arrives as cached input tokens; Anthropic
//!   also reports cache **writes** separately. The transcript shows both exactly as reported.
//!   A saving is computed only from a reported read and only at a published local rate, so a
//!   provider that reported no cache activity never looks like it saved money.
//!
//! A marker on a prefix shorter than the provider's minimum is ignored by the provider rather
//! than rejected, so the request is valid whether or not the conversation has grown enough to
//! cache yet.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::domain::Usage;

use super::pricing;

/// The cache activity a provider reported, as the transcript renders it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheReport {
    /// Tokens the provider served from its cache, billed at the cached-input rate.
    pub read_tokens: u64,
    /// Tokens the provider wrote into its cache, billed as input. Zero when the provider does
    /// not report writes separately.
    pub write_tokens: u64,
    /// What the reported reads would have cost at the full input rate, minus what they did
    /// cost. Absent when there is no reported read to discount.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_micros: Option<i64>,
    /// How the saving was obtained: `estimated`, because the local price table rather than the
    /// provider's invoice prices the difference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saving: Option<String>,
}

/// The provider-reported cache activity, if there is any to show.
pub fn report(usage: &Usage, model: &str) -> Option<CacheReport> {
    if usage.cached_input_tokens == 0 && usage.cache_write_tokens == 0 {
        return None;
    }
    let saved = savings_micros(usage, model);
    Some(CacheReport {
        read_tokens: usage.cached_input_tokens,
        write_tokens: usage.cache_write_tokens,
        saved_micros: saved,
        saving: saved.map(|_| "estimated".to_string()),
    })
}

/// The discount on the provider's reported reads, at the local table's rates.
///
/// `None` when no read was reported, when the model has no published row (the fallback price
/// would be a guess), or when the cached rate is not actually cheaper. A zero is never invented
/// and a saving is never inferred from a provider that reported none.
pub fn savings_micros(usage: &Usage, model: &str) -> Option<i64> {
    if usage.cached_input_tokens == 0 {
        return None;
    }
    let billing_model = usage.served_model.as_deref().unwrap_or(model);
    let billing_model = super::model::chat_model_id(billing_model);
    if !pricing::is_listed(billing_model) {
        return None;
    }
    let price = pricing::price_for(billing_model);
    let difference = price.input - price.cached_input;
    if difference <= 0.0 {
        return None;
    }
    Some((usage.cached_input_tokens as f64 * difference).round() as i64)
}

/// Add Anthropic's cache breakpoints to a Messages body.
///
/// Three of the four allowed breakpoints are used:
/// * the last tool definition, so the tool block is a cached prefix;
/// * the system block;
/// * the last content block of the last message, so the growing conversation is cached once
///   per step instead of re-read at full price.
///
/// The fourth is left free for a caller that appends its own marker. Only the Anthropic adapter
/// calls this; a provider that does not read `cache_control` is never sent one.
pub fn mark_anthropic(body: &mut Value) {
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut)
        && let Some(last) = tools.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }

    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut)
        && let Some(last) = system.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }

    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut)
        && let Some(last_message) = messages.last_mut()
        && let Some(blocks) = last_message
            .get_mut("content")
            .and_then(Value::as_array_mut)
        && let Some(last) = blocks.last_mut()
    {
        last["cache_control"] = json!({ "type": "ephemeral" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Usage;

    fn body() -> Value {
        json!({
            "system": [{ "type": "text", "text": "rules" }],
            "tools": [{ "name": "read_file" }, { "name": "write_file" }],
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "one" }] },
                { "role": "assistant", "content": [{ "type": "text", "text": "two" }] }
            ]
        })
    }

    #[test]
    fn anthropic_markers_cover_the_definition_and_conversation_prefixes() {
        let mut body = body();
        mark_anthropic(&mut body);

        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(
            body["tools"][0].get("cache_control").is_none(),
            "only the last tool closes the cached prefix"
        );
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        assert!(
            body["messages"][0]["content"][0]
                .get("cache_control")
                .is_none(),
            "only the last block closes the cached prefix"
        );
        assert_eq!(
            body["messages"][1]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn a_marker_is_skipped_when_its_section_is_absent() {
        let mut body = json!({ "messages": [] });
        mark_anthropic(&mut body);
        assert!(body["messages"].as_array().unwrap().is_empty());
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn only_a_reported_read_can_save_money() {
        // 1,000 cached reads at GPT-5.4 mini: (0.75 − 0.075) µ$ per token = 675 µ$.
        let cached = Usage {
            cached_input_tokens: 1_000,
            ..Usage::default()
        };
        assert_eq!(savings_micros(&cached, "openai/gpt-5.4-mini"), Some(675));

        let none = Usage::default();
        assert_eq!(savings_micros(&none, "openai/gpt-5.4-mini"), None);
        assert!(
            report(&none, "openai/gpt-5.4-mini").is_none(),
            "nothing reported means no cache row at all"
        );

        // A model without a published row is priced conservatively for the bill, but the
        // fallback rate is not evidence of a discount.
        assert_eq!(savings_micros(&cached, "some/new-model"), None);
    }

    #[test]
    fn a_reported_write_is_shown_but_never_counted_as_a_saving() {
        let written = Usage {
            cache_write_tokens: 500,
            ..Usage::default()
        };
        let report = report(&written, "openai/gpt-5.4-mini").expect("a cache row");
        assert_eq!(report.read_tokens, 0);
        assert_eq!(report.write_tokens, 500);
        assert_eq!(report.saved_micros, None);
        assert_eq!(report.saving, None);
    }

    #[test]
    fn the_served_model_prices_the_saving_when_a_router_names_one() {
        let cached = Usage {
            cached_input_tokens: 1_000,
            served_model: Some("anthropic/claude-sonnet-4.5".to_string()),
            ..Usage::default()
        };
        // (3.00 − 0.30) µ$ per token × 1,000 = 2,700 µ$.
        assert_eq!(savings_micros(&cached, "openrouter"), Some(2_700));
    }
}
