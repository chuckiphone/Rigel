# Flaky-test hunter

`rigel test flakes` runs one test command several times and reports a test whose outcome changes
between identical runs. It never edits, skips or quarantines a test; it produces evidence and a
named suspect for a human or a follow-up change.

```
rigel test flakes [--project DIR] [--runs N] [--json] [-- COMMAND...]
```

Without a command after `--`, the project's detected suite is used (`testrun::detect`: Cargo,
Go, the `package.json` test script, or pytest). `--runs` accepts 2–10 and defaults to 3.

## What it reports

- `runs`: each run's exit code, pass/fail counts, timeout and duration.
- `tests`: per-test pass and fail counts and the number of runs in which the name appeared.
- `flaky`: names that both passed and failed. The human output proposes quarantine for each.
- `unstable_set`: names that did not appear in every run — a filtered, conditional or
  skip-on-failure test.
- `aggregate_flaky`: the run outcomes disagreed but no per-test result line attributed the
  disagreement, so the report says so instead of guessing a name.
- `all_ok`: every run passed. A consistently failing suite is stable but not green.

Exit `0` means no inconsistency was found. Exit `1` means a flake, an unstable set, an
unattributed disagreement, or a command/range error. The JSON form is the whole report.

## Parsing

The hunter reads per-test lines from the common runners:

- Rust: `test module::name ... ok` / `... FAILED`
- Go: `--- PASS: TestName (0.01s)` / `--- FAIL: TestName`
- Jest and Vitest: `✓ name (5 ms)` / `✗ name`
- Playwright: `✓  1 [chromium] › file.spec.ts:12:3 › test name (3s)`

Durations are stripped because they change on every run. A name repeated inside one run is
counted once, and a failure wins over a pass in that run. Summary lines such as
`test result: FAILED. 2 passed; 1 failed` do not invent a test.

## What it does not do

- **Order dependence is not tested.** Cargo and most runners use a stable order, and a shuffle
  mode is runner-specific. A test that only fails after another test runs needs the runner's own
  shuffle flag or a dedicated harness; this command says that in its notes rather than implying
  it covered order.
- **No history or quarantine file.** Results are not persisted across invocations, no owner is
  assigned, and no test is skipped. `batches` and repeated runs are the evidence; the proposed
  quarantine is a line in the receipt.
- **Heuristic names.** A runner whose output has no per-test line yields an aggregate result.
  A test name that contains a duration-like suffix could be trimmed.
- Each run has the runner's normal 15-minute timeout; ten runs of a slow suite take ten times as
  long, so start with 3.

## Examples

```sh
# Run the detected suite three times.
rigel test flakes

# Stress one Cargo test target eight times.
rigel test flakes --runs 8 -- cargo test -p rigel-core --test snapshot

# Machine-readable evidence for CI.
rigel test flakes --runs 5 --json -- node test.js
```

## Tests

`crates/rigel-core/src/flakes.rs` covers Rust/Go/Jest/Playwright parsing, duplicate lines in one
run, a real flake, an aggregate disagreement, a stable suite, an unstable set and the repeat
bounds. `crates/rigel-cli/tests/flakes.rs` drives the real binary with a Node script that
alternates pass and failure, a stable script, the detected suite and an out-of-range repeat.
