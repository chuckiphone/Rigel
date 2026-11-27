//! Markdown and JSON export with redaction (roadmap 89).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::redaction::{redact_json, redact_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmittedItem {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExportManifest {
    pub items: Vec<OmittedItem>,
}

impl ExportManifest {
    pub fn push(&mut self, kind: impl Into<String>, detail: impl Into<String>) {
        self.items.push(OmittedItem {
            kind: kind.into(),
            detail: detail.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ExportResult {
    pub content: String,
    pub manifest: ExportManifest,
}

/// Options for an export.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// When set, absolute paths under this root are relativised.
    pub root: Option<PathBuf>,
    /// Relativise paths.
    pub relativise_paths: bool,
}

/// Redact secrets from markdown text and optionally relativise paths.
pub fn redact_markdown(input: &str, options: &ExportOptions) -> ExportResult {
    let mut manifest = ExportManifest::default();
    let mut out = redact_text(input);

    // Count redactions: if the marker appears, record one omitted item per occurrence.
    let redaction_count = out.matches("<redacted>").count();
    if redaction_count > 0 {
        manifest.push(
            "secret",
            format!("{redaction_count} credential shape(s) redacted"),
        );
    }

    if options.relativise_paths
        && let Some(root) = &options.root
    {
        let (text, count) = relativise_paths(&out, root);
        out = text;
        if count > 0 {
            manifest.push("path", format!("{count} absolute path(s) relativised"));
        }
    }

    ExportResult {
        content: out,
        manifest,
    }
}

/// Redact secrets from a JSON value and optionally relativise paths in string fields.
pub fn redact_json_value(value: &Value, options: &ExportOptions) -> (Value, ExportManifest) {
    let mut manifest = ExportManifest::default();
    let redacted = redact_json(value);
    // Count redacted markers in serialized form.
    let serialized = redacted.to_string();
    let count = serialized.matches("<redacted>").count();
    if count > 0 {
        manifest.push("secret", format!("{count} credential shape(s) redacted"));
    }

    let mut value = redacted;
    if options.relativise_paths
        && let Some(root) = &options.root
    {
        let changed = relativise_json_paths(&mut value, root);
        if changed > 0 {
            manifest.push("path", format!("{changed} absolute path(s) relativised"));
        }
    }

    (value, manifest)
}

/// Convenience: export markdown content.
pub fn export_markdown(input: &str, options: &ExportOptions) -> ExportResult {
    redact_markdown(input, options)
}

/// Convenience: export a JSON value as a pretty string.
pub fn export_json(value: &Value, options: &ExportOptions) -> ExportResult {
    let (redacted, manifest) = redact_json_value(value, options);
    let content = serde_json::to_string_pretty(&redacted).unwrap_or_default();
    ExportResult { content, manifest }
}

fn relativise_paths(text: &str, root: &Path) -> (String, usize) {
    let root_str = root.to_string_lossy();
    let root_str = root_str.trim_end_matches(['/', '\\']);
    if root_str.is_empty() {
        return (text.to_string(), 0);
    }
    let root_plain = root_str.trim_end_matches(['/', '\\']).to_string();
    let count = text.matches(&root_plain).count();
    let out = text.replace(&root_plain, "");
    (out, count)
}

fn relativise_json_paths(value: &mut Value, root: &Path) -> usize {
    match value {
        Value::String(s) => {
            let (new, count) = relativise_paths(s, root);
            if count > 0 {
                *s = new;
            }
            count
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|v| relativise_json_paths(v, root))
            .sum(),
        Value::Object(map) => map
            .values_mut()
            .map(|v| relativise_json_paths(v, root))
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_export_redacts_secrets() {
        let input = "token = sk-proj-AbCdEfGhIjKlMnOpQrStUvWx";
        let res = export_markdown(input, &ExportOptions::default());
        assert!(res.content.contains("<redacted>"));
        assert!(!res.content.contains("sk-proj"));
        assert!(!res.manifest.is_empty());
    }

    #[test]
    fn json_export_redacts_secrets() {
        let value = json!({ "api_key": "sk-proj-AbCdEfGhIjKlMnOpQrStUvWx" });
        let res = export_json(&value, &ExportOptions::default());
        assert!(res.content.contains("<redacted>"));
        assert!(!res.content.contains("sk-proj"));
    }

    #[test]
    fn manifest_records_omitted_items() {
        let input = "secret sk-proj-AbCdEfGhIjKlMnOpQrStUvWx";
        let res = export_markdown(input, &ExportOptions::default());
        assert!(!res.manifest.is_empty());
        assert_eq!(res.manifest.items[0].kind, "secret");
    }

    #[test]
    fn paths_are_relativised_when_requested() {
        let root = PathBuf::from("/tmp/project");
        let input = "file at /tmp/project/src/main.rs";
        let opts = ExportOptions {
            root: Some(root),
            relativise_paths: true,
        };
        let res = export_markdown(input, &opts);
        assert!(res.content.contains("src/main.rs"));
        assert!(!res.content.contains("/tmp/project"));
    }

    #[test]
    fn a_corpus_export_contains_no_known_shaped_secret() {
        let input = "OPENAI=sk-proj-AbCdEfGhIjKlMnOpQrStUvWx\nAWS=AKIAIOSFODNN7EXAMPLE\n";
        let res = export_markdown(input, &ExportOptions::default());
        assert!(!res.content.contains("sk-proj"));
        assert!(!res.content.contains("AKIAIOSFODNN7EXAMPLE"));
    }
}
