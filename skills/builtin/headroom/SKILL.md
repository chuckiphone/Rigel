---
name: headroom
description: Compress long tool output, logs and file reads before they reach the model, and turn repeated corrections into a reviewed AGENTS.md note.
license: Apache-2.0
metadata:
  author: Rigel
  tags:
    - headroom
---

# Headroom

## Purpose


Headroom reduces what an agent sends to the model without changing the answer. It is useful
on this project because long command output and repeated source reads dominate the bill.

Licence: **Apache-2.0**, verified 2026-11-22 at `headroomlabs-ai/headroom`. The earlier
"licence unverified" note is superseded.

## Preconditions

```bash
command -v headroom || pip install "headroom-ai[all]"
headroom --help
```

The installed version's help is the authority for subcommand names. Do not invent a flag;
read `--help` and copy the real one.

## Safe adoption order

1. Pick one long, read-only turn: a log triage, a multi-file read or a failing test run.
2. Capture the current input tokens and cost from the turn's usage row.
3. Run the same task with compression enabled.
4. Compare answer correctness first, tokens second. A cheaper wrong answer is a regression.

Report both numbers and the command that produced them. "Feels shorter" is not a
measurement.

## Learn mode

The project's learn mode mines failed sessions into corrections for `AGENTS.md`. Treat its
output as a **proposal**:

- Run it against a copy of the rules file, never the live one.
- Keep a correction only when it names a repeated failure you can point to.
- One sentence per rule, stated as an instruction, with the trigger named.
- Delete rules the code now enforces itself.
- Show the diff and ask before writing. Rigel's project rules are read by every turn.

## Boundaries

- Do not compress approval prompts, command text, safety findings or error codes.
- Do not compress a diff being reviewed for exactness.
- Do not install it into a shared harness config without saying so.
- If compression drops a line a later step needs, disable it for that path and say why.
