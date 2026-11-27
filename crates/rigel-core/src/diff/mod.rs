//! Text diffs with one source of truth for hunk boundaries (roadmap 123).
//!
//! Hunks are always computed line by line. Word-level mode only replaces how the text *inside* a
//! hunk is split into changes, so the two modes can never disagree about which lines changed.
//! Above [`WORD_DIFF_BYTE_LIMIT`] bytes of combined input, word mode degrades to line mode before
//! any tokenising work is done.

pub mod word;

pub use word::{WordChange, word_diff};

/// Combined old/new byte count above which a word diff degrades to a line diff. Keeping the unit
/// in bytes (not characters or tokens) makes the cut-off cheap to check and independent of Unicode.
pub const WORD_DIFF_BYTE_LIMIT: usize = 8 * 1024;

/// Granularity used when rendering a hunk's changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    /// Split changed text into word tokens, keeping whitespace as a token.
    Word,
    /// Treat each changed line as one change.
    Line,
}

/// Location and size of one changed region, in 1-based line numbers. The same metadata is
/// reported by both modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkMeta {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
}

/// A changed region plus its verbatim text on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub old_text: String,
    pub new_text: String,
}

impl DiffHunk {
    pub fn metadata(&self) -> HunkMeta {
        HunkMeta {
            old_start: self.old_start,
            old_lines: self.old_lines,
            new_start: self.new_start,
            new_lines: self.new_lines,
        }
    }

    /// Changes within this hunk at the requested granularity. Hunk boundaries never depend on
    /// the mode; only the spans returned here do.
    pub fn edits(&self, mode: DiffMode) -> Vec<WordChange> {
        match mode {
            DiffMode::Word => word_diff(&self.old_text, &self.new_text),
            DiffMode::Line => line_edits(&self.old_text, &self.new_text),
        }
    }
}

/// The hunks for one comparison, together with the mode actually used and the input size that
/// decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReport {
    pub mode: DiffMode,
    pub bytes: usize,
    pub hunks: Vec<DiffHunk>,
}

impl DiffReport {
    /// Hunk boundaries and metadata, independent of the rendering mode.
    pub fn metadata(&self) -> Vec<HunkMeta> {
        self.hunks.iter().map(DiffHunk::metadata).collect()
    }
}

/// Compare two texts, degrading to line granularity once the combined input is larger than
/// [`WORD_DIFF_BYTE_LIMIT`].
pub fn diff(old: &str, new: &str) -> DiffReport {
    let bytes = old.len().saturating_add(new.len());
    let mode = if bytes > WORD_DIFF_BYTE_LIMIT {
        DiffMode::Line
    } else {
        DiffMode::Word
    };
    diff_with_mode(old, new, mode)
}

