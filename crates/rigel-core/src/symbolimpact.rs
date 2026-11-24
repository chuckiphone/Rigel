//! Roadmap #36: resolve what changed inside a source file, then find the files that actually
//! reference those items.
//!
//! The package graph answers "which packages can compile against this change". This module adds
//! the narrower question: "which changed function, type or module is named by which file". It
//! reuses the shallow symbol and import readers in [`crate::codegraph`] instead of adding a
//! language parser. The result is evidence for the selector, never a claim of compiler accuracy:
//! an edit whose item cannot be mapped falls back to the whole changed file, and the caller keeps
//! the package-level answer as the floor.

use std::collections::{BTreeMap, BTreeSet, HashSet};
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

    let (reached, unreferenced, truncated) =
        references(root, files, priority_roots, scopes, &changed_symbols);
    impact.reached = reached;
    if truncated {
        impact
            .notes
            .push("the reference scan hit its file or byte bound".into());
    }
    if !unreferenced.is_empty() {
        impact.notes.push(format!(
            "no scanned reference to: {}",
            unreferenced.join(", ")
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

fn references(
    root: &Path,
    files: &[String],
    priority_roots: &[String],
    scopes: &[PackageScope],
    changed: &[ChangedSymbol],
) -> (Vec<SymbolReach>, Vec<String>, bool) {
    let searchable: Vec<&ChangedSymbol> = changed
        .iter()
        .filter(|symbol| symbol.name.len() >= 3 && !codegraph::is_common(&symbol.name))
        .collect();
    if searchable.is_empty() {
        return (Vec::new(), Vec::new(), false);
    }
    let changed_keys: BTreeSet<(String, String)> = searchable
        .iter()
        .map(|symbol| (symbol.file.clone(), symbol.name.clone()))
        .collect();
    let by_key: BTreeMap<(String, String), &ChangedSymbol> = searchable
        .iter()
        .map(|symbol| ((symbol.file.clone(), symbol.name.clone()), *symbol))
        .collect();
    let mut by_symbol: BTreeMap<(String, String), Vec<SymbolReference>> = searchable
        .iter()
        .map(|symbol| ((symbol.file.clone(), symbol.name.clone()), Vec::new()))
        .collect();

    // Spend the scan budget first on tests anywhere, then round-robin the packages the package
    // graph selected so one large crate cannot exhaust the budget before its dependents are
    // scanned.
    let mut ordered: Vec<&String> = Vec::new();
    let mut seen = HashSet::new();
    for file in files {
        if is_test_path(file) && seen.insert(file.as_str()) {
            ordered.push(file);
        }
    }
    let roots: Vec<&str> = priority_roots
        .iter()
        .map(String::as_str)
        .filter(|root| !root.is_empty())
        .collect();
    let mut buckets: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for file in files {
        if seen.contains(file.as_str()) {
            continue;
        }
        if let Some(root) = roots
            .iter()
            .filter(|root| within(file, root))
            .max_by_key(|root| root.len())
        {
            buckets.entry(root).or_default().push(file);
        }
    }
    let mut took = true;
    while took {
        took = false;
        for root in &roots {
            let Some(bucket) = buckets.get_mut(root) else {
                continue;
            };
            while let Some(file) = bucket.first().copied() {
                bucket.remove(0);
                if seen.insert(file.as_str()) {
                    ordered.push(file);
                    took = true;
                    break;
                }
            }
        }
    }
    for file in files {
        if seen.insert(file.as_str()) {
            ordered.push(file);
        }
    }

    let mut scanned = 0usize;
    let mut bytes_scanned = 0u64;
    let mut truncated = false;
    for path in ordered {
        if scanned >= MAX_SYMBOL_FILES || bytes_scanned >= MAX_SYMBOL_SCAN_BYTES {
            truncated = true;
            break;
        }
        if codegraph::language(path).is_none() || is_skipped(path) {
            continue;
        }
        let full = root.join(path);
        let Ok(meta) = fs::metadata(&full) else {
            continue;
        };
        if meta.len() > MAX_SYMBOL_FILE_BYTES {
            truncated = true;
            continue;
        }
        if bytes_scanned.saturating_add(meta.len()) > MAX_SYMBOL_SCAN_BYTES {
            truncated = true;
            break;
        }
        let Ok(bytes) = fs::read(&full) else {
            continue;
        };
        scanned += 1;
        bytes_scanned = bytes_scanned.saturating_add(meta.len());
        let text = String::from_utf8_lossy(&bytes);
        let package = package_label(path, scopes);
        let test = is_test_path(path);
        let imported: HashSet<(String, String)> = searchable
            .iter()
            .filter(|symbol| import_evidence(path, &text, symbol))
            .map(|symbol| (symbol.file.clone(), symbol.name.clone()))
            .collect();
        let definitions: HashSet<(String, usize)> = codegraph::symbols(path, &text)
            .into_iter()
            .map(|symbol| (symbol.name, symbol.line))
            .collect();
        for (line_number, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if is_comment(trimmed) {
                continue;
            }
            for (file, name) in &changed_keys {
                if !codegraph::contains_word(line, name) {
                    continue;
                }
                let Some(symbol) = by_key.get(&(file.clone(), name.clone())) else {
                    continue;
                };
                if !reference_allowed(path, line, symbol, &imported) {
                    continue;
                }
                // The declaration itself is not a use of the item.
                if path == file && line_number + 1 == definition_line(changed, file, name) {
                    continue;
                }
                if definitions.contains(&(name.clone(), line_number + 1)) {
                    continue;
                }
                let references = by_symbol.entry((file.clone(), name.clone())).or_default();
                if references.len() >= MAX_REFERENCES_PER_SYMBOL {
                    truncated = true;
                    continue;
                }
                let kind = if test {
                    "test"
                } else if is_import_line(trimmed) {
                    "import"
                } else {
                    "source"
                };
                let reference = SymbolReference {
                    path: path.clone(),
                    package: package.clone(),
                    line: line_number + 1,
                    kind: kind.to_string(),
                };
                if !references.contains(&reference) {
                    references.push(reference);
                }
            }
        }
    }

    let mut reached = Vec::new();
    let mut unreferenced = Vec::new();
    for symbol in searchable {
        let key = (symbol.file.clone(), symbol.name.clone());
        let references = by_symbol.remove(&key).unwrap_or_default();
        if references.is_empty() {
            unreferenced.push(symbol.name.clone());
            continue;
        }
        let packages: Vec<String> = references
            .iter()
            .map(|reference| reference.package.clone())
            .filter(|package| !package.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        reached.push(SymbolReach {
            symbol: symbol.name.clone(),
            file: symbol.file.clone(),
            references,
            packages,
        });
    }
    (reached, unreferenced, truncated)
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

fn import_evidence(path: &str, text: &str, symbol: &ChangedSymbol) -> bool {
    let language = codegraph::language(path).unwrap_or_default();
    let module = rust_module_path(&symbol.file);
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
                if !codegraph::contains_word(line, &symbol.name) {
                    continue;
                }
                if module.is_empty() {
                    if compact.contains(&format!("::{}", symbol.name)) {
                        return true;
                    }
                } else if compact.contains(&format!("{module}::{}", symbol.name))
                    || compact.contains(&format!("{module}::{{"))
                {
                    return true;
                }
            }
            "typescript" | "javascript" | "web" => {
                if (line.starts_with("import ") || line.contains("require("))
                    && codegraph::contains_word(line, &symbol.name)
                {
                    return true;
                }
            }
            "python"
                if (line.starts_with("from ") || line.starts_with("import "))
                    && codegraph::contains_word(line, &symbol.name) =>
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
    symbol: &ChangedSymbol,
    imported: &HashSet<(String, String)>,
) -> bool {
    if path == symbol.file {
        return true;
    }
    let key = (symbol.file.clone(), symbol.name.clone());
    if imported.contains(&key) {
        return true;
    }
    let compact: String = line
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    match symbol.language.as_str() {
        "rust" => {
            let module = rust_module_path(&symbol.file);
            if module.is_empty() {
                compact.contains(&format!("::{}", symbol.name))
            } else {
                compact.contains(&format!("{module}::{name}", name = symbol.name))
            }
        }
        "typescript" | "javascript" | "web" | "python" => {
            compact.contains(&format!(".{}", symbol.name))
        }
        "go" => {
            if Path::new(path).parent() == Path::new(&symbol.file).parent() {
                return true;
            }
            compact.contains(&format!(".{}", symbol.name))
        }
        _ => false,
    }
}

fn definition_line(changed: &[ChangedSymbol], file: &str, name: &str) -> usize {
    changed
        .iter()
        .find(|symbol| symbol.file == file && symbol.name == name)
        .map(|symbol| symbol.line)
        .unwrap_or(0)
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
}
