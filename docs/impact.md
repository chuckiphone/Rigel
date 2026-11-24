# Change-impact graph

`rigel impact` answers one review question before anything runs: **what does this change
actually reach?** It is the same plan `rigel test plan` builds, printed for a reviewer rather than
for a runner. The package graph is the safety floor; changed items and their references are the
narrower evidence on top of it.

```
rigel impact [--project DIR] [--since REV] [--changed-file PATH]... [--full] [--json]
```

`--changed-file` supplies paths explicitly and skips Git for the changed-file list; otherwise
`--since` (default `HEAD`) is read with `git diff --name-status` plus untracked files. `.rigel/`
is never a project change. The symbol pass still compares an explicit path with `--since` when
Git is available, so it can find the real hunk rather than treating the whole file as edited.

## The package graph

For a Cargo workspace the selector asks Cargo for the workspace graph:

```
cargo metadata --no-deps --format-version 1 --locked --offline --manifest-path <root>/Cargo.toml
```

`--locked` and `--offline` are deliberate: a plan must not rewrite `Cargo.lock` or reach the
network. When the project has no lockfile, or Cargo cannot answer offline, the fallback reads
each `Cargo.toml` for its package name, its `tests/` targets and its path dependencies. The
receipt says which source was used:

```json
"graph": { "source": "cargo metadata", "packages": 5, "edges": 4 }
```

The graph is over **path dependencies**; registry dependencies are not internal edges. A
dependent package is selected because its tests compile against the changed package. That is a
safe over-approximation; symbol reachability never removes it.

## Changed items and references

When Git can answer, the selector reads the real zero-context diff against `--since` and maps
each changed line to the top-level declaration whose body contains it. A module-level edit (an
import, an attribute or a deleted item) is reported as treating every top-level symbol in that
file as changed rather than guessing one. When Git is unavailable, or the path is a new file,
every top-level symbol in the file is scanned and the receipt says `file scan`. An explicit
`--changed-file` path with no Git hunk is not treated as an edit: it is reported as unresolved
and the package answer stands.

The symbol reader is the one already used by the code graph: shallow top-level declarations and
imports, not a compiler. Reference matching is deliberately conservative, and then a bounded
reverse-use walk follows a declaration to the declarations that use it. A test can therefore be
selected through a wrapper even when the test never names the changed item itself:

- **Rust**: a use is accepted when the file imports the item through its module path or writes a
  qualified path such as `testimpact::changed_files`. A bare `plan()` in another module is not a
  reference unless that file imports it; this keeps an unrelated `plan::propose` from selecting
  a package. Crate-root symbols accept `crate::name` and `rigel_core::name` forms.
- **TypeScript / JavaScript**: top-level exported symbols; a use needs an `import`/`require`
  naming the symbol, or a qualified `.name` call.
- **Python**: `def`/`class` names; a use needs `from ... import` or `import`, or a qualified
  `.name` attribute.
- **Go**: `func`/`type` names; a use in the same directory is accepted, and another directory
  needs a package-qualified `.Name`.
- **Other languages already read by the code graph** (Swift, Kotlin, Java, C/C++, C#, Ruby,
  PHP): declarations are reported, and only same-file references are trusted. No exact test
  target is invented for them.

Common declaration names (`run`, `new`, `test`, `parse`, ...) are listed as `ambiguous` and are
not scanned for references, because a bare common word is not evidence. Their packages still
come from the package graph. A changed symbol with no scanned reference is listed in a note;
this means "no reference found by this shallow scan", not "nothing calls it".

The walk starts at each changed declaration and moves backward through use sites. A use inside
another declaration (for example a wrapper body) makes that declaration reached; an import or a
use outside any declaration is a file-level leaf and stops there. Every reported reference
carries `depth` (`1` is a direct use) and `via`, the declaration chain from the changed item to
the use. A per-root visited set stops cycles and duplicate rows. For each changed item the walk
is capped at `MAX_REACH_DEPTH = 4`, 600 reached declarations and 4,000 reverse-use edges. The caps are
explicit: a caller beyond the depth cap is not guessed, and a cycle/revisit, depth stop, node
stop or edge stop is reported in `symbols.notes`.

Reference scanning is bounded at 40 changed symbols, 500 files, 512 KiB per file, 32 MiB total
and 200 references per symbol. Tests are scanned before other files, and files in affected
packages are visited round-robin so one large crate cannot consume the budget before its
dependents. A bound is reported in `symbols.notes`; selection remains package-safe. Because the
package targets are kept independently, a transitive chain can only add an exact test target or
another package's evidence; it cannot remove the package-level floor.

## What is reported

