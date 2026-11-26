//! Redacted session export (roadmap 89).
//!
//! Markdown and JSON exports of a session, with every known credential shape removed
//! via the shared redactor and absolute paths optionally relativised. Each export
//! carries a manifest of what was omitted so a reader can see that something was
//! removed rather than silently losing it.

pub mod markdown;

pub use markdown::{
    ExportFormat, ExportManifest, ExportResult, OmittedItem, export_json, export_markdown,
};
