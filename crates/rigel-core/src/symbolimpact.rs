//! Roadmap #36: resolve what changed inside a source file, then find the files that actually
//! reference those items.
//!
//! The package graph answers "which packages can compile against this change". This module adds
//! the narrower question: "which changed function, type or module is named by which file, or by a
//! file that reaches it through another declaration". It reuses the shallow symbol and import
//! readers in [`crate::codegraph`] instead of adding a language parser. The result is evidence
//! for the selector, never a claim of compiler accuracy: an edit whose item cannot be mapped
//! falls back to the whole changed file, and the caller keeps the package-level answer as the
//! floor.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

use crate::codegraph;
use crate::process;
use crate::testimpact::ChangedFile;

/// Bound the reference scan so a plan stays a planning command rather than a full index.
pub const MAX_SYMBOL_FILES: usize = 500;
pub const MAX_SYMBOL_FILE_BYTES: u64 = 512 * 1024;
pub const MAX_SYMBOL_SCAN_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_CHANGED_SYMBOLS: usize = 40;
pub const MAX_REFERENCES_PER_SYMBOL: usize = 200;
/// A changed item is a seed; each hop to a caller is one depth. The cap is explicit in the
/// receipt, and a caller beyond it is not guessed.
pub const MAX_REACH_DEPTH: usize = 4;
pub const MAX_REACH_NODES: usize = 600;
pub const MAX_REACH_EDGES: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangedSymbol {
    /// Project-relative file that contains the changed item.
    pub file: String,
    pub name: String,
    /// `fn`, `struct`, `class`, `def`, `func`, `mod`, or the line's declaration keyword.
    pub kind: String,
    pub language: String,
    pub line: usize,
    /// `git diff` when a hunk selected this item, `file scan` when the whole file is assumed.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolReference {
    pub path: String,
    /// Cargo or Node package name; the language name when the file is outside a known package.
    pub package: String,
    pub line: usize,
    /// `test`, `import` or `source`.
    pub kind: String,
    /// `1` is a direct use of the changed item; higher values follow one or more declarations.
    pub depth: usize,
    /// Declaration names from the changed item to the referencing declaration, for example
    /// `["changed", "wrapper", "test"]`. A file-level use ends at the changed item.
    pub via: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolReach {
    pub symbol: String,
    pub file: String,
    pub references: Vec<SymbolReference>,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct SymbolImpact {
    /// `git diff`, `file scan`, or `unavailable`.
    pub source: String,
    pub languages: Vec<String>,
    pub changed: Vec<ChangedSymbol>,
    pub reached: Vec<SymbolReach>,
    pub depth_limit: usize,
    /// Deepest reference actually reported.
    pub max_depth: usize,
    /// Declarations reached through one or more edges, excluding the changed seeds.
    pub reached_symbols: usize,
    /// Edges skipped because the declaration was already reached (a cycle or a revisit).
    pub cycles: usize,
    /// Common declaration names (`run`, `new`, `test`, ...) that were deliberately not used for
    /// reference matching; their package selection remains the fallback.
    pub ambiguous: Vec<String>,
    /// Changed source files where no item could be resolved; their package remains selected.
    pub unresolved: Vec<String>,
    /// Why the result is partial, including an unavailable diff or a module-level edit.
    pub notes: Vec<String>,
}

/// A Cargo or Node package that a reference can belong to. The longest matching root wins, so a
/// nested package is not attributed to its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageScope {
    pub name: String,
    pub root: String,
}

/// Resolve changed symbols from the real Git edit when possible and scan their references.
///
/// This function never returns an error. A missing repository, unreadable file, oversized file or
/// unrecognised language merely leaves a note and the caller falls back to package selection.
pub fn analyze(
    root: &Path,
    since: &str,
    changed: &[ChangedFile],
    scopes: &[PackageScope],
    files: &[String],
    priority_roots: &[String],
) -> SymbolImpact {
    let mut impact = SymbolImpact {
        source: "unavailable".into(),
        ..SymbolImpact::default()
    };
    let diff = git_diff_ranges(root, since, changed);
    let mut diff_used = false;
    let mut changed_symbols = Vec::new();
    let mut languages = BTreeSet::new();
    let mut ambiguous = BTreeSet::new();

    for change in changed {
        let path = change.path.as_str();
        let Some(language) = codegraph::language(path) else {
            continue;
        };
        if is_skipped(path) {
            continue;
        }
        let full = root.join(path);
        let Ok(meta) = fs::metadata(&full) else {
            impact
                .unresolved
                .push(format!("{path}: file cannot be read"));
            continue;
        };
        if meta.len() > MAX_SYMBOL_FILE_BYTES {
            impact.unresolved.push(format!(
                "{path}: larger than {} bytes",
                MAX_SYMBOL_FILE_BYTES
            ));
            continue;
        }
        let Ok(bytes) = fs::read(&full) else {
            impact
                .unresolved
                .push(format!("{path}: file cannot be read"));
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let symbols = codegraph::symbols(path, &text);
        if symbols.is_empty() {
            impact
                .unresolved
                .push(format!("{path}: no top-level symbol could be read"));
            continue;
        }
        languages.insert(language.to_string());

        let ranges = diff.as_ref().and_then(|diff| diff.get(path));
        let new_file = change.status == "?" || change.status.starts_with('A');
        let (selected, source) = if let Some(ranges) = ranges.filter(|ranges| !ranges.is_empty()) {
            diff_used = true;
            let selected = select_symbols(&symbols, ranges);
            if selected.is_empty() {
                // A module-level edit (an import, attribute or deleted item) can change every
                // item in the file, so treat the whole file as changed rather than guessing.
                impact.notes.push(format!(
                    "{path}: edit did not land inside one item; all top-level symbols are treated as changed"
                ));
                (symbols, "git diff")
            } else {
                (selected, "git diff")
            }
        } else if diff.is_some() && !new_file {
            // Git answered and there is no hunk for an existing file: an explicit path is not
            // an edit, so do not guess every symbol in it. The package fallback still applies.
            impact.unresolved.push(format!(
                "{path}: Git reports no changed hunk; package fallback applies"
            ));
            continue;
        } else {
            (symbols, "file scan")
        };

        let lines: Vec<&str> = text.lines().collect();
        for symbol in selected {
            if symbol.name.len() < 3 || codegraph::is_common(&symbol.name) {
                ambiguous.insert(symbol.name.clone());
            }
            changed_symbols.push(ChangedSymbol {
                file: path.to_string(),
                name: symbol.name.clone(),
                kind: symbol_kind(
                    lines
                        .get(symbol.line.saturating_sub(1))
                        .copied()
                        .unwrap_or(""),
                ),
                language: language.to_string(),
                line: symbol.line,
                source: source.to_string(),
            });
        }
    }

    changed_symbols.sort_by(|left, right| {
        (&left.file, left.line, &left.name).cmp(&(&right.file, right.line, &right.name))
    });
    changed_symbols.dedup_by(|left, right| left.file == right.file && left.name == right.name);
    if changed_symbols.len() > MAX_CHANGED_SYMBOLS {
        impact.notes.push(format!(
            "{} changed symbols found; only the first {MAX_CHANGED_SYMBOLS} are scanned",
            changed_symbols.len()
        ));
        changed_symbols.truncate(MAX_CHANGED_SYMBOLS);
    }

    if diff_used {
        impact.source = "git diff".into();
    } else if !changed_symbols.is_empty() {
        impact.source = "file scan".into();
    }
    if diff.is_none() && !changed_symbols.is_empty() {
        impact.notes.push(
            "git diff was unavailable; every top-level symbol in each changed file is scanned"
                .into(),
        );
    }
    impact.languages = languages.into_iter().collect();

    impact.ambiguous = ambiguous.into_iter().collect();
    if !impact.ambiguous.is_empty() {
        impact.notes.push(format!(
            "common symbol name(s) not used for reference matching: {}; package fallback applies",
            impact.ambiguous.join(", ")
        ));
    }

    let report = references(root, files, priority_roots, scopes, &changed_symbols);
    impact.reached = report.reached;
    impact.depth_limit = MAX_REACH_DEPTH;
    impact.max_depth = report.max_depth;
    impact.reached_symbols = report.reached_symbols;
    impact.cycles = report.cycles;
    impact.unresolved.extend(report.unresolved);
    if report.file_truncated {
        impact
            .notes
            .push("the reference scan hit its file or byte bound".into());
    }
    if report.edges_capped {
        impact.notes.push(format!(
            "the symbol reach graph hit its {MAX_REACH_EDGES}-edge bound"
        ));
    }
    if report.nodes_capped {
        impact.notes.push(format!(
            "the symbol reach walk hit its {MAX_REACH_NODES}-declaration bound"
        ));
    }
    if report.depth_capped {
        impact.notes.push(format!(
            "the symbol reach walk stopped at depth {MAX_REACH_DEPTH}; deeper callers were not scanned"
        ));
    }
    if report.cycles > 0 {
        impact.notes.push(format!(
            "the cycle/revisit guard skipped {} already-reached edge(s)",
            report.cycles
        ));
    }
    if !report.unreferenced.is_empty() {
        impact.notes.push(format!(
            "no scanned reference to: {}",
            report.unreferenced.join(", ")
        ));
    }
    impact.changed = changed_symbols;
    impact
}

/// The item whose body contains a changed line. `body_end` is an exclusive zero-based index, so
/// it is also the one-based last line of the body.
fn select_symbols(
    symbols: &[codegraph::Symbol],
    ranges: &[(usize, usize)],
) -> Vec<codegraph::Symbol> {
    symbols
        .iter()
        .filter(|symbol| {
            let start = symbol.line.max(1);
            let end = symbol.body_end.max(symbol.line).max(1);
            ranges
                .iter()
                .any(|(changed_start, changed_end)| *changed_start <= end && *changed_end >= start)
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
struct SymbolDef {
    file: String,
    name: String,
    line: usize,
    body_end: usize,
    language: String,
}

impl SymbolDef {
    fn key(&self) -> (String, String) {
        (self.file.clone(), self.name.clone())
    }
}

#[derive(Debug, Clone)]
struct ScannedFile {
    path: String,
    text: String,
    line_ranges: Vec<(usize, usize)>,
    package: String,
    test: bool,
    symbols: Vec<SymbolDef>,
}

struct ReachReport {
    reached: Vec<SymbolReach>,
    unreferenced: Vec<String>,
    unresolved: Vec<String>,
    file_truncated: bool,
    edges_capped: bool,
    nodes_capped: bool,
    depth_capped: bool,
    cycles: usize,
    max_depth: usize,
    reached_symbols: usize,
}

fn empty_report() -> ReachReport {
    ReachReport {
        reached: Vec::new(),
        unreferenced: Vec::new(),
        unresolved: Vec::new(),
        file_truncated: false,
        edges_capped: false,
        nodes_capped: false,
        depth_capped: false,
        cycles: 0,
        max_depth: 0,
        reached_symbols: 0,
    }
}

fn references(
    root: &Path,
    files: &[String],
    priority_roots: &[String],
    scopes: &[PackageScope],
    changed: &[ChangedSymbol],
) -> ReachReport {
    let searchable: Vec<&ChangedSymbol> = changed
        .iter()
        .filter(|symbol| symbol.name.len() >= 3 && !codegraph::is_common(&symbol.name))
        .collect();
    if searchable.is_empty() {
        return empty_report();
    }

    let mut report = empty_report();
    let ordered = ordered_paths(files, changed, priority_roots);
    let mut scanned_files = Vec::new();
    let mut scanned = 0usize;
    let mut bytes_scanned = 0u64;
    for path in ordered {
        if scanned >= MAX_SYMBOL_FILES || bytes_scanned >= MAX_SYMBOL_SCAN_BYTES {
            report.file_truncated = true;
            break;
        }
        let Some(language) = codegraph::language(&path) else {
            continue;
        };
        if is_skipped(&path) {
            continue;
        }
        let full = root.join(&path);
        let Ok(meta) = fs::metadata(&full) else {
            continue;
        };
        if meta.len() > MAX_SYMBOL_FILE_BYTES {
            report.file_truncated = true;
            continue;
        }
        if bytes_scanned.saturating_add(meta.len()) > MAX_SYMBOL_SCAN_BYTES {
            report.file_truncated = true;
            break;
        }
        let Ok(bytes) = fs::read(&full) else {
            continue;
        };
        scanned += 1;
        bytes_scanned = bytes_scanned.saturating_add(meta.len());
        let text = String::from_utf8_lossy(&bytes);
        let mut line_ranges = Vec::new();
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let content = line.trim_end_matches(['\n', '\r']);
            line_ranges.push((offset, offset + content.len()));
            offset += line.len();
        }
        let symbols = codegraph::symbols(&path, &text)
            .into_iter()
            .map(|symbol| SymbolDef {
                file: path.clone(),
                name: symbol.name,
                line: symbol.line,
                body_end: symbol.body_end,
                language: language.to_string(),
            })
            .collect();
        scanned_files.push(ScannedFile {
            path: path.clone(),
            text: text.to_string(),
            line_ranges,
            package: package_label(&path, scopes),
            test: is_test_path(&path),
            symbols,
        });
    }

    let mut defs: Vec<SymbolDef> = Vec::new();
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut by_key: BTreeMap<(String, String), usize> = BTreeMap::new();
    for file in &scanned_files {
        for symbol in &file.symbols {
            let key = symbol.key();
            if by_key.contains_key(&key) {
                continue;
            }
            let index = defs.len();
            by_key.insert(key, index);
            by_name.entry(symbol.name.clone()).or_default().push(index);
            defs.push(symbol.clone());
        }
    }

    // Index every declaration name's occurrences. Reverse uses are resolved on demand during the
    // walk below: one dense crate's unrelated references must not exhaust the edge budget before
    // the changed item's chain has been seen.
    let mut occurrences: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (file_index, file) in scanned_files.iter().enumerate() {
        for (line_index, &(start, end)) in file.line_ranges.iter().enumerate() {
            let line = &file.text[start..end];
            for word in line_words(line) {
                if !by_name.contains_key(word) {
                    continue;
                }
                let rows = occurrences.entry(word.to_string()).or_default();
                if rows.last() != Some(&(file_index, line_index)) {
                    rows.push((file_index, line_index));
                }
            }
        }
    }

    for symbol in &searchable {
        let root_key = (symbol.file.clone(), symbol.name.clone());
        let Some(&root_index) = by_key.get(&root_key) else {
            report.unresolved.push(format!(
                "{}::{}: changed item is outside the scanned declaration index; package fallback applies",
                symbol.file, symbol.name
            ));
            continue;
        };
        let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
        visited.insert(root_key.clone());
        let mut queue: VecDeque<(usize, Vec<String>, usize)> = VecDeque::new();
        queue.push_back((root_index, vec![symbol.name.clone()], 0));
        let mut rows: Vec<SymbolReference> = Vec::new();
        let mut seen: BTreeSet<(String, usize, String)> = BTreeSet::new();
        let mut edges = 0usize;
        'walk: while let Some((target_index, chain, depth)) = queue.pop_front() {
            let target = &defs[target_index];
            let Some(found) = occurrences.get(&target.name) else {
                continue;
            };
            if depth >= MAX_REACH_DEPTH {
                if found.iter().any(|&(file_index, line_index)| {
                    let file = &scanned_files[file_index];
                    let (start, end) = file.line_ranges[line_index];
                    let line = &file.text[start..end];
                    !is_comment(line.trim_start())
                        && !(target.file == file.path && target.line == line_index + 1)
                }) {
                    report.depth_capped = true;
                }
                continue;
            }
            let mut import_cache: BTreeMap<usize, bool> = BTreeMap::new();
            for &(file_index, line_index) in found {
                let file = &scanned_files[file_index];
                let (start, end) = file.line_ranges[line_index];
                let line = &file.text[start..end];
                let trimmed = line.trim_start();
                if is_comment(trimmed) {
                    continue;
                }
                // The declaration line itself is a definition, not a use.
                if target.file == file.path && target.line == line_index + 1 {
                    continue;
                }
                let imported = *import_cache.entry(file_index).or_insert_with(|| {
                    import_evidence(&file.path, &file.text, &target.file, &target.name)
                });
                if !reference_allowed(
                    &file.path,
                    line,
                    &target.file,
                    &target.name,
                    &target.language,
                    imported,
                ) {
                    continue;
                }
                let source: Vec<&SymbolDef> = if is_import_line(trimmed) {
                    Vec::new()
                } else {
                    file.symbols
                        .iter()
                        .filter(|symbol| {
                            line_index + 1 >= symbol.line && line_index < symbol.body_end
                        })
                        .collect()
                };
                let kind = if file.test {
                    "test"
                } else if is_import_line(trimmed) {
                    "import"
                } else {
                    "source"
                };
                let next_depth = depth + 1;
                if source.is_empty() {
                    if edges >= MAX_REACH_EDGES {
                        report.edges_capped = true;
                        break 'walk;
                    }
                    edges += 1;
                    if seen.insert((file.path.clone(), line_index + 1, kind.to_string()))
                        && rows.len() < MAX_REFERENCES_PER_SYMBOL
                    {
                        rows.push(SymbolReference {
                            path: file.path.clone(),
                            package: file.package.clone(),
                            line: line_index + 1,
                            kind: kind.to_string(),
                            depth: next_depth,
                            via: chain.clone(),
                        });
                        report.max_depth = report.max_depth.max(next_depth);
                    }
                    continue;
                }
                for declaration in &source {
                    let source_key = declaration.key();
                    if source_key == (target.file.clone(), target.name.clone()) {
                        continue;
                    }
                    if edges >= MAX_REACH_EDGES {
                        report.edges_capped = true;
                        break 'walk;
                    }
                    edges += 1;
                    if !visited.insert(source_key.clone()) {
                        report.cycles += 1;
                        continue;
                    }
                    let mut via = chain.clone();
                    via.push(declaration.name.clone());
                    if seen.insert((file.path.clone(), line_index + 1, kind.to_string())) {
                        if rows.len() < MAX_REFERENCES_PER_SYMBOL {
                            rows.push(SymbolReference {
                                path: file.path.clone(),
                                package: file.package.clone(),
                                line: line_index + 1,
                                kind: kind.to_string(),
                                depth: next_depth,
                                via: via.clone(),
                            });
                            report.max_depth = report.max_depth.max(next_depth);
                        } else {
                            report.file_truncated = true;
                        }
                    }
                    if report.reached_symbols >= MAX_REACH_NODES {
                        report.nodes_capped = true;
                        continue;
                    }
                    if let Some(&source_index) = by_key.get(&source_key) {
                        report.reached_symbols += 1;
                        queue.push_back((source_index, via, next_depth));
                    }
                }
            }
        }
        if rows.is_empty() {
            report.unreferenced.push(symbol.name.clone());
            continue;
        }
        rows.sort_by(|left, right| {
            (left.depth, &left.path, left.line, &left.kind).cmp(&(
                right.depth,
                &right.path,
                right.line,
                &right.kind,
            ))
        });
        let packages: Vec<String> = rows
            .iter()
            .map(|reference| reference.package.clone())
            .filter(|package| !package.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        report.reached.push(SymbolReach {
            symbol: symbol.name.clone(),
            file: symbol.file.clone(),
            references: rows,
            packages,
        });
    }
    report
}

/// Deterministic scan order: changed files, tests, affected packages round-robin, then the rest.
fn ordered_paths(
    files: &[String],
    changed: &[ChangedSymbol],
    priority_roots: &[String],
) -> Vec<String> {
    let mut ordered: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for change in changed {
        if !is_skipped(&change.file) && seen.insert(change.file.clone()) {
            ordered.push(change.file.clone());
        }
    }
    for file in files {
        if is_test_path(file) && seen.insert(file.clone()) {
            ordered.push(file.clone());
        }
    }
    let roots: Vec<&str> = priority_roots
        .iter()
        .map(String::as_str)
        .filter(|root| !root.is_empty())
        .collect();
    let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        if seen.contains(file) {
            continue;
        }
        if let Some(root) = roots
            .iter()
            .filter(|root| within(file, root))
            .max_by_key(|root| root.len())
        {
            buckets
                .entry(root.to_string())
                .or_default()
                .push(file.clone());
        }
    }
    let mut took = true;
    while took {
        took = false;
        for root in &roots {
            let Some(bucket) = buckets.get_mut(*root) else {
                continue;
            };
            while let Some(file) = bucket.first().cloned() {
                bucket.remove(0);
                if seen.insert(file.clone()) {
                    ordered.push(file);
                    took = true;
                    break;
                }
            }
        }
    }
    for file in files {
        if seen.insert(file.clone()) {
            ordered.push(file.clone());
        }
    }
    ordered
}

fn line_words(line: &str) -> Vec<&str> {
    line.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '_' || character == '$')
    })
    .filter(|word| word.len() >= 3)
    .collect()
}

