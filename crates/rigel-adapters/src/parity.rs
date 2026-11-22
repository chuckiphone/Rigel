//! The harness parity matrix (roadmap item 48).
//!
//! The table is generated, never transcribed: every approvals, streaming, usage and tools cell
//! comes from [`crate::registry::capabilities`], including the typed resume support. [`markdown`]
//! is the committed artifact; `tests/harness_parity.rs` compares it byte for byte so a capability
//! change cannot land quietly.

use std::fmt::Write as _;

use crate::registry::capabilities;
use crate::{Harness, ResumeSupport};
use serde::Serialize;

/// The date on every cell. A later re-check changes this one value and regenerates the table.
pub const CHECKED: &str = "2026-11-18";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParityStatus {
    Pass,
    Fail,
    NotSupported,
}

impl ParityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotSupported => "not-supported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ParityCell {
    pub status: ParityStatus,
    pub checked: &'static str,
    pub evidence: &'static str,
}

impl ParityCell {
    fn new(status: ParityStatus, evidence: &'static str) -> Self {
        Self {
            status,
            checked: CHECKED,
            evidence,
        }
    }

    /// The exact cell text in the checked-in table.
    pub fn label(&self) -> String {
        format!("{} ({})", self.status.as_str(), self.checked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Approvals,
    Streaming,
    Usage,
    Resume,
    Tools,
}

pub const DIMENSIONS: [Dimension; 5] = [
    Dimension::Approvals,
    Dimension::Streaming,
    Dimension::Usage,
    Dimension::Resume,
    Dimension::Tools,
];

impl Dimension {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approvals => "Approvals",
            Self::Streaming => "Streaming",
            Self::Usage => "Usage",
            Self::Resume => "Resume",
            Self::Tools => "Tools",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Approvals => 0,
            Self::Streaming => 1,
            Self::Usage => 2,
            Self::Resume => 3,
            Self::Tools => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParityRow {
    pub label: &'static str,
    pub harnesses: Vec<Harness>,
    /// In [`DIMENSIONS`] order.
    pub cells: [ParityCell; 5],
}

impl ParityRow {
    pub fn cell(&self, dimension: Dimension) -> &ParityCell {
        &self.cells[dimension.index()]
    }
}

/// The named rows. ACP is one protocol with two spawns, so each spawn gets its own row; a merged
/// row could hide a difference between `agent acp` and `cline --acp`.
pub fn matrix() -> Vec<ParityRow> {
    [
        ("Native", Harness::Native),
        ("Claude Code", Harness::Claude),
        ("Codex", Harness::Codex),
        ("Gemini CLI", Harness::Gemini),
        ("ACP (Cursor)", Harness::Cursor),
        ("ACP (Cline)", Harness::Cline),
    ]
    .into_iter()
    .map(|(label, harness)| row(label, harness))
    .collect()
}

fn row(label: &'static str, harness: Harness) -> ParityRow {
    let caps = capabilities(harness, None);
    let cells = [
        flag(
            caps.programmatic_approval,
            "`Capabilities::programmatic_approval` (approval adapter test)",
        ),
        flag(
            caps.json_stream,
            "`Capabilities::json_stream` (stream fixture)",
        ),
        flag(caps.usage, "`Capabilities::usage` (usage fixture)"),
        resume_cell(caps.resume),
        flag(
            caps.tools,
            "`Capabilities::tools` (normalised tool-call fixture)",
        ),
    ];
    ParityRow {
        label,
        harnesses: vec![harness],
        cells,
    }
}

fn flag(present: bool, evidence: &'static str) -> ParityCell {
    let status = if present {
        ParityStatus::Pass
    } else {
        ParityStatus::NotSupported
    };
    ParityCell::new(status, evidence)
}

fn resume_cell(support: ResumeSupport) -> ParityCell {
    match support {
        ResumeSupport::Wired => ParityCell::new(
            ParityStatus::Pass,
            "`rigel_core::resume` and the paused-turn store",
        ),
        // The CLI ships a vendor resume command, but the adapter only records the thread/session
        // id for display; it never forwards it to the next invocation. That is a fail, not a
        // protocol limitation.
        ResumeSupport::VendorOnly => ParityCell::new(
            ParityStatus::Fail,
            "vendor resume exists; the adapter never passes the recorded thread/session id",
        ),
        ResumeSupport::Unsupported => ParityCell::new(
            ParityStatus::NotSupported,
            "the ACP client only calls `session/new`; it has no `session/load` path",
        ),
    }
}

/// The checked-in Markdown. Keep the surrounding prose here too: a test compares the whole file,
/// so a claim added by hand is as visible as a changed cell.
pub fn markdown() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Harness parity matrix");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "_Generated by `rigel_adapters::parity::markdown()` from `registry::capabilities` on \
         {CHECKED}. Do not edit this file by hand; change the capability flag or the generator._"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Scope: Native and the four external harness families named by roadmap item 48 — Claude, \
         Codex, Gemini and ACP. ACP is the shared client used by Cursor and Cline, so both spawns \
         have their own row. Muse Code is probed by `rigel harness probe` but is outside this \
         item's named scope."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Harness | Approvals | Streaming | Usage | Resume | Tools |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for row in matrix() {
        let _ = write!(out, "| {} |", row.label);
        for cell in &row.cells {
            let _ = write!(out, " {} |", cell.label());
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "## Statuses");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Status | Meaning |");
    let _ = writeln!(out, "|---|---|");
    let _ = writeln!(
        out,
        "| `pass` | The adapter path exists and a focused test or fixture covers it. |"
    );
    let _ = writeln!(
        out,
        "| `fail` | The vendor supports the capability, but Rigel does not wire it up yet. |"
    );
    let _ = writeln!(
        out,
        "| `not-supported` | Rigel's adapter or client has no path to the capability. |"
    );

    let _ = writeln!(out);
    let _ = writeln!(out, "## What each column checks");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **Approvals** — Rigel answers the harness's own permission request: the native approval \
         bus, Codex app-server approvals, Claude's MCP permission tool, or ACP \
         `session/request_permission`."
    );
    let _ = writeln!(
        out,
        "- **Streaming** — deltas arrive while the turn runs: native provider streams, Claude \
         `stream-json`, Codex app-server deltas, or ACP `session/update` message chunks."
    );
    let _ = writeln!(
        out,
        "- **Usage** — a `usage` transcript item is written from harness-reported data. ACP reports \
         context used/size, not a billed input/output split; that limit is recorded as-is."
    );
    let _ = writeln!(
        out,
        "- **Resume** — a new invocation continues the harness's own conversation. Native resumes \
         Rigel's session and paused turn; Codex and Claude record a thread/session id but never \
         pass it back; the ACP client starts every turn with `session/new`."
    );
    let _ = writeln!(
        out,
        "- **Tools** — tool calls and results become transcript items. Native executes Rigel's tool \
         set; external harnesses run their own tools and Rigel records them."
    );

    let _ = writeln!(out);
    let _ = writeln!(out, "## How this is checked");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "`cargo test -p rigel-cli --test harness_parity` compares this file with \
         `rigel_adapters::parity::markdown()` and pins each expected status. \
         `cargo test -p rigel-adapters parity` checks every cell against \
         `registry::capabilities`; a capability that silently flips therefore fails the check."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Codex approvals are app-server only; its `exec --json` fallback has no approval channel. \
         Claude approvals require the MCP permission tool the CLI writes per run. ACP approvals \
         are answered only with an option the agent offered, otherwise `cancelled`."
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::HARNESSES;

    #[test]
    fn every_named_row_has_five_dated_cells() {
        let rows = matrix();
        assert_eq!(rows.len(), 6);
        for row in rows {
            assert!(!row.label.is_empty());
            assert_eq!(row.cells.len(), DIMENSIONS.len());
            for cell in row.cells {
                assert_eq!(cell.checked, CHECKED);
                assert!(matches!(
                    cell.status,
                    ParityStatus::Pass | ParityStatus::Fail | ParityStatus::NotSupported
                ));
                assert!(!cell.evidence.is_empty());
            }
        }
    }

    #[test]
    fn the_four_shared_flags_drive_the_table() {
        for row in matrix() {
            let caps = capabilities(row.harnesses[0], None);
            assert_eq!(
                row.cell(Dimension::Approvals).status,
                if caps.programmatic_approval {
                    ParityStatus::Pass
                } else {
                    ParityStatus::NotSupported
                }
            );
            assert_eq!(
                row.cell(Dimension::Streaming).status,
                if caps.json_stream {
                    ParityStatus::Pass
                } else {
                    ParityStatus::NotSupported
                }
            );
            assert_eq!(
                row.cell(Dimension::Usage).status,
                if caps.usage {
                    ParityStatus::Pass
                } else {
                    ParityStatus::NotSupported
                }
            );
            assert_eq!(
                row.cell(Dimension::Tools).status,
                if caps.tools {
                    ParityStatus::Pass
                } else {
                    ParityStatus::NotSupported
                }
            );
        }
    }

    #[test]
    fn expected_statuses_are_pinned() {
        let got: Vec<(&str, Vec<&str>)> = matrix()
            .into_iter()
            .map(|row| {
                (
                    row.label,
                    row.cells.iter().map(|cell| cell.status.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Native", vec!["pass", "pass", "pass", "pass", "pass"]),
                ("Claude Code", vec!["pass", "pass", "pass", "fail", "pass"]),
                ("Codex", vec!["pass", "pass", "pass", "fail", "pass"]),
                (
                    "Gemini CLI",
                    vec!["pass", "pass", "pass", "not-supported", "pass"]
                ),
                (
                    "ACP (Cursor)",
                    vec!["pass", "pass", "pass", "not-supported", "pass"]
                ),
                (
                    "ACP (Cline)",
                    vec!["pass", "pass", "pass", "not-supported", "pass"]
                ),
            ]
        );
    }

    #[test]
    fn a_new_probed_harness_must_be_placed_in_the_matrix() {
        let represented: Vec<Harness> =
            matrix().into_iter().flat_map(|row| row.harnesses).collect();
        assert!(represented.contains(&Harness::Native));
        for harness in HARNESSES {
            // Muse Code is deliberately outside item 48's named scope. Every other probed
            // harness must appear, so adding one cannot silently shrink the matrix.
            assert!(
                harness == Harness::Muse || represented.contains(&harness),
                "{harness:?} is probed but missing from the parity matrix"
            );
        }
    }

    #[test]
    fn resume_support_is_typed_for_each_named_harness() {
        assert_eq!(
            capabilities(Harness::Native, None).resume,
            ResumeSupport::Wired
        );
        assert_eq!(
            capabilities(Harness::Claude, None).resume,
            ResumeSupport::VendorOnly
        );
        assert_eq!(
            capabilities(Harness::Codex, None).resume,
            ResumeSupport::VendorOnly
        );
        for harness in [Harness::Gemini, Harness::Cursor, Harness::Cline] {
            assert_eq!(
                capabilities(harness, None).resume,
                ResumeSupport::Unsupported
            );
        }
    }
}
