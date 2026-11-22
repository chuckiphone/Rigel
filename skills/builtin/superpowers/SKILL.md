---
name: superpowers
description: A disciplined build workflow for every coding agent: clarify intent, plan in small tasks, test first, debug by root cause and verify before claiming done.
license: MIT
metadata:
  author: Rigel
  tags:
    - superpowers
    - workflow
    - tdd
    - debugging
    - planning
---

# Superpowers

## Purpose

Superpowers is a portable software-development method, not a Claude Code feature. It gives
every agent the same loop:

1. find out what the human actually wants;
2. write a plan small enough to execute without guessing;
3. prove each change with a failing test first;
4. investigate a cause before changing code;
5. review the work against the plan;
6. verify with fresh evidence before saying it is finished.

This body is a Rigel-native adaptation of the MIT-licensed
[obra/superpowers](https://github.com/obra/superpowers) skills library. It does not depend on
Claude-only commands, hooks or plugin APIs. Any agent that can read a `SKILL.md` can follow it.

## When to use it

Load this skill before implementation work that is more than a one-line correction, and any
time a task is ambiguous, risky, or likely to need several files. Also load it when a turn has
gone wrong: repeated fixes, unexplained test failures, or a claim of completion that has no
fresh command output behind it.

Small, obvious edits may skip the written plan. They do not skip the evidence rule.

## The four iron laws

- **No production code without a failing test first.** If a test cannot be written, write down
  why and get agreement before changing behaviour.
- **No fix without root-cause investigation first.** A change that makes a symptom disappear
  without explaining it is not a fix.
- **No completion claim without fresh verification evidence.** Run the check after the final
  edit; an earlier pass does not cover a later change.
- **No speculative scope.** Build what was asked for. Offer improvements; do not smuggle them
  into the change.

## 1. Brainstorm the intent

Before code, establish what the human is trying to achieve and what must not change.

Ask only questions whose answers change the design. Prefer a few sharp questions over a long
interrogation. Useful angles:

- Who uses the result, and what does success look like?
- What is the smallest version that would be useful?
- What existing behaviour, file or interface must stay untouched?
- What is explicitly out of scope?
- Which constraints are real: stack, dependency policy, performance, platform, review process?

Play the design back in your own words. If the answer is still ambiguous, say which assumption
you will make and proceed when the risk is low; ask when a wrong assumption would be expensive
to undo.

For a large change, write a short design note beside the code or in the task description. For a
small one, the agreed summary in the conversation is enough.

## 2. Write the plan

Break the work into tasks that each take a few minutes, not hours. Every task should name:

- the files or modules it touches;
- the behaviour it adds or removes;
- the test that will fail before the change and pass after it;
- the exact command that proves it;
- what "done" looks like in one sentence.

Order the tasks so the project is runnable after each one. Prefer vertical slices over a wide
refactor. Put risky investigation first. If a task needs a decision from the human, mark it and
stop there rather than guessing.

A plan that names no verification is a wish list. Rewrite it.

## 3. Prepare an isolated workspace when it helps

A branch or worktree is useful when the change is large, when the current tree has unrelated
work, or when you may need to abandon the attempt. Check the project's own conventions first.
Never discard or overwrite uncommitted work that belongs to the human.

Before starting, establish a clean baseline:

```bash
git status --short
# run the smallest existing check that covers the area
```

If the baseline is already failing, record that fact before your change so you do not claim the
failure as yours or miss a regression hidden behind it.

## 4. Execute one task at a time

For each task:

1. Re-read the task and the files it names.
2. Write or update the smallest test that expresses the required behaviour.
3. Run it and confirm it fails for the expected reason. A test that fails because of a typo in
   the test is not the red step.
4. Make the smallest production change that turns it green.
5. Run the focused test again, then the nearest broader check.
6. Read the diff. Remove debug output, dead code and accidental formatting churn.
7. Mark the task done only when its verification command passes.

If the implementation reveals that the plan is wrong, stop and revise the plan. Do not force the
code to match a plan the evidence has disproved.

### Choosing between inline and subagent execution

- **Inline execution** is cheapest and best for a short plan, a single context, or work that
  needs constant human judgement. Do every task, then review the whole branch once.
- **Subagent-driven execution** is better for a long independent plan. Give each task to a fresh
  worker with the task text, the relevant files and the verification command. Review each
  result before starting the next task: first for spec compliance, then for quality.
- **Parallel workers** only make sense for tasks that touch disjoint files and do not depend on
  each other's result. Give every worker its own scope and require a summary plus command
  output. Never let two workers edit the same file concurrently.

Whichever mode is used, the orchestrator owns the final verification and the final report.

## 5. Test-driven development

The cycle is **RED → GREEN → REFACTOR**.

- **RED:** add the smallest failing test. Run it. Confirm the failure message describes the
  missing behaviour, not a broken test.
- **GREEN:** write only enough production code to pass. Run the test.
- **REFACTOR:** improve names and structure while the test stays green. Run it again.

Rules that keep the cycle honest:

- One behaviour per test where practical.
- Test the public behaviour, not private implementation details.
- Prefer a real dependency or an in-memory fake over an over-specified mock.
- A bug fix gets a regression test that fails on the old code.
- Delete code written before its test, rather than retrofitting a test to it.
- Do not weaken an assertion to make a test pass. Fix the cause.
- If a test is flaky, fix or quarantine it explicitly; never call a flaky pass proof.

## 6. Systematic debugging

Do not guess. Work through four phases.

### Phase 1: reproduce and describe

- Reproduce with the smallest command or input that shows the problem.
- Write down the expected result, the actual result, and the exact error.
- Identify the first bad state, not the last place the error surfaced.
- Check whether the failure is deterministic and whether it predates the current change.

### Phase 2: gather evidence

- Read the actual code path and the relevant configuration.
- Add temporary logging or a debugger at the boundary where the value is still correct and where
  it becomes wrong.
- Bisect the input, the commit range or the layers until the transition is narrow.
- Compare a working case with the broken one; the difference is usually the clue.

### Phase 3: form and test one hypothesis

State the cause in one sentence: "X happens because Y." Predict what would change if that were
true. Run the smallest experiment that can disprove it. Change one variable at a time.

If three fixes in a row fail, stop patching. The model of the system is wrong: re-read the
design, question an assumption, or ask for context. Write down what you have ruled out.

### Phase 4: fix, prove, prevent

- Fix the cause, not the symptom.
- Add the regression test that would have caught it.
- Run the focused test and the broader check.
- Remove temporary instrumentation.
- Note the remaining risk if the root cause cannot be fully removed.

## 7. Review before calling it done

Review the whole diff against the request and the plan, not against your memory of what you
meant to write.

Check:

- Every requested behaviour exists; nothing unrequested slipped in.
- Error paths, empty states and permission boundaries are handled.
- The change follows the project's conventions and does not add a dependency casually.
- Tests assert behaviour and would fail if the behaviour regressed.
- No secret, debug token, generated junk or unrelated file is included.
- The diff is readable, with names that explain intent.

For a substantial change, a fresh reviewer is better than self-review. Give the reviewer the
plan, the diff and the verification commands. Act on critical findings before continuing. When
receiving review, fix the cause; do not argue with a valid finding or silently ignore it.

## 8. Finish the branch

When all tasks are done:

1. Run the final verification commands from a clean state.
2. Report exactly what was run and what passed. Name anything that could not run.
3. Summarise the change in a few sentences: what changed, why, and any material limitation.
4. Leave the work uncommitted unless the human asked for a commit. If asked, follow the
   project's commit style and never commit unrelated files.
5. Present the next decision (keep, merge, open a PR, discard) rather than assuming it.
6. Clean up temporary worktrees and branches only when their contents are safely preserved.

## Working across agents

This skill is deliberately harness-neutral. Apply these translations instead of depending on
one vendor's commands:

| Need | Native Rigel | Claude Code | Codex / Cursor / Cline / Gemini |
|---|---|---|---|
| Read a skill | `skill_load` or the Shelf | install into `~/.claude/skills` | install into `~/.agents/skills` (Codex, Cursor, Cline, Gemini CLI, Muse Code) |
| Plan | session plan / task list | task list or plan file | `AGENTS.md` or a plan file |
| Subagent | dispatch a child agent | Task / subagent tool | available worker or a fresh session |
| Verify | run the command in the terminal | same | same |
| Project rules | `AGENTS.md`, `CLAUDE.md`, `.cursor/rules` | `CLAUDE.md` | `AGENTS.md`, project rules |

`rigel skills builtin --install` writes this body into both `~/.agents/skills` and
`~/.claude/skills`, which covers Rigel's native loop, Claude Code, Codex, Cursor, Cline, Gemini
CLI and Muse Code. Installation never runs on launch; it is an explicit action.

## Writing a new skill

When a workflow proves reusable, capture it as a skill rather than a long prompt:

- Give it a trigger-first description that says when to use it.
- Keep the body procedural and testable; name the commands and the evidence.
- State preconditions, the hard rules, and the boundary the agent must not cross.
- Include a small example or a table where it prevents ambiguity.
- Test it on a real task, including a case where it should not trigger.
- Keep the license and provenance visible.

## Attribution

The workflow is adapted from Jesse Vincent's Superpowers project
(<https://github.com/obra/superpowers>), MIT License. The upstream project ships additional
harness-specific hooks and installation paths; this Rigel version intentionally depends only on
a readable `SKILL.md` so every agent can use it.