/// A Rust file's module path inside its crate. `src/lib.rs`, `src/main.rs` and a `mod.rs` file
/// name the parent module, so they are dropped from the path.
fn rust_module_path(file: &str) -> String {
    let normalized = file.replace('\\', "/");
    let relative = normalized
        .split_once("/src/")
        .map(|(_, rest)| rest)
        .or_else(|| normalized.strip_prefix("src/"))
        .unwrap_or(&normalized);
    let Some(stem) = relative.strip_suffix(".rs") else {
        return String::new();
    };
    let mut parts: Vec<&str> = stem.split('/').filter(|part| !part.is_empty()).collect();
    if matches!(parts.last().copied(), Some("lib" | "main" | "mod")) {
        parts.pop();
    }
    parts.join("::")
}

fn import_evidence(path: &str, text: &str, target_file: &str, target_name: &str) -> bool {
    let language = codegraph::language(path).unwrap_or_default();
    let module = rust_module_path(target_file);
    for line in text.lines() {
        let line = line.trim();
        let compact: String = line
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        match language {
            "rust" => {
                if !line.starts_with("use ") && !line.starts_with("pub use ") {
                    continue;
                }
                if !codegraph::contains_word(line, target_name) {
                    continue;
                }
                if module.is_empty() {
                    if compact.contains(&format!("::{target_name}")) {
                        return true;
                    }
                } else if compact.contains(&format!("{module}::{target_name}"))
                    || compact.contains(&format!("{module}::{{"))
                {
                    return true;
                }
            }
            "typescript" | "javascript" | "web" => {
                if (line.starts_with("import ") || line.contains("require("))
                    && codegraph::contains_word(line, target_name)
                {
                    return true;
                }
            }
            "python"
                if (line.starts_with("from ") || line.starts_with("import "))
                    && codegraph::contains_word(line, target_name) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// A bare name is only a reference when this file imports the item or the language requires a
/// qualified path for it. This is what keeps an unrelated `plan::propose` or a local helper named
/// `run` from selecting another package's tests.
fn reference_allowed(
    path: &str,
    line: &str,
    target_file: &str,
    target_name: &str,
    target_language: &str,
    imported: bool,
) -> bool {
    if path == target_file || imported {
        return true;
    }
    let compact: String = line
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    match target_language {
        "rust" => {
            let module = rust_module_path(target_file);
            if module.is_empty() {
                compact.contains(&format!("::{target_name}"))
            } else {
                compact.contains(&format!("{module}::{target_name}"))
            }
        }
        "typescript" | "javascript" | "web" | "python" => {
            compact.contains(&format!(".{target_name}"))
        }
        "go" => {
            if Path::new(path).parent() == Path::new(target_file).parent() {
                return true;
            }
            compact.contains(&format!(".{target_name}"))
        }
        _ => false,
    }
}

fn package_label(path: &str, scopes: &[PackageScope]) -> String {
    scopes
        .iter()
        .filter(|scope| within(path, &scope.root))
        .max_by_key(|scope| scope.root.len())
        .map(|scope| scope.name.clone())
        .unwrap_or_else(|| codegraph::language(path).unwrap_or_default().to_string())
}

fn within(path: &str, root: &str) -> bool {
    root.is_empty() || path == root || path.starts_with(&format!("{root}/"))
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/__tests__/")
        || lower.starts_with("__tests__/")
        || name.ends_with("_test.go")
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_test.py")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn is_comment(trimmed: &str) -> bool {
    trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with('#')
        || trimmed.starts_with("--")
}

fn is_import_line(trimmed: &str) -> bool {
    trimmed.starts_with("use ")
        || trimmed.starts_with("pub use ")
        || trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.starts_with("require(")
        || trimmed.starts_with("mod ")
}

fn is_skipped(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    [
        "target/",
        "node_modules/",
        "vendor/",
        "dist/",
        "build/",
        ".git/",
        ".next/",
        "coverage/",
        "pods/",
        ".venv/",
        "venv/",
        "__pycache__/",
        ".rigel/",
    ]
    .iter()
    .any(|dir| lower.starts_with(dir) || lower.contains(&format!("/{dir}")))
}

fn symbol_kind(line: &str) -> String {
    const KINDS: [&str; 18] = [
        "fn",
        "function",
        "func",
        "def",
        "class",
        "struct",
        "enum",
        "trait",
        "impl",
        "interface",
        "type",
        "const",
        "static",
        "let",
        "var",
        "union",
        "mod",
        "module",
    ];
    line.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '_' || character == '$')
    })
    .find(|token| KINDS.contains(&token.to_ascii_lowercase().as_str()))
    .map(|token| token.to_ascii_lowercase())
    .unwrap_or_else(|| "symbol".into())
}

