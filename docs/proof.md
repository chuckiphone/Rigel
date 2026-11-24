# Proof-carrying changes

A proof bundle is local evidence that a specific working-tree state was actually tested. It
turns "the agent said it ran the tests" into a file a reviewer can check:

```
rigel proof record [--project DIR] [--since REV] [--note TEXT]... [--json] -- COMMAND...
rigel proof list   [--project DIR] [--json]
rigel proof show   [--project DIR] --id ID [--json]
rigel proof verify [--project DIR] [--id ID | --all] [--json]
rigel test run     [--project DIR] [--since REV] [--proof] [--json]
```

`record` runs one command and stores its evidence. `test run --proof` records every selected
and full-suite run automatically, including a docs-only change with no tests.

## What a bundle contains

```json
{
  "schema": "rigel.proof/1",
  "id": "bc0c4d5683fcd665",
  "created_at_ms": 1795535856125,
  "base": "aba60e563b59da36bdc61c37d3b5236f4fbcfba7",
  "tree": "bc0c4d56...",
  "files": [{ "path": "src/index.js", "status": "M", "sha256": "...", "bytes": 21 }],
  "evidence": [{
    "command": "npm run test",
    "exit_code": 0, "ok": true, "passed": 3, "failed": 0, "timed_out": false,
    "duration_ms": 412, "output": "3 passed\n", "output_sha256": "...",
    "output_truncated": false, "recorded_at_ms": 1795535856000
  }],
  "ok": true,
  "notes": ["recorded by rigel test run --proof"]
}
```

- **Base** is the commit the change is measured against, resolved when the bundle is written;
  a later commit does not silently move the comparison.
- **Files** are every tracked or untracked change against that base, with a SHA-256 per file.
  `.rigel/` is excluded; Rigel's own state is never evidence.
- **Tree** is a sorted digest over path and content hash. Git's status is shown but not hashed,
  so committing a recorded change keeps its proof fresh.
- **Evidence** is the exact command, exit code, parsed counts, duration and the first 32 KiB of
  output with a hash of the stored text.

Recording the same tree again appends evidence instead of replacing it. A bundle with any
failed command stays `ok: false`; that is a record of the failure, not a pass.

## What `verify` checks

1. The schema is `rigel.proof/1`.
2. The stored file hashes still reproduce the tree digest.
3. Every evidence output still matches its hash, and each evidence outcome agrees with its exit
   code and timeout flag.
4. A bundle that says `ok: true` for a source change has at least one command.
5. The working tree still fingerprints to the same tree against the recorded base.

The result has three independent facts: `valid` (the bundle is internally consistent), `fresh`
(the working tree still matches) and `ok` (the recorded runs passed). Merge should use all
three. `rigel proof verify` exits `0` only when every verified bundle is valid, fresh and
successful; `2` otherwise.

A bundle goes **stale** when a file changes after the run. `verify` names the changed paths
rather than guessing. Bundles are local files under `.rigel/proofs/`; copy one into a review or
ticket when the reviewer cannot open the project. The digest chain is evidence against
accidental drift, **not a signature**: anyone who can rewrite the bundle can rewrite its
hashes.

## Examples

```sh
# Record a focused test run as evidence for the current change.
rigel proof record --since HEAD -- cargo test -p rigel-core --lib testimpact::tests

# Run impacted tests and record all of them in one step.
rigel test run --since HEAD --proof

# Check the newest proof after a reviewer pulls the branch.
rigel proof verify --json

# Check every proof a CI checkout carries.
rigel proof verify --all
```

## Limits

- Proofs need a Git repository with at least one commit; the fingerprint is a diff against a
  base commit.
- Only the first 32 KiB of each command's output is stored. The full output is not replayed;
  the stored text is hashed.
- A docs-only change is allowed to have no evidence. A source change is not.
- There is no automatic merge gate yet. A CI job or a review check must call
  `rigel proof verify`; the command and its exit codes are the seam.
- File modes, ownership and file system metadata are not hashed.

## Tests

`crates/rigel-core/src/proof.rs` covers freshness and staleness, appending evidence, tampered
output, docs-only versus source changes, failed commands, prefix lookup, newest-first listing
and a commit after recording. `crates/rigel-cli/tests/proof.rs` drives the real binary with a
real Node command, `rigel test run --proof` and a stale-tree verification.
