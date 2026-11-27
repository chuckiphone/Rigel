import assert from "node:assert/strict";
import test from "node:test";

import type { Item, Turn, TurnTranscript } from "../src/api";
import {
  FILTERS,
  filterCounts,
  filterKey,
  filterShowsPrompt,
  hiddenCount,
  matchesFilter,
  matchesText,
  outlineGroups,
  readFilter,
  visibleItems,
  writeFilter,
  type FilterStorage,
  type TranscriptFilter,
} from "../src/transcript-filter";

const turn: Turn = {
  id: "t-1",
  agent_id: "a-1",
  parent_turn_id: null,
  seq: 1,
  status: "failed",
  prompt: "Fix the resume test",
  started_at: 1,
  completed_at: 2,
  usage: null,
  stop_reason: "model_error",
};

const row = (id: string, kind: string, body: Record<string, unknown> = {}, status = "ok"): Item => ({
  id,
  turn_id: turn.id,
  seq: 1,
  kind,
  status,
  body,
  created_at: 1,
});

const entry: TurnTranscript = {
  turn,
  items: [
    row("i-user", "message", { role: "user", text: "please fix it" }),
    row("i-model", "message", { role: "assistant", text: "The ordering fix is right" }),
    row("i-reason", "reasoning", { text: "Windows renames land late" }),
    row("i-cmd", "command", { command: "cargo test" }),
    row("i-mcp", "mcp_call", { tool: "read_file", arguments: { path: "resume.rs" } }),
    row("i-diff", "file_change", { result: { path: "crates/resume.rs" } }),
    row("i-approval", "approval", { kind: "command", decision: "waiting" }),
    row("i-cost", "usage", { cost_micros: 200 }),
    row("i-error", "error", { message: "provider returned 503" }, "error"),
  ],
};

function memoryStorage(): FilterStorage {
  const values = new Map<string, string>();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => void values.set(key, value),
  };
}

test("every filter names one kind of row and all keeps every row", () => {
  const expected: Record<Exclude<TranscriptFilter, "all">, string[]> = {
    yours: ["i-user"],
    model: ["i-model", "i-reason"],
    tools: ["i-cmd", "i-mcp"],
    diffs: ["i-diff"],
    errors: ["i-error"],
    approvals: ["i-approval"],
    cost: ["i-cost"],
  };
  for (const [filter, ids] of Object.entries(expected)) {
    assert.deepEqual(
      entry.items.filter((item) => matchesFilter(item, filter)).map((item) => item.id),
      ids,
      `${filter} selects its own rows`,
    );
  }
  assert.equal(entry.items.filter((item) => matchesFilter(item, "all")).length, entry.items.length);
  assert.equal(FILTERS.length, 8, "the bar exposes all eight views");
});

test("the prompt follows the role filter, not the tool or cost filters", () => {
  assert.equal(filterShowsPrompt("all"), true);
  assert.equal(filterShowsPrompt("yours"), true);
  for (const filter of ["model", "tools", "diffs", "errors", "approvals", "cost"] as const) {
    assert.equal(filterShowsPrompt(filter), false, `${filter} hides the prompt`);
  }
});

test("search narrows the same rows and the hidden count stays exact", () => {
  assert.equal(matchesText("RESUME Test", "resume"), true);
  assert.equal(matchesText("resume", "missing"), false);

  assert.equal(visibleItems(entry, "all", "resume").length, 2, "path and prompt-shaped text match");
  assert.equal(hiddenCount(entry, "all", ""), 0);
  assert.equal(hiddenCount(entry, "all", "resume"), entry.items.length - 2);
  assert.equal(hiddenCount(entry, "tools", ""), entry.items.length - 2 + 1, "prompt hidden too");
  assert.equal(hiddenCount(entry, "yours", "please"), entry.items.length);
});

test("the outline is built from visible rows only", () => {
  const all = outlineGroups([entry], "all");
  assert.deepEqual(
    all[0].marks.map((mark) => mark.kind),
    ["prompt", "model", "model", "diff", "approval", "error"],
  );

  const tools = outlineGroups([entry], "tools");
  assert.equal(tools.length, 0, "a tool-only view has no prompt, diff or error marks");

  const model = outlineGroups([entry], "model");
  assert.deepEqual(model[0].marks.map((mark) => mark.kind), ["model", "model"]);

  const searched = outlineGroups([entry], "all", "resume");
  assert.equal(searched.length, 1);
  assert.deepEqual(searched[0].marks.map((mark) => mark.kind), ["prompt", "diff"]);
});

test("counts cover every view regardless of the current one", () => {
  const counts = filterCounts([entry]);
  assert.equal(counts.all, entry.items.length + 1);
  assert.equal(counts.yours, 2);
  assert.equal(counts.model, 2);
  assert.equal(counts.tools, 2);
  assert.equal(counts.diffs, 1);
  assert.equal(counts.errors, 1);
  assert.equal(counts.approvals, 1);
  assert.equal(counts.cost, 1);
});

test("the filter round-trips per project and session and refuses unknown values", () => {
  const store = memoryStorage();
  assert.equal(readFilter("p", "s", store), "all");
  writeFilter("p", "s", "cost", store);
  assert.equal(store.getItem(filterKey("p", "s")), "cost");
  assert.equal(readFilter("p", "s", store), "cost");
  assert.equal(readFilter("p", "other", store), "all", "the choice is session-scoped");

  store.setItem(filterKey("p", "s"), "nonsense");
  assert.equal(readFilter("p", "s", store), "all");
  writeFilter("p", "s", "errors", {
    getItem() {
      throw new Error("storage off");
    },
    setItem() {
      throw new Error("storage off");
    },
  });
  assert.equal(store.getItem(filterKey("p", "s")), "nonsense", "a blocked store is not fatal");
});