/// `None` means Git could not answer, which makes the caller scan whole files. An empty map means
/// Git answered but none of the requested paths had a hunk (an untracked or unmodified file).
fn git_diff_ranges(
    root: &Path,
    since: &str,
    changed: &[ChangedFile],
) -> Option<BTreeMap<String, Vec<(usize, usize)>>> {
    if changed.is_empty() {
        return None;
    }
    let since = if since.trim().is_empty() {
        "HEAD"
    } else {
        since.trim()
    };
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            since,
            "--",
        ])
        .args(changed.iter().map(|change| change.path.as_str()));
    process::hide(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_diff(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_diff(text: &str) -> BTreeMap<String, Vec<(usize, usize)>> {
    static HUNK: OnceLock<Regex> = OnceLock::new();
    let hunk = HUNK.get_or_init(|| {
        Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
            .expect("the unified-diff hunk pattern")
    });
    let mut ranges: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(path) = line.strip_prefix("+++ b/") {
            current = Some(path.to_string());
            continue;
        }
        if line.starts_with("+++ /dev/null") {
            current = None;
            continue;
        }
        let Some(captures) = hunk.captures(line) else {
            continue;
        };
        let Some(path) = &current else {
            continue;
        };
        let Ok(start) = captures.get(1).unwrap().as_str().parse::<usize>() else {
            continue;
        };
        let count = captures
            .get(2)
            .and_then(|value| value.as_str().parse::<usize>().ok())
            .unwrap_or(1);
        let start = start.max(1);
        let end = if count == 0 {
            start
        } else {
            start.saturating_add(count - 1)
        };
        ranges.entry(path.clone()).or_default().push((start, end));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: "M".into(),
        }
    }

    fn scope(name: &str, root: &str) -> PackageScope {
        PackageScope {
            name: name.into(),
            root: root.into(),
        }
    }

    #[test]
    fn a_diff_hunk_maps_to_the_new_file_lines() {
        let diff = "\
diff --git a/crates/core/src/lib.rs b/crates/core/src/lib.rs
--- a/crates/core/src/lib.rs
+++ b/crates/core/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn one() {}
+pub fn two() {}
 pub fn three() {}
@@ -9 +10,0 @@
-old();
";
        let ranges = parse_diff(diff);
        assert_eq!(ranges["crates/core/src/lib.rs"], vec![(1, 3), (10, 10)]);
    }

    #[test]
    fn the_enclosing_function_is_selected_from_a_changed_line() {
        let text = "pub fn one() {}\n\npub fn two() {\n    let value = 1;\n}\n";
        let symbols = codegraph::symbols("src/lib.rs", text);
        let selected = select_symbols(&symbols, &[(4, 4)]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "two");
    }

    #[test]
    fn a_git_failure_falls_back_to_the_whole_file() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("crates/core/src")).unwrap();
        fs::write(
            project.path().join("crates/core/src/lib.rs"),
            "pub fn core() {}\n",
        )
        .unwrap();
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("crates/core/src/lib.rs")],
            &[scope("core", "crates/core")],
            &["crates/core/src/lib.rs".to_string()],
            &["crates/core".to_string()],
        );
        assert_eq!(impact.source, "file scan");
        assert_eq!(impact.changed[0].name, "core");
        assert_eq!(impact.changed[0].kind, "fn");
        assert_eq!(impact.changed[0].source, "file scan");
    }

    #[test]
    fn references_in_tests_and_sources_are_found() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("crates/core/src")).unwrap();
        fs::create_dir_all(project.path().join("crates/app/src")).unwrap();
        fs::create_dir_all(project.path().join("crates/app/tests")).unwrap();
        fs::write(
            project.path().join("crates/core/src/lib.rs"),
            "pub fn core() -> u32 { 1 }\n",
        )
        .unwrap();
        fs::write(
            project.path().join("crates/app/src/lib.rs"),
            "pub fn app() -> u32 { demo_core::core() }\n",
        )
        .unwrap();
        fs::write(
            project.path().join("crates/app/tests/app.rs"),
            "use demo_core::core;\n#[test]\nfn app_test() { assert_eq!(core(), 1); }\n",
        )
        .unwrap();
        let files = vec![
            "crates/core/src/lib.rs".to_string(),
            "crates/app/src/lib.rs".to_string(),
            "crates/app/tests/app.rs".to_string(),
        ];
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("crates/core/src/lib.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-app", "crates/app"),
            ],
            &files,
            &["crates/core".to_string(), "crates/app".to_string()],
        );
        assert_eq!(impact.changed.len(), 1);
        assert_eq!(impact.reached.len(), 1);
        let references = &impact.reached[0].references;
        assert!(references.iter().any(|reference| {
            reference.path == "crates/app/tests/app.rs"
                && reference.kind == "test"
                && reference.package == "demo-app"
        }));
        assert!(references.iter().any(|reference| {
            reference.path == "crates/app/src/lib.rs"
                && reference.kind == "source"
                && reference.package == "demo-app"
        }));
        assert!(impact.reached[0].packages.contains(&"demo-app".to_string()));
    }

    #[test]
    fn a_comment_is_not_a_reference() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::write(project.path().join("src/lib.rs"), "pub fn core() {}\n").unwrap();
        fs::write(
            project.path().join("src/other.rs"),
            "// core is mentioned here\npub fn other() {}\n",
        )
        .unwrap();
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("src/lib.rs")],
            &[],
            &["src/lib.rs".to_string(), "src/other.rs".to_string()],
            &[],
        );
        assert!(impact.reached.is_empty());
        assert!(
            impact
                .notes
                .iter()
                .any(|note| note.contains("no scanned reference"))
        );
    }

    #[test]
    fn a_bare_name_without_an_import_or_qualifier_is_not_a_reference() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("crates/core/src")).unwrap();
        fs::create_dir_all(project.path().join("crates/other/src")).unwrap();
        fs::write(
            project.path().join("crates/core/src/testimpact.rs"),
            "pub fn plan() {}\n",
        )
        .unwrap();
        fs::write(
            project.path().join("crates/other/src/lib.rs"),
            "fn run() { plan::propose(); }\n",
        )
        .unwrap();
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("crates/core/src/testimpact.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-other", "crates/other"),
            ],
            &[
                "crates/core/src/testimpact.rs".to_string(),
                "crates/other/src/lib.rs".to_string(),
            ],
            &["crates/core".to_string(), "crates/other".to_string()],
        );
        assert!(impact.reached.is_empty());
    }

    #[test]
    fn an_imported_or_module_qualified_reference_is_found() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("crates/core/src")).unwrap();
        fs::create_dir_all(project.path().join("crates/other/src")).unwrap();
        fs::write(
            project.path().join("crates/core/src/testimpact.rs"),
            "pub fn plan() {}\n",
        )
        .unwrap();
        fs::write(
            project.path().join("crates/other/src/lib.rs"),
            "use demo_core::testimpact::plan;\nfn run() { plan(); }\n",
        )
        .unwrap();
        fs::write(
            project.path().join("crates/other/src/qualified.rs"),
            "fn run() { demo_core::testimpact::plan(); }\n",
        )
        .unwrap();
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("crates/core/src/testimpact.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-other", "crates/other"),
            ],
            &[
                "crates/core/src/testimpact.rs".to_string(),
                "crates/other/src/lib.rs".to_string(),
                "crates/other/src/qualified.rs".to_string(),
            ],
            &["crates/core".to_string(), "crates/other".to_string()],
        );
        assert!(
            impact.reached[0]
                .references
                .iter()
                .any(|reference| reference.path == "crates/other/src/lib.rs")
        );
        assert!(
            impact.reached[0]
                .references
                .iter()
                .any(|reference| reference.path == "crates/other/src/qualified.rs")
        );
    }

    #[test]
    fn a_common_symbol_name_is_package_fallback_not_a_reference_match() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::create_dir_all(project.path().join("tests")).unwrap();
        fs::write(project.path().join("src/lib.rs"), "pub fn run() {}\n").unwrap();
        fs::write(project.path().join("tests/a.rs"), "run();\n").unwrap();
        let impact = analyze(
            project.path(),
            "HEAD",
            &[change("src/lib.rs")],
            &[],
            &["src/lib.rs".to_string(), "tests/a.rs".to_string()],
            &[],
        );
        assert_eq!(impact.ambiguous, vec!["run"]);
        assert!(impact.reached.is_empty());
    }

    fn write(project: &tempfile::TempDir, path: &str, text: &str) {
        let full = project.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }

    fn analyze_files(
        project: &Path,
        files: &[&str],
        changed: &[ChangedFile],
        scopes: &[PackageScope],
    ) -> SymbolImpact {
        let files: Vec<String> = files.iter().map(|path| (*path).to_string()).collect();
        let roots: Vec<String> = scopes.iter().map(|scope| scope.root.clone()).collect();
        analyze(project, "HEAD", changed, scopes, &files, &roots)
    }

    #[test]
    fn an_intermediate_declaration_carries_reach_to_a_test() {
        let project = tempfile::tempdir().unwrap();
        write(&project, "crates/core/src/lib.rs", "pub fn core() {}\n");
        write(
            &project,
            "crates/middle/src/lib.rs",
            "use demo_core::core;\npub fn wrapper() { core(); }\n",
        );
        write(
            &project,
            "crates/app/tests/transitive.rs",
            "use demo_middle::wrapper;\n#[test]\nfn reached() { wrapper(); }\n",
        );
        let impact = analyze_files(
            project.path(),
            &[
                "crates/core/src/lib.rs",
                "crates/middle/src/lib.rs",
                "crates/app/tests/transitive.rs",
            ],
            &[change("crates/core/src/lib.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-middle", "crates/middle"),
                scope("demo-app", "crates/app"),
            ],
        );
        let reach = impact
            .reached
            .iter()
            .find(|reach| reach.symbol == "core")
            .expect("core is reached");
        let test = reach
            .references
            .iter()
            .find(|reference| reference.path == "crates/app/tests/transitive.rs")
            .expect("the test is reached through wrapper");
        assert_eq!(test.kind, "test");
        assert!(test.depth >= 2, "{test:?}");
        assert!(test.via.iter().any(|name| name == "wrapper"), "{test:?}");
        assert!(impact.max_depth >= 2);
        assert!(impact.reached_symbols >= 2);
        assert!(
            !impact
                .notes
                .iter()
                .any(|note| note.contains("stopped at depth"))
        );
    }

    #[test]
    fn a_cycle_terminates_and_does_not_duplicate_references() {
        let project = tempfile::tempdir().unwrap();
        write(&project, "crates/core/src/lib.rs", "pub fn core() {}\n");
        write(
            &project,
            "crates/middle/src/lib.rs",
            "use demo_core::core;\npub fn wrapper() { core(); cycle(); }\npub fn cycle() { wrapper(); }\n",
        );
        write(
            &project,
            "crates/app/tests/cycle.rs",
            "use demo_middle::wrapper;\n#[test]\nfn reached() { wrapper(); }\n",
        );
        let impact = analyze_files(
            project.path(),
            &[
                "crates/core/src/lib.rs",
                "crates/middle/src/lib.rs",
                "crates/app/tests/cycle.rs",
            ],
            &[change("crates/core/src/lib.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-middle", "crates/middle"),
                scope("demo-app", "crates/app"),
            ],
        );
        assert!(impact.cycles > 0);
        assert!(
            impact
                .notes
                .iter()
                .any(|note| note.contains("cycle/revisit guard"))
        );
        let reach = &impact.reached[0];
        let mut keys = BTreeSet::new();
        for reference in &reach.references {
            assert!(
                keys.insert((
                    reference.path.clone(),
                    reference.line,
                    reference.kind.clone()
                )),
                "duplicate reference: {reference:?}"
            );
        }
        assert!(
            reach
                .references
                .iter()
                .any(|reference| reference.path == "crates/app/tests/cycle.rs")
        );
    }

    #[test]
    fn the_depth_cap_is_enforced_and_reported() {
        let project = tempfile::tempdir().unwrap();
        write(&project, "crates/core/src/lib.rs", "pub fn core() {}\n");
        write(
            &project,
            "crates/core/src/one.rs",
            "use demo_core::core;\npub fn one() { core(); }\n",
        );
        write(
            &project,
            "crates/core/src/two.rs",
            "use demo_core::one::one;\npub fn two() { one(); }\n",
        );
        write(
            &project,
            "crates/core/src/three.rs",
            "use demo_core::two::two;\npub fn three() { two(); }\n",
        );
        write(
            &project,
            "crates/core/src/four.rs",
            "use demo_core::three::three;\npub fn four() { three(); }\n",
        );
        write(
            &project,
            "crates/core/src/five.rs",
            "use demo_core::four::four;\npub fn five() { four(); }\n",
        );
        write(
            &project,
            "crates/core/tests/deep.rs",
            "use demo_core::five::five;\n#[test]\nfn deep() { five(); }\n",
        );
        let files = [
            "crates/core/src/lib.rs",
            "crates/core/src/one.rs",
            "crates/core/src/two.rs",
            "crates/core/src/three.rs",
            "crates/core/src/four.rs",
            "crates/core/src/five.rs",
            "crates/core/tests/deep.rs",
        ];
        let impact = analyze_files(
            project.path(),
            &files,
            &[change("crates/core/src/lib.rs")],
            &[scope("demo-core", "crates/core")],
        );
        assert_eq!(impact.depth_limit, MAX_REACH_DEPTH);
        assert!(impact.max_depth <= MAX_REACH_DEPTH);
        assert!(
            impact
                .notes
                .iter()
                .any(|note| note.contains("stopped at depth"))
        );
        assert!(
            !impact.reached[0]
                .references
                .iter()
                .any(|reference| reference.path == "crates/core/tests/deep.rs")
        );
    }

    #[test]
    fn an_unrelated_chain_adds_no_reference() {
        let project = tempfile::tempdir().unwrap();
        write(&project, "crates/core/src/lib.rs", "pub fn core() {}\n");
        write(
            &project,
            "crates/middle/src/lib.rs",
            "pub fn wrapper() { other(); }\npub fn other() {}\n",
        );
        write(
            &project,
            "crates/app/tests/unrelated.rs",
            "use demo_middle::wrapper;\n#[test]\nfn unrelated() { wrapper(); }\n",
        );
        let impact = analyze_files(
            project.path(),
            &[
                "crates/core/src/lib.rs",
                "crates/middle/src/lib.rs",
                "crates/app/tests/unrelated.rs",
            ],
            &[change("crates/core/src/lib.rs")],
            &[
                scope("demo-core", "crates/core"),
                scope("demo-middle", "crates/middle"),
                scope("demo-app", "crates/app"),
            ],
        );
        assert!(impact.reached.is_empty());
        assert!(
            impact
                .notes
                .iter()
                .any(|note| note.contains("no scanned reference"))
        );
    }
}
