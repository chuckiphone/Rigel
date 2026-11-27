//! Opt-in spans stored in the bounded local diagnostics ring.
//!
//! Traces are never sent anywhere. Call [`configure`] to enable them for this process; they
//! remain in memory and are exposed only through [`trace_view`]. Fields pass through the shared
//! JSON redactor before serialization, and the diagnostics ring applies its text redactor again.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostics::ring::{self, Level};

const TRACE_PREFIX: &str = "trace.";
static ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub enabled: bool,
}

/// A locally stored span as returned by the read-only trace view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub at_ms: u128,
    pub name: String,
    pub fields: Value,
}

/// Read-only view of the spans currently retained in the shared diagnostics ring.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceView {
    pub spans: Vec<Span>,
}

/// Apply process-wide local tracing configuration. Tracing is disabled by default.
pub fn configure(config: Config) {
    ENABLED.store(config.enabled, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Store a redacted span in the local diagnostics ring when tracing is enabled.
pub fn record(name: &'static str, fields: Value) {
    if !is_enabled() {
        return;
    }
    let fields = crate::redaction::redact_json(&fields);
    let message = serde_json::json!({ "name": name, "fields": fields }).to_string();
    let ring_span = format!("{TRACE_PREFIX}{name}");
    ring::shared_record(Level::Info, Some(&ring_span), &message);
}

/// Return the retained trace spans. This view has no upload or export operation.
pub fn trace_view() -> TraceView {
    let Ok(ring) = ring::shared().lock() else {
        return TraceView::default();
    };
    let spans = ring
        .events()
        .filter_map(|event| {
            let name = event.span.as_deref()?.strip_prefix(TRACE_PREFIX)?;
            let value: Value = serde_json::from_str(&event.message).ok()?;
            Some(Span {
                at_ms: event.at_ms,
                name: name.to_string(),
                fields: value.get("fields")?.clone(),
            })
        })
        .collect();
    TraceView { spans }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn opt_in_controls_local_redacted_storage_and_view_is_read_only_data() {
        let _guard = TEST_LOCK.lock().await;
        let marker = "trace-opt-in-test";
        configure(Config { enabled: false });
        record("turn", json!({ "marker": marker }));
        assert!(
            !trace_view()
                .spans
                .iter()
                .any(|span| span.fields["marker"] == marker)
        );

        configure(Config { enabled: true });
        let secret = "sk-proj-AbCdEfGhIjKlMnOpQrStUvWx";
        record(
            "turn",
            json!({ "marker": marker, "prompt": format!("use {secret}") }),
        );
        let span = trace_view()
            .spans
            .into_iter()
            .find(|span| span.fields["marker"] == marker)
            .expect("enabled spans are retained in the local ring");
        assert_eq!(span.name, "turn");
        assert_eq!(span.fields["prompt"], "use <redacted>");
        assert!(
            !serde_json::to_string(&TraceView { spans: vec![span] })
                .unwrap()
                .to_lowercase()
                .contains("upload")
        );
        configure(Config::default());
    }
}
