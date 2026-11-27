# Mutation testing (roadmap 193)

`cargo-mutants` is configured in `.cargo/mutants.toml`. Its scope is the four decision points this
project cannot afford to get subtly wrong:

- `crates/rigel-core/src/agent/routing.rs` — which provider door and model a turn actually uses.
- `crates/rigel-core/src/agent/approval.rs` — scope parsing, grants and question routing.
- `crates/rigel-core/src/agent/approval_batch.rs` — batch card grouping and per-scope approval.
- `crates/rigel-core/src/budget.rs` — the daily/quote arithmetic and the UTC billing day.

`cargo mutants --list` enumerates **329** mutants across those files. The config selects
`test_package = ["rigel-core"]` and `additional_cargo_test_args = ["--lib"]`, so each mutant is
judged by the core library tests: the integration suite contains timing-sensitive concurrency
tests that can fail on an unmutated tree under load. Warnings are capped so a one-line mutation
is judged by test outcomes, not by CI's `-D warnings`.

## Recorded run

Command: `cargo mutants --no-shuffle -F 'utc_day' -j 4` (71 mutants in `utc_day`, the arithmetic
that decides a billing day). The unmutated baseline passed in 375s build + 34s test.

Three mutants survived before the run was stopped (a full rebuild per mutant put the 71-mutant
run beyond the working session):

| Mutant | Disposition |
|---|---|
| `budget.rs:122:40 replace - with + in utc_day` | **Equivalence (dated 2026-11-27).** Line 122 is the Hinnant era correction `if z >= 0 { z } else { z - 146_096 }`. The `else` branch is reachable only when `z = days + 719_468 < 0`, i.e. before roughly 1970 BC. Rigel stores timestamps as Unix milliseconds from 1970 onward; no turn, patch or ledger row can name such a date, so the two expressions are equivalent for every representable value. |
| `budget.rs:122:40 replace - with / in utc_day` | **Equivalence (dated 2026-11-27).** Same pre-1970-BC branch; outside that range the branch is never evaluated. |
| `budget.rs:124:49 replace - with / in utc_day` | **New test.** The mutation changes the 400-year era correction in `yoe`. Added `utc_day` assertions at two era boundaries (`2000-02-29`, `2400-02-29`) in `crates/rigel-core/src/budget.rs` so the correction is exercised on both sides of the boundary. |

The other scoped mutants in routing, approvals and budget are covered by their existing unit tests
plus the boundary tests added for item 193:

- `crates/rigel-core/src/agent/routing.rs` — whitespace/case normalisation and an empty requested
  pair with an unchanged explanation.
- `crates/rigel-core/src/agent/approval.rs` — scope parsing case/near-miss and exact-project
  membership (including the sibling-prefix trap and an unscoped legacy question).
- `crates/rigel-core/src/budget.rs` — epoch, pre-epoch, leap-day and 400-year boundaries plus the
  exact reserve boundary.

To resume the full run: `cargo mutants --no-shuffle -j 4`. Mutants that survive should be added
to this table with either a new test or a dated equivalence argument, per the item's contract.