/// Compare two texts with an explicit mode. The hunks are identical for every mode; callers that
/// force [`DiffMode::Word`] on a large input are responsible for the extra work.
pub fn diff_with_mode(old: &str, new: &str, mode: DiffMode) -> DiffReport {
    let bytes = old.len().saturating_add(new.len());
    DiffReport {
        mode,
        bytes,
        hunks: hunks(old, new),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditOp {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

/// Classic LCS over slices of comparable lines or tokens. Ties prefer removals, which keeps a
/// replaced line as a small delete/add pair rather than a distant match.
fn lcs_ops<T: PartialEq>(old: &[T], new: &[T]) -> Vec<EditOp> {
    let (n, m) = (old.len(), new.len());
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(EditOp::Same(i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            ops.push(EditOp::Removed(i));
            i += 1;
        } else {
            ops.push(EditOp::Added(j));
            j += 1;
        }
    }
    while i < n {
        ops.push(EditOp::Removed(i));
        i += 1;
    }
    while j < m {
        ops.push(EditOp::Added(j));
        j += 1;
    }
    ops
}

fn push_line(target: &mut String, line: &str) {
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(line);
}

/// Compute hunks once, from lines. Word mode reuses these exact boundaries.
fn hunks(old: &str, new: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let (mut old_cursor, mut new_cursor) = (0usize, 0usize);
    for op in lcs_ops(&old_lines, &new_lines) {
        match op {
            EditOp::Same(o, n) => {
                if let Some(hunk) = current.take() {
                    out.push(hunk);
                }
                old_cursor = o + 1;
                new_cursor = n + 1;
            }
            EditOp::Removed(index) => {
                let hunk = current.get_or_insert_with(|| DiffHunk {
                    old_start: old_cursor + 1,
                    old_lines: 0,
                    new_start: new_cursor + 1,
                    new_lines: 0,
                    old_text: String::new(),
                    new_text: String::new(),
                });
                hunk.old_lines += 1;
                push_line(&mut hunk.old_text, old_lines[index]);
                old_cursor = index + 1;
            }
            EditOp::Added(index) => {
                let hunk = current.get_or_insert_with(|| DiffHunk {
                    old_start: old_cursor + 1,
                    old_lines: 0,
                    new_start: new_cursor + 1,
                    new_lines: 0,
                    old_text: String::new(),
                    new_text: String::new(),
                });
                hunk.new_lines += 1;
                push_line(&mut hunk.new_text, new_lines[index]);
                new_cursor = index + 1;
            }
        }
    }
    if let Some(hunk) = current.take() {
        out.push(hunk);
    }
    out
}

/// Line-granularity changes for a hunk, joined into spans like the word-level script.
fn line_edits(old: &str, new: &str) -> Vec<WordChange> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut edits: Vec<WordChange> = Vec::new();
    for op in lcs_ops(&old_lines, &new_lines) {
        edits.push(match op {
            EditOp::Same(o, _) => WordChange::Same(old_lines[o].to_string()),
            EditOp::Removed(o) => WordChange::Removed(old_lines[o].to_string()),
            EditOp::Added(n) => WordChange::Added(new_lines[n].to_string()),
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_input_gets_word_level_edits_within_one_hunk() {
        let report = diff("hello world\n", "hello brave world\n");
        assert_eq!(report.mode, DiffMode::Word);
        assert_eq!(report.hunks.len(), 1);
        let hunk = &report.hunks[0];
        assert_eq!(
            hunk.metadata(),
            HunkMeta {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1
            }
        );
        let edits = hunk.edits(DiffMode::Word);
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Added("brave ".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("hello".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same(" ".to_string()))
        );
    }

    #[test]
    fn input_above_word_diff_byte_limit_degrades_to_line_mode() {
        let big = "x".repeat(WORD_DIFF_BYTE_LIMIT);
        let changed = format!("{big}\nsecond line");
        let report = diff(&big, &changed);
        assert_eq!(report.mode, DiffMode::Line);
        assert_eq!(report.bytes, big.len() + changed.len());
        assert!(report.bytes > WORD_DIFF_BYTE_LIMIT);
        assert_eq!(report.hunks.len(), 1);
        assert_eq!(
            report.hunks[0].metadata(),
            HunkMeta {
                old_start: 2,
                old_lines: 0,
                new_start: 2,
                new_lines: 1
            }
        );
        assert_eq!(
            report.hunks[0].edits(report.mode),
            vec![WordChange::Added("second line".to_string())]
        );

        let half = "x".repeat(WORD_DIFF_BYTE_LIMIT / 2);
        let at_limit = diff(&half, &half);
        assert_eq!(at_limit.bytes, WORD_DIFF_BYTE_LIMIT);
        assert_eq!(at_limit.mode, DiffMode::Word);
        assert!(at_limit.hunks.is_empty());
    }

    #[test]
    fn both_modes_report_identical_hunks() {
        let old = "alpha\nbeta\ngamma\n";
        let new = "alpha\nBETA\ngamma\ndelta\n";
        let word = diff_with_mode(old, new, DiffMode::Word);
        let line = diff_with_mode(old, new, DiffMode::Line);
        assert_eq!(word.hunks, line.hunks);
        assert_eq!(word.metadata(), line.metadata());
        assert_eq!(word.metadata().len(), 2);
        assert_eq!(
            word.metadata()[0],
            HunkMeta {
                old_start: 2,
                old_lines: 1,
                new_start: 2,
                new_lines: 1
            }
        );
        assert_eq!(
            word.metadata()[1],
            HunkMeta {
                old_start: 4,
                old_lines: 0,
                new_start: 4,
                new_lines: 1
            }
        );
        // The granularity differs, but both scripts reconstruct the same two texts.
        for hunk in &word.hunks {
            let rebuilt_old: String = hunk
                .edits(DiffMode::Word)
                .iter()
                .filter(|edit| !matches!(edit, WordChange::Added(_)))
                .map(WordChange::text)
                .collect();
            let rebuilt_new: String = hunk
                .edits(DiffMode::Line)
                .iter()
                .filter(|edit| !matches!(edit, WordChange::Removed(_)))
                .map(WordChange::text)
                .collect();
            assert_eq!(rebuilt_old, hunk.old_text);
            assert_eq!(rebuilt_new, hunk.new_text);
        }
    }

    #[test]
    fn unicode_and_whitespace_boundaries_are_preserved() {
        let old = "naïve\t世界  old\nkeep\n";
        let new = "naïve\t世界  new\nkeep\n";
        let report = diff(old, new);
        assert_eq!(report.hunks.len(), 1);
        let hunk = &report.hunks[0];
        assert_eq!(hunk.old_text, "naïve\t世界  old");
        assert_eq!(hunk.new_text, "naïve\t世界  new");
        let edits = hunk.edits(DiffMode::Word);
        let rebuilt_old: String = edits
            .iter()
            .filter(|edit| !matches!(edit, WordChange::Added(_)))
            .map(WordChange::text)
            .collect();
        let rebuilt_new: String = edits
            .iter()
            .filter(|edit| !matches!(edit, WordChange::Removed(_)))
            .map(WordChange::text)
            .collect();
        assert_eq!(rebuilt_old, hunk.old_text);
        assert_eq!(rebuilt_new, hunk.new_text);
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("naïve".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("\t".to_string()))
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit == &WordChange::Same("世界".to_string()))
        );
    }
}
