---
name: skillevaluator
description: Score and audit an agent skill before shipping it. Use when a SKILL.md is new or edited, or before pinning it into a session.
license: Apache-2.0
metadata:
  author: Rigel
  tags:
    - skillevaluator
---

# SkillEvaluator

## Purpose


Use this skill to check a skill's format, internal contradictions, security surface and
harness compatibility before anyone relies on it. The first tier is deterministic, offline
and needs no API key; start there.

## Preconditions

Check the CLI before claiming a result:

```bash
command -v skillevaluator || uv tool install --python 3.13 "skillevaluator @ git+https://github.com/NVIDIA/SkillEvaluator.git"
skillevaluator --help
```

If the install fails, report the exact error. Do not substitute a hand-written opinion for
a score.

## Run it on this repository

```bash
for dir in skills/builtin/*/; do
  skillevaluator quality-check "$dir"
done
```

`quality-check` reports four weighted categories and needs nothing else. For the full
deterministic pass:

```bash
skillevaluator validate --tiers 1 skills/builtin/<name>
skillevaluator security-scan skills/builtin/<name>
skillevaluator harness-check skills/builtin/<name>
```

`security-scan` is static. It does not prove a skill harmless, and the skill's own scripts
run later under Rigel's command approval, not under this scan.

## Reading the result

- Report the score and each finding with its file and line.
- A format finding usually means the frontmatter `name` and directory disagree, or the
  description is too vague to trigger.
- Two rules that contradict each other are a correctness finding even when both are
  individually valid. Quote both lines.
- A harness-check `drop` means that harness will not load the skill at all; fix it before
  pinning.
- A `degrade` means the skill loads without part of its instructions; say which part.

## Boundaries

- Tier 1 only unless the user provides a provider key. Never send a key to the tool on
  your own initiative.
- Tier 3 runs a live agent and a sandbox and can spend money. Ask first.
- Drawing from the report is allowed; inventing a tier or a score is not.
