//! Rule precedence resolver (roadmap 98).
//!
//! A project can carry `AGENTS.md`, `CLAUDE.md` and `.cursor/rules/*`. When more
//! than one is present Rigel must pick one winner and say which one it picked, with
//! its path and the line the rule starts on, instead of silently merging them.
//! Two rules that disagree produce a diff so the conflict is visible rather than
//! resolved by accident.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleCandidate {
    pub path: String,
    pub kind: String,
    /// 1-based line number where the rule's first non-empty content begins.
    pub line: usize,
    pub bytes: usize,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleConflict {
    pub left: String,
    pub right: String,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleResolution {
    pub winner: Option<RuleCandidate>,
    pub candidates: Vec<RuleCandidate>,
    pub conflicts: Vec<RuleConflict>,
}

/// Precedence order: AGENTS.md, then CLAUDE.md, then .cursor/rules/*.
const PRECEDENCE: &[(&str, &str)] = &[("AGENTS.md", "agents"), ("CLAUDE.md", "claude")];

pub fn resolve(root: &Path) -> RuleResolution {
    let mut candidates: Vec<RuleCandidate> = Vec::new();

    // AGENTS.md wins over CLAUDE.md: only read CLAUDE.md when AGENTS.md is
    // absent or empty.
    let agents_text = read_text(&root.join("AGENTS.md"));
    let agents_wins = agents_text
        .as_ref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);

    for (name, kind) in PRECEDENCE {
        if *kind == "claude" && agents_wins {
            continue;
        }
        let path = root.join(name);
        if let Some(text) = read_text(&path) {
            if text.trim().is_empty() {
                continue;
            }
            candidates.push(candidate(root, &path, kind, &text));
        }
    }

    // .cursor/rules/* always participates, sorted by path.
    let cursor = root.join(".cursor").join("rules");
    if let Ok(entries) = fs::read_dir(&cursor) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e, "md" | "mdc" | "txt"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for path in paths {
            if let Some(text) = read_text(&path) {
                if text.trim().is_empty() {
                    continue;
                }
                candidates.push(candidate(root, &path, "cursor", &text));
            }
        }
    }

    // Winner is the first candidate in precedence order (AGENTS.md already first).
    let winner = candidates.first().cloned();

    // Conflicts: compare every pair of distinct candidates; emit a unified diff
    // when their contents differ.
    let mut conflicts = Vec::new();
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            let left_path = &candidates[i].path;
            let right_path = &candidates[j].path;
            let left_text = read_text(&root.join(left_path)).unwrap_or_default();
            let right_text = read_text(&root.join(right_path)).unwrap_or_default();
            if left_text != right_text {
                conflicts.push(RuleConflict {
                    left: left_path.clone(),
                    right: right_path.clone(),
                    diff: unified_diff(left_path, &left_text, right_path, &right_text),
                });
            }
        }
    }

    RuleResolution {
        winner,
        candidates,
        conflicts,
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn candidate(root: &Path, path: &Path, kind: &str, text: &str) -> RuleCandidate {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let line = text
        .lines()
        .enumerate()
        .find(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i + 1)
        .unwrap_or(1);
    let preview = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    RuleCandidate {
        path: rel,
        kind: kind.to_string(),
        line,
        bytes: text.len(),
        preview,
    }
}

fn unified_diff(left: &str, left_text: &str, right: &str, right_text: &str) -> String {
    // Minimal unified-ish diff: show lines unique to each side.
    let mut out = String::new();
    out.push_str(&format!("--- {left}\n"));
    out.push_str(&format!("+++ {right}\n"));
    for line in left_text.lines() {
        if !right_text.lines().any(|r| r == line) {
            out.push_str(&format!("-{line}\n"));
        }
    }
    for line in right_text.lines() {
        if !left_text.lines().any(|l| l == line) {
            out.push_str(&format!("+{line}\n"));
        }
    }
    out
}

/// Format a human-readable explanation.
pub fn explain(resolution: &RuleResolution) -> String {
    let mut out = String::new();
    match &resolution.winner {
        Some(winner) => {
            out.push_str(&format!(
                "winner: {}:{} (kind: {})\n",
                winner.path, winner.line, winner.kind
            ));
        }
        None => {
            out.push_str("winner: (none)\n");
        }
    }
    out.push_str(&format!("candidates: {}\n", resolution.candidates.len()));
    for c in &resolution.candidates {
        out.push_str(&format!("  - {}:{} ({} bytes)\n", c.path, c.line, c.bytes));
    }
    if !resolution.conflicts.is_empty() {
        out.push_str(&format!("conflicts: {}\n", resolution.conflicts.len()));
        for conflict in &resolution.conflicts {
            out.push_str(&format!(
                "--- {} vs {} ---\n",
                conflict.left, conflict.right
            ));
            out.push_str(&conflict.diff);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn agents_md_wins_over_claude_md() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "agent rules").unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
        let res = resolve(dir.path());
        assert_eq!(res.winner.as_ref().unwrap().path, "AGENTS.md");
        assert_eq!(res.candidates.len(), 1);
    }

    #[test]
    fn claude_md_wins_when_agents_absent() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
        let res = resolve(dir.path());
        assert_eq!(res.winner.as_ref().unwrap().path, "CLAUDE.md");
    }

    #[test]
    fn empty_agents_does_not_silence_claude() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "").unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
        let res = resolve(dir.path());
        assert_eq!(res.winner.as_ref().unwrap().path, "CLAUDE.md");
    }

    #[test]
    fn winner_carries_path_and_line() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "\n\nfirst rule line").unwrap();
        let res = resolve(dir.path());
        let w = res.winner.unwrap();
        assert_eq!(w.path, "AGENTS.md");
        assert_eq!(w.line, 3);
    }

    #[test]
    fn two_conflicting_rules_produce_a_diff() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "rule A").unwrap();
        fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
        fs::write(dir.path().join(".cursor/rules/x.md"), "rule B").unwrap();
        let res = resolve(dir.path());
        assert!(!res.conflicts.is_empty());
        assert!(res.conflicts[0].diff.contains("-rule A"));
        assert!(res.conflicts[0].diff.contains("+rule B"));
    }

    #[test]
    fn identical_rules_do_not_conflict() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "same").unwrap();
        fs::create_dir_all(dir.path().join(".cursor/rules")).unwrap();
        fs::write(dir.path().join(".cursor/rules/x.md"), "same").unwrap();
        let res = resolve(dir.path());
        assert!(res.conflicts.is_empty());
    }

    #[test]
    fn no_rules_yields_no_winner() {
        let dir = tempdir().unwrap();
        let res = resolve(dir.path());
        assert!(res.winner.is_none());
        assert!(res.candidates.is_empty());
    }
}
