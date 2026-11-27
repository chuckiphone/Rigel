# Performance regression suite

Run the deterministic benchmark fixtures and compare medians with the checked-in
`baseline.json` using:

```sh
cargo bench -p rigel-core --bench performance
```

The runner prints the current median, percentage trend and per-case regression threshold.
It exits nonzero when a median exceeds its recorded threshold. The suite covers the cached
tree hash, workspace and FTS history searches, diff generation and parsing, text patch
application, and the durable patch pipeline's propose and apply operations.

## Baseline policy

Record a baseline only after reviewing an intentional performance change, using a quiet,
otherwise idle machine with the same OS, CPU class and Rust toolchain as the previous baseline:

```sh
cargo bench -p rigel-core --bench performance -- --record-baseline
```

Commit the resulting `baseline.json` with a short explanation in the change that motivated
the update. Do not update a baseline merely to silence a regression. Each case currently allows
a 25% median regression to absorb ordinary timing noise; larger regressions fail the run.
Use `-- --samples=30` for a more stable local comparison. CI compiles the suite with
`cargo bench --no-run`; timing comparisons belong on a stable runner because shared CI hosts
do not provide repeatable CPU timing.

The runner's threshold self-check can be exercised without running the workload:

```sh
cargo bench -p rigel-core --bench performance -- --check-policy
```
