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
  manifest scan. The package run is the floor. When the changed item can be resolved, a test
  file that imports or qualifies it, or that reaches it through a bounded chain of declarations,
  adds its exact target, for example `cargo test -p rigel-cli --test impact`; the package target
  stays as well, so a symbol answer can add precision but never fewer targets. `rigel impact`
  prints the graph, changed items, references and CODEOWNERS owners
  ([change impact](impact.md)).
- A change to a **Cargo integration test** under `tests/` selects the exact target, for example
  `cargo test -p rigel-core --test snapshot`.
- A **Cargo manifest** change selects the package's tests and requires the full suite.
- A **JavaScript/TypeScript package source** change selects that package's declared test script
  (`test`, then `test:e2e`, then `test:unit`). A changed test file passes that file to the script,
  for example `npm --prefix ui run test:e2e -- tests/session.spec.ts`. A test in any package
  that imports a changed symbol also gets its exact file target, in addition to the owning
  package's script.
- A package manifest with a test script requires the package's full test script.
- Documentation, data files and generated/vendored paths are reported as skipped with their
  reason.
- A **Go or Python** test file that references a changed symbol adds `go test ./<dir>` or
  `python -m pytest <file>`; the changed source is still reported as unmapped, so the full suite
  follows. Java and the other languages with no selector remain unmapped and force the full
  suite rather than silently running nothing.

The target list is deduplicated by its displayed command, so several files in one package produce
one package run.

## Symbol reachability

The selector reads the real Git hunk when it can and maps the changed lines to top-level
declarations. It then looks for imported or module-qualified uses, not every occurrence of a
word. Rust uses a `use` path or a qualified `module::name`; TypeScript/JavaScript and Python use
an import or a qualified `.name`; Go uses a same-directory reference or a package-qualified
`.Name`. From each changed item it walks backward through declarations that use the item, so a
test that calls a wrapper can still get its exact target. A use outside any declaration is a
file-level leaf and does not propagate. Every reference reports `depth` and `via`, and a
per-root visited set prevents cycles and duplicate targets. Common names such as `run` and `new`
are treated as ambiguous and fall back to the package answer. A module-level edit or an
unreadable/oversized file is also a package fallback.

For each changed item the walk is bounded at **4** declaration hops, **600** reached declarations
and **4,000** reverse-use edges. A caller beyond the depth cap is not guessed; a cycle, depth
stop, node stop or edge stop is reported in the symbol notes. These bounds are part of the
receipt rather than silent truncation.

The package targets are **always kept**. Symbol reachability can add an exact Rust integration
test, a JavaScript test file, a Go test directory or a Python test file, and it can mark a
dependent package as `selected_by` symbol references, but it never selects fewer targets than
the package-level answer. This is the safety property that makes a shallow parser acceptable.

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

- The Cargo package graph is compiler-backed; symbol references are a line-oriented heuristic
  layered on top. Bounded transitive reach follows declaration-to-declaration uses, but stops at
  depth 4 and at file-level uses; Rust macros, aliases, function pointers, trait/dynamic dispatch
  and unusual declarations can fall back to the package target. A chain beyond the bounds is not
  guessed.
- JavaScript and TypeScript selection remains a package-script map; there is no workspace
  dependency closure for those packages yet. A symbol reference can add a cross-package test
  file, but a package with no matching test script is not invented.
- The selector does not infer test targets from coverage data or historical failures. A package
  whose test script is expensive is still run whole when one of its files changes.
- Test commands come from the project's manifest, as in the existing Run tests action. Rigel does
  not invent a runner or silently install one.
- The full suite follows only after the selected targets pass. A failure is reported as a failure,
  not as a partial success.

## Tests

`crates/rigel-core/src/testimpact.rs` covers Cargo source and integration-test selection, the
path-dependency closure in both directions, skipped documentation, manifest and unmapped-change
fallbacks, broad/forced plans, CODEOWNERS matching, Git changed-file discovery, direct and
transitive symbol-reference exact targets without dropping package targets, and stopping before
a full run after a selected failure. `crates/rigel-core/src/symbolimpact.rs` covers hunk mapping,
import/qualifier filtering, ambiguous names, an intermediate-only chain, cycle termination,
depth-cap reporting and an unrelated chain.
`crates/rigel-cli/tests/testimpact.rs` drives the real binary against a temporary Git project with
a `package.json` test script: plan output, manifest risk, explicit `--changed-file`, and an actual
`rigel test run`. `crates/rigel-cli/tests/impact.rs` drives the Cargo graph and owners.
