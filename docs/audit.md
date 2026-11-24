# Audit ledger and export

Every approval is written once to the session database and sealed into an append-only ledger at
`.rigel/audit/approvals.jsonl`. Each line commits to the previous line's SHA-256, so editing,
reordering or deleting a line breaks the chain at that point. `rigel audit verify` checks it
without needing the database, and `rigel audit provenance` follows one command item back to its
turn, model step, tool call, approval and snapshot.

## Export

```
rigel audit export [--project DIR] --format jsonl|csv --out FILE [--json]
rigel audit verify [--project DIR] [--file FILE] [--json]
```

An export is refused while the chain is broken: copying a tampered ledger would make it look
authoritative. An existing destination is also refused rather than overwritten.

- **JSONL** is the ledger itself, one approval per line. It is byte-for-byte verifiable with
  `rigel audit verify --file <export>` on a machine that does not have the project database.
- **CSV** is a SIEM-friendly projection with this header:

  ```text
  seq,approval_id,session_id,agent_id,turn_id,kind,decision,scope,requested_at,decided_at,prev_hash,hash,request
  ```

  `request` is compact JSON; fields containing a comma, quote or newline are quoted and inner
  quotes are doubled. Empty `turn_id`, `scope` or `decided_at` values are empty cells.

The JSON report carries the exact byte count, the SHA-256 of the exported file and the final
chain tip:

```json
{
  "path": "audit-export.jsonl",
  "format": "jsonl",
  "records": 2,
  "sha256": "3f...",
  "chain_tip": "49d2...",
  "valid": true
}
```

The hash chain is evidence of local tampering, not a signature by a remote authority. It proves
the exported lines have not changed since they were sealed; it does not prove who wrote them.

## Filtered view in the app

Settings → Audit log reads the same ledger. Project, session, agent, turn, decision and free-text
filters are kept in the address (`#settings/audit?session=…&decision=denied`), so a copied URL
opens the same rows. **Export filtered CSV** writes the rows currently matched, with a leading
`project` column and the same escaping as the full CSV above. Its report also carries the SHA-256
of the exact bytes and the intact chain tip the view came from.

A filtered CSV is deliberately not an offline-verifiable artifact: omitted rows cannot be checked
as a continuous chain. Use `rigel audit export --format jsonl` for the full ledger when a reviewer
needs to verify it. Both paths refuse a broken chain; the page still shows the readable rows and
names the first broken line so the damage is visible.

## Examples

```sh
# Hand a reviewer the ledger, then verify the copy offline.
rigel audit export --format jsonl --out /tmp/approvals.jsonl
rigel audit verify --file /tmp/approvals.jsonl

# Feed a SIEM with the same approvals.
rigel audit export --format csv --out /tmp/approvals.csv
```

## Tests

`crates/rigel-core/src/audit/chain.rs` covers sealing and verification, a mutated line, a JSONL
round-trip through `verify_file`, CSV escaping and row shape, refusal of a broken chain and
refusal to overwrite. `crates/rigel-cli/tests/audit.rs` drives the real binary through JSONL
export, CSV export, offline verification and a tampered export.

`crates/rigel-core/src/audit/log.rs` covers every filter (including a wrong project), the shared
matcher used by query and CSV export, the matching row count, and refusal to export a broken
chain. `ui/tests/audit-filter.test.ts` round-trips the six filter fields through the settings
hash.