- `package`: every package the change reaches, with `role` `changed` or `dependent`, its root,
  the direct changed files, the integration-test targets it declares, `selected_by`, and the
  changed or referencing `symbol`/`path`/`line` rows that justify it. A package selected only by
  the dependency closure says so instead of inventing a reference.
- `symbols`: the changed items, their language and declaration kind, the references found with
  their depth and `via` chain, `max_depth`, `depth_limit`, `reached_symbols`, `cycles`, the
  packages those references name, `ambiguous` common names, unresolved files and scan notes.
- `owners`: CODEOWNERS owners for each changed path that has a rule.
- `selected`: the exact test commands `rigel test run` would execute. A changed package gets
  `cargo test -p <package>`; a changed integration test under `tests/` gets the exact `--test`
  target, while dependent packages still get their package runs. A test file that imports or
  qualifies a changed item adds its exact target (Rust `--test`, a JS test file, a Go test
  directory or a Python pytest file) **in addition to** the package target. Symbol selection
  therefore adds precision but never removes the package-level floor.
- `full`: the full suite when the plan is risky, with every reason.
- `skipped`: files that produce no target, and why.

## Owners

The first `CODEOWNERS` file found in `.github/`, the repository root or `docs/` is used. Rules
are matched in file order and the **last** match wins, as GitHub documents. `*` and `?` match
within one path segment, `**` spans segments, a leading `/` anchors to the repository root, a
trailing `/` covers the directory and its contents, and `!pattern` clears the owners for a
later match. Escaped spaces in patterns are not supported.

Owners are attribution, not a test selector. A path with no matching rule is simply absent from
the `owners` list rather than being assigned to a default.

## Selection safety

The dependency closure is capped at **8** packages. A larger closure, or a package newly reached
through symbol references, adds the full suite and names the cap. The existing rules still
apply: a manifest, lockfile, build script, migration, workflow or release script, more than 6
source files, more than 2 directly changed Cargo packages, an unmapped source, generated or
vendored source, or `--full` adds the full suite.

A Cargo package that appears only because of the closure is selected, not treated as a risk:
its tests genuinely compile against the changed code. The safety net still runs the full suite
when the selection cannot be trusted.

## Examples

```sh
# What does this working tree reach?
rigel impact --since HEAD

# A turn that already knows its files can skip the changed-file listing.
rigel impact --changed-file crates/rigel-core/src/testimpact.rs --json

# Show the changed function and the files that import or qualify it.
rigel impact --changed-file crates/rigel-core/src/testimpact.rs

# Show the graph and require the full suite for the review.
rigel impact --since origin/main --full
```

JSON is the same `ImpactPlan` that `rigel test plan --json` prints, with the added `packages`,
`owners`, `graph` and `symbols` fields. `packages[].references` is the per-package evidence.

## Limits

- Reachability follows use chains only up to the fixed bounds above. Rust macro expansion,
  re-exports through an alias, trait/dynamic dispatch and a call made only through a function
  pointer are not resolved; neither is a chain that crosses a file-level use. A caller beyond
  depth 4 or the node/edge caps is deliberately left to the package target. These are limits of
  the shallow evidence, not a claim that the test is irrelevant.
- The symbol reader is a line-oriented heuristic. A symbol name inside a string, a same-named
  local helper, an unusual declaration form or a file over the scan bounds can hide or add a
  reference. The package answer and its cap/full-suite rules remain the safe result.
- No coverage or historical-failure signals; tests outside the dependency closure are found by
  the full-suite fallback, not by this graph.
- The fallback graph understands inline and table-form `path` dependencies; a workspace
  dependency inherited with `foo.workspace = true` without a path is not an edge.
- CODEOWNERS glob support is intentionally small: `*`, `**`, `?`, root anchoring and directory
  patterns. Character classes and escaped spaces are not parsed.
- Cargo metadata is read without compiling. A project whose manifest cannot be parsed falls
  back to the manifest scan rather than failing the plan.

## Tests

`crates/rigel-core/src/testimpact.rs` covers the workspace closure, the reverse direction, the
metadata source and edge count, CODEOWNERS last-match and `!` rules, path-dependency parsing,
and a symbol reference adding an exact test without dropping package targets.
`crates/rigel-core/src/symbolimpact.rs` covers hunk mapping, enclosing-item selection, Git
failure fallback, comments, import/qualifier filtering, ambiguous common names, an
intermediate-only chain, a cycle, the depth cap and an unrelated chain.
`crates/rigel-cli/tests/impact.rs` drives the real binary against a temporary Cargo workspace
with a real `Cargo.lock`, CODEOWNERS and a test that imports a changed core function.
