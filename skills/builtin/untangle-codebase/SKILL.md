---
name: untangle-codebase
description: Untangle a whole codebase in one pass so agents read less per change, break less without noticing, and debug faster.
license: MIT
metadata:
  author: Rigel
  tags:
    - refactor
    - architecture
    - codebase
    - design
---

# Untangle a codebase

## Purpose

A codebase is tangled when the code does not show what must change together. Agents then
search too much, miss hidden dependencies and break things without noticing. Untangling is a
deliberate structural refactor, not a style sweep: make each likely change land in one place,
make dependencies point at what changes less, and remove code that has no reason to exist.

This body is a Rigel-native adaptation of Maze's
[caldoai/skills untangle-codebase](https://github.com/caldoai/skills), MIT License.

## When to use it

Use this skill when the user asks for a whole-codebase or architecture-level refactor, or when
several unrelated fixes keep exposing the same structural problem. It is a goal that may span
many files and several verification cycles.

Do not use it as permission to rewrite everything. Skip it for a focused bug fix, a small
feature, or a codebase the user has not agreed to restructure. Never discard uncommitted work
or change public behavior silently.

## 1. Confirm the mandate

Besides untangling the codebase, two optional jobs may be in scope:

| Addition | What it does |
| --- | --- |
| **Comments** | Remove comments that only restate the implementation. |
| **Tests** | Remove tests that cannot catch a regression, and repair tests that fail for the wrong reason. |

Describe each option exactly, ask which ones the user wants, and do only those. A request to
untangle the code is not automatic permission to delete tests or comments.

Before editing, establish the baseline:

```bash
git status --short
```

Read the project's rules and run the smallest check that covers the area. Record any existing
failure so it is not mistaken for a regression later. For a large refactor, use a branch or
worktree when the project supports one.

## 2. List every problem

Do not change anything yet.

Go through the codebase asking one question: **should these pieces change together?** Write
down every place the code gets that answer wrong. There are three kinds of wrong answer, plus
special cases.

### Hidden links

Pieces that must change together, where the code does not show it.

- Copies of logic or values: change one and the others must change too, but nothing says so.
- A module that reads a global or file set by another module: change that module and this one
  breaks, but nothing shows the dependency.
- A feature split across folders by layer, or related code in different folders: one change
  needs all of them, but the layout hides that.
- A call order or convention callers must simply know: change it and callers break, but
  nothing warns them.

### Needless links

Pieces tied together that change for different reasons.

- Callers that depend on internals: the internals change for their own reasons, and the
  callers break anyway.
- Stable code that imports something more volatile: the stable code has to change whenever
  the volatile code does.
- A general function with a parameter only one workflow understands: the function has to
  change whenever that workflow does.
- A shallow module: its interface changes whenever its internals do, so its callers change
  with it.

### Surplus

Pieces with no reason of their own to change.

- Wrappers that only change when the thing they wrap changes.
- Extension points and configuration for things that never vary.
- Dead code: nothing uses it, so nothing will ever need it to change.

### Special cases

Also write down every special case: code that exists for one situation instead of following
from how the system works. Each one is a requirement the design never accounted for, and the
set of them shows what the system actually needs.

Examples: an `if` for one value, a parameter for a specific caller, a swallowed exception, a
retry or sleep, or a disabled test.

### Write the decision log

Keep the list beside the work, in the conversation or a scratch file. For each entry, name the
file and line, the problem, and later the single fix that removes it. Do not turn the list into
a pile of local patches.

## 3. Redesign from first principles

Work out the fixes as if the system had known today's requirements from the start. Look for
the one fix behind several problems; it often looks nothing like them. Do not fix special
cases one by one.

Apply the same test to every proposed fix, even one that reads better immediately:

- pieces that change together end up together;
- pieces that do not change together are not tied together;
- everything depends on what changes less.

Prefer moving responsibility or deleting a concept over adding a layer. A new abstraction must
earn its place by removing more special cases than it introduces. Write each fix above the
problems it solves in the decision log.

## 4. Fix everything on the list

Remove the special cases the redesign makes unnecessary, and keep the ones the system still
needs. Do not wander off looking for unrelated bugs. A structural fix may repair some along
the way, which is fine; record it.

If the user chose **Comments**, remove comments that merely restate the implementation, such
as `// sort first so the binary search works`. Keep comments about something outside the code,
such as `// the proxy drops websockets, so we poll`, and keep a workaround comment until the
redesign removes the problem behind it.

If the user chose **Tests**, do this last, because tests are the regression safety net. Never
delete or weaken a test merely to make the refactor pass.

- Remove a test only when it repeats the implementation with no independent expectation, or
  provably cannot fail for any behavior the project cares about.
- Rewrite tests tied to internals so they assert observable behavior instead.
- Repair a test that fails without a real regression; never silence it with a skip, a weaker
  assertion or an allow attribute.
- Keep a test that documents intended public behavior even if it would not catch every bug.

When in doubt, keep the test and say why.

## 5. Check, and repeat until done

Run the focused checks while working, then the project's full relevant suite after the final
edit. Read the diff for accidental behavior changes, debug output and formatting churn.
Behavior must match the baseline except for bugs the refactor intentionally fixed.

For a large codebase, use subagents or separate passes to search for remaining special cases,
hidden links, needless links and surplus. Do not stop at the first green test if the decision
log still has structural problems that are worth fixing.

You are done when:

- as few special cases remain as the requirements allow;
- a typical change lands in one place;
- every import points at something no more volatile than its importer;
- the structural review has no worthwhile issues left;
- the final verification passes and intended behavior is unchanged.

## Guardrails

- **Ask before a broad destructive refactor.** A skill does not replace the user's approval.
- **Preserve behavior.** If a behavior change is required, name it and get agreement.
- **Do not delete tests to make a refactor pass.** Repair them; only remove a provably
  redundant test when the user chose the Tests option.
- **Do not rewrite comments wholesale.** Keep external constraints, workarounds and reasons.
- **Stay in the requested scope.** Offer adjacent improvements; do not smuggle them in.
- **Leave evidence.** Report the commands run and the remaining special cases.

## Useful habits

- Start from a skeleton when the codebase is large: script away function bodies so the whole
  shape fits in context.
- Summarize parts of the codebase with subagents, then reason across the summaries.
- Check the dependency graph often; rerun it as the shape changes.
- Enter where leverage is highest.
- Keep a decision log; compaction is less of a problem when the reasoning is written down.
- Use Git history as evidence for how often pieces change and what changes together.

## Decision log example

```md
- `ui/LogoutDialog.tsx:30` waits 100ms after closing and hopes the message input can take
  focus by then.
- `ui/Toolbar.tsx:40` and `ui/Menu.tsx:22` call the same actions, each with its own
  disabled-state check.
- `checkout/submit.ts:9` depends directly on `payments/stripe.ts`.
- `shortcuts/keymap.ts:18` has a switch with one case per action, separate from the menu's
  action list.
- `ui/CommandPalette.tsx:55` keeps its own action list and is missing the newer ones.
- `ui/SettingsDialog.tsx:61` sets a global `skipRefocus` flag to stop the logout dialog from
  sending focus to the message input.
```

With the fixes:

```md
- Make the general `Dialog` restore focus to whatever had it before the dialog opened, so the
  wait and the flag both go.
- Define a `PaymentProvider` interface that Stripe and PayPal implement, so checkout and
  refunds no longer know which provider they are using.
- Make every user action a command with a name, handler and undo, so the toolbar, menu,
  shortcuts, palette and undo history all run from one list.
```

## Attribution

Adapted from [caldoai/skills](https://github.com/caldoai/skills), MIT License,
Copyright (c) 2026 Maze. The upstream skill and its full MIT text live in that repository.
