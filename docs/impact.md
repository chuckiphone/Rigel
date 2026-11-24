# Change-impact graph

`rigel impact` answers one review question before anything runs: **what does this change
actually reach?** It is the same plan `rigel test plan` builds, printed for a reviewer rather
than for a runner.

```
rigel impact [--project DIR] [--since REV] [--changed-file PATH]... [--full] [--json]
```

`--changed-file` supplies paths explicitly and skips Git; otherwise `--since` (default `HEAD`)
is read with `git diff --name-status` plus untracked files. `.rigel/` is never a project change.

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
dependent package is selected because its tests compile against the changed package. This is a
safe over-approximation, not a symbol-level import graph.

## What is reported

- `package`: every package the change reaches, with `role` `changed` or `dependent`, its root,
  the direct changed files and the integration-test targets it declares.
- `owners`: CODEOWNERS owners for each changed path that has a rule.
- `selected`: the exact test commands `rigel test run` would execute. A changed package gets
  `cargo test -p <package>`; a changed integration test under `tests/` gets the exact
  `--test` target, while dependent packages still get their package runs.
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

The dependency closure is capped at **8** packages. A larger closure adds the full suite and
names the cap; past that point the selected runs cost more than the suite they would replace.
The existing rules still apply: a manifest, lockfile, build script, migration, workflow or
release script, more than 6 source files, more than 2 directly changed Cargo packages, an
unmapped source, generated/vendored source, or `--full` adds the full suite.

A Cargo package that appears only because of the closure is selected, not treated as a risk:
its tests genuinely compile against the changed code. The safety net still runs the full suite
when the selection cannot be trusted.

## Examples

```sh
# What does this working tree reach?
rigel impact --since HEAD

# A turn that already knows its files can skip Git.
rigel impact --changed-file crates/rigel-core/src/testimpact.rs --json

# Show the graph and require the full suite for the review.
rigel impact --since origin/main --full
```

JSON is the same `ImpactPlan` that `rigel test plan --json` prints, with the added `packages`,
`owners` and `graph` fields.

## Limits

- Package-level ownership, not symbol-level imports. A change in one module of a large package
  still selects that package's whole suite.
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
metadata source and edge count, CODEOWNERS last-match and `!` rules, and path-dependency
parsing. `crates/rigel-cli/tests/impact.rs` drives the real binary against a temporary Cargo
workspace with a real `Cargo.lock` and CODEOWNERS.
