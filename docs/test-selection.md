# Test selection with a safety net

`rigel test` answers two questions before a turn runs a suite: which tests can this change
plausibly affect, and when is the fast answer not trustworthy enough to stand alone?

```
rigel test plan [--project DIR] [--since REV] [--changed-file PATH]... [--full] [--json]
rigel test run  [--project DIR] [--since REV] [--changed-file PATH]... [--full] [--json]
```

`plan` prints the changed files, selected targets, skipped changes and risk reasons without
running anything. `run` executes the selected targets in order and then the full suite when the
plan requires it.

## Changed files

By default the selector asks Git for tracked and untracked changes relative to `HEAD`. `--since`
compares against another revision, and `--changed-file` supplies a path explicitly (useful for a
turn that already knows what it touched, or for a project without Git). Every changed path is
reported, including documentation and generated files.

## What gets selected

- A **Cargo package source** change selects `cargo test -p <package>` for the owning package and
  for every package that reaches it through a Cargo path dependency. The graph comes from
  `cargo metadata --no-deps --locked --offline`; a project without a lockfile falls back to a
  manifest scan. This is package-level, not symbol-level: the package's unit and integration
  tests are the smallest reliable unit. `rigel impact` prints the same graph with package roles,
  declared integration tests and CODEOWNERS owners ([change impact](impact.md)).
- A change to a **Cargo integration test** under `tests/` selects the exact target, for example
  `cargo test -p rigel-core --test snapshot`.
- A **Cargo manifest** change selects the package's tests and requires the full suite.
- A **JavaScript/TypeScript package source** change selects that package's declared test script
  (`test`, then `test:e2e`, then `test:unit`). A changed test file passes that file to the script,
  for example `npm --prefix ui run test:e2e -- tests/session.spec.ts`.
- A package manifest with a test script requires the package's full test script.
- Documentation, data files and generated/vendored paths are reported as skipped with their
  reason.
- A source file in a language with no selector (Python, Go, Java and so on) is reported as
  unmapped and forces the full suite rather than silently running nothing.

The target list is deduplicated by its displayed command, so several files in one package produce
one package run.

## When the full suite follows

The plan adds the full suite when any of these is true:

- a manifest, lockfile, build script, migration, workflow or release script changed;
- more than **6** source files changed;
- more than **2** Cargo packages changed;
- the dependency closure is larger than **8** packages;
- a source file could not be mapped to a package;
- source changed in a generated or vendored tree;
- `--full` was passed.

The report names every reason. The full target is run only after every selected target passes; a
failed selected target stops the sequence and is never hidden by a later full run. `run` exits
non-zero on the first failing target and prints that target's bounded output in human mode. JSON
carries the plan, every selected run, every full run, `ok` and `stopped`.

A change containing only documentation produces no targets and exits 0 with an explicit "nothing
to run" report. A change that needs a full suite but has no detectable runner fails loudly instead
of pretending it was tested.

## Examples

```sh
# What would this working tree run?
rigel test plan --since HEAD

# Run impacted tests for a change on a branch, then the full suite if needed.
rigel test run --since origin/main

# A turn that already knows its files can skip Git entirely.
rigel test run --changed-file crates/rigel-core/src/backup.rs

# Force the safety net even for a small, mapped change.
rigel test run --changed-file crates/rigel-cli/src/main.rs --full
```

The JSON form is designed for a turn or CI step:

```json
{
  "plan": {
    "changed": [{ "path": "src/index.js", "status": "M" }],
    "targets": [{ "label": "npm run test", "reason": "package test script", "paths": [] }],
    "full": [],
    "skipped": [],
    "risk": []
  },
  "runs": [{ "command": "npm run test", "ok": true, "passed": 3, "failed": 0 }],
  "full_runs": [],
  "ok": true,
  "stopped": false
}
```

## Limits

- The Cargo side is a compiler-backed **package** graph, not a symbol or import graph. A change in
  one module of a large package still selects that package's whole suite, and a registry
  dependency is never treated as an internal edge.
- JavaScript and TypeScript selection remains a package-script map; there is no workspace
  dependency closure for those packages yet. A test inside another package that crosses the
  boundary is found by the full-suite fallback, not by the selected run.
- The selector does not infer test targets from coverage data or historical failures. A package
  whose test script is expensive is still run whole when one of its files changes.
- Test commands come from the project's manifest, as in the existing Run tests action. Rigel does
  not invent a runner or silently install one.
- The full suite follows only after the selected targets pass. A failure is reported as a failure,
  not as a partial success.

## Tests

`crates/rigel-core/src/testimpact.rs` covers Cargo source and integration-test selection, the
path-dependency closure in both directions, skipped documentation, manifest and unmapped-change
fallbacks, broad/forced plans, CODEOWNERS matching, Git changed-file discovery, and stopping
before a full run after a selected failure.
`crates/rigel-cli/tests/testimpact.rs` drives the real binary against a temporary Git project with
a `package.json` test script: plan output, manifest risk, explicit `--changed-file`, and an actual
`rigel test run`. `crates/rigel-cli/tests/impact.rs` drives the Cargo graph and owners.
