//! The checked-in harness parity matrix is generated, and this is the gate that keeps it honest.
//!
//! The table lives at `docs/harness-parity.md`; these tests compare it byte for byte with
//! `rigel_adapters::parity::markdown()` and exercise the CLI command that regenerates or checks it.
//! A silent capability change in `registry::capabilities` therefore fails here instead of leaving
//! the document claiming something the adapters no longer do.

use std::path::{Path, PathBuf};
use std::process::Command;

use rigel_adapters::parity::{CHECKED, DIMENSIONS, ParityStatus, markdown, matrix};
use serde_json::Value;

fn docs_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/harness-parity.md")
}

fn read_doc() -> String {
    std::fs::read_to_string(docs_path())
        .expect("docs/harness-parity.md exists")
        .replace("\r\n", "\n")
}

fn rigel(data: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("rigel runs")
}

#[test]
fn the_checked_in_matrix_is_exactly_the_generated_one() {
    assert_eq!(read_doc(), markdown());
}

#[test]
fn every_named_harness_has_a_dated_status_in_every_column() {
    let rows = matrix();
    let labels: Vec<&str> = rows.iter().map(|row| row.label).collect();
    assert_eq!(
        labels,
        vec![
            "Native",
            "Claude Code",
            "Codex",
            "Gemini CLI",
            "ACP (Cursor)",
            "ACP (Cline)",
        ]
    );
    for row in rows {
        assert_eq!(row.cells.len(), DIMENSIONS.len());
        for cell in row.cells {
            assert_eq!(cell.checked, CHECKED);
            assert!(matches!(
                cell.status,
                ParityStatus::Pass | ParityStatus::Fail | ParityStatus::NotSupported
            ));
            assert!(
                read_doc().contains(&cell.label()),
                "{} is missing from the table",
                cell.label()
            );
        }
    }
}

#[test]
fn a_harness_gaining_or_losing_a_capability_changes_the_table() {
    // The generator and the document are compared above; this pins the values a change would
    // move. If a flag flips, both this expectation and the checked-in table must be updated
    // deliberately rather than drifting silently.
    let statuses: Vec<(&str, Vec<&str>)> = matrix()
        .into_iter()
        .map(|row| {
            (
                row.label,
                row.cells.iter().map(|cell| cell.status.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
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
fn the_cli_writes_prints_and_checks_the_same_matrix() {
    let data = tempfile::tempdir().unwrap();
    let out = data.path().join("harness-parity.md");
    let out_text = out.to_str().unwrap();

    let written = rigel(data.path(), &["harness", "parity", "--out", out_text]);
    assert!(
        written.status.success(),
        "write failed: {}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().replace("\r\n", "\n"),
        markdown()
    );

    let printed = rigel(data.path(), &["harness", "parity"]);
    assert!(printed.status.success());
    assert_eq!(String::from_utf8_lossy(&printed.stdout), markdown());

    let json = rigel(data.path(), &["harness", "parity", "--json"]);
    assert!(json.status.success());
    let body: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["checked"], CHECKED);
    assert_eq!(body["rows"].as_array().unwrap().len(), 6);
    assert_eq!(body["rows"][0]["cells"][0]["status"], "pass");

    let checked = rigel(
        data.path(),
        &["harness", "parity", "--check", "--out", out_text],
    );
    assert!(
        checked.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&checked.stderr)
    );

    std::fs::write(&out, "stale\n").unwrap();
    let stale = rigel(
        data.path(),
        &["harness", "parity", "--check", "--out", out_text],
    );
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("is stale"));
}
