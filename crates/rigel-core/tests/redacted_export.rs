//! Redacted export (roadmap 89) integration tests.

use rigel_core::export::markdown::{ExportOptions, export_json, export_markdown};
use serde_json::json;

#[test]
fn markdown_and_json_exports_are_redacted() {
    let input = "token = sk-proj-AbCdEfGhIjKlMnOpQrStUvWx";
    let res = export_markdown(input, &ExportOptions::default());
    assert!(res.content.contains("<redacted>"));
    assert!(!res.content.contains("sk-proj"));

    let value = json!({ "api_key": "sk-proj-AbCdEfGhIjKlMnOpQrStUvWx" });
    let res = export_json(&value, &ExportOptions::default());
    assert!(res.content.contains("<redacted>"));
    assert!(!res.content.contains("sk-proj"));

    // manifest non-empty
    assert!(!res.manifest.is_empty());
}
