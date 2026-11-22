---
name: cognee-evaluate
description: Evaluate Cognee as an external graph-memory layer against Rigel's reviewed MEMORY.md, without migrating project memory into it.
license: Apache-2.0
metadata:
  author: Rigel
  tags:
    - cognee-evaluate
---

# Cognee, evaluate only

## Purpose


Cognee builds a knowledge graph from documents, code and conversations and answers
retrieval queries over it. It is listed to be **measured against** Rigel's memory, not to
replace it.

## The rule

Rigel's memory is a reviewed file (`MEMORY.md` plus topics) with a validation and approval
path. Moving it into an external graph would remove the review, the diff and the rollback.
Do not do that as part of an evaluation.

## A fair evaluation

1. Copy a bounded corpus to a scratch directory: one project's docs, a sample of sessions,
   and the current `MEMORY.md`. Never point Cognee at the live project database.
2. Build the graph in that scratch directory and record the build time and model used.
3. Write ten questions the current memory answers. For each, compare:
   - did the answer appear at all,
   - was it correct,
   - how many tokens of context did it take,
   - could a reviewer see why the fact was stored.
4. Report the comparison as a table. A retrieval win that cannot be reviewed is not a win
   for this project.

## What would justify adoption

Only all of these together:

- a measured retrieval improvement on the ten questions,
- a way to export, diff and roll back every fact,
- a local-only path with no key required for the basic case,
- an approval surface equivalent to Rigel's memory write card.

Until then, keep it out of the turn path.

## Boundaries

- Do not write to the live `MEMORY.md` from an experiment.
- Do not send private sessions to a hosted service.
- Do not present a demo notebook's result as a measurement on this repository.
