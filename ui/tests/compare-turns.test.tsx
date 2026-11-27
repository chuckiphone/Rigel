import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import CompareTurns, { compareFields, differingKeys } from "../src/compare-turns";
import type { Item, TurnTranscript, Usage } from "../src/api";

const usage = (cost_micros: number, served_model: string): Usage => ({
  input_tokens: 10,
  cached_input_tokens: 0,
  output_tokens: 5,
  reasoning_tokens: 0,
  cost_micros,
  provider_micros: null,
  served_model,
  source: "provider",
});

const item = (id: string, kind: string, body: Record<string, unknown>): Item => ({
  id,
  turn_id: "t",
  seq: 1,
  kind,
  status: "ok",
  body,
  created_at: 0,
});

const turn = (seq: number, prompt: string, cost: number, model: string): TurnTranscript => ({
  turn: {
    id: `t${seq}`,
    agent_id: "a",
    parent_turn_id: null,
    seq,
    status: "completed",
    prompt,
    started_at: 0,
    completed_at: 1,
    usage: usage(cost, model),
    stop_reason: "stop",
  },
  items: [
    item("m1", "message", { text: "first answer" }),
    item("c1", "command", { tool: "read_file", arguments: { path: "one.rs" } }),
  ],
});

test("a turn becomes prompt, route, model, tool-call, output and cost fields", () => {
  const fields = compareFields(turn(1, "do the work", 1_234_567, "openai/gpt-5"));
  assert.deepEqual(
    fields.map((field) => field.key),
    ["prompt", "route", "model", "tools", "output", "cost"],
  );
  assert.equal(fields.find((field) => field.key === "prompt")?.value, "do the work");
  assert.equal(fields.find((field) => field.key === "tools")?.value, 'read_file(path=one.rs)');
  assert.equal(fields.find((field) => field.key === "output")?.value, "first answer");
  assert.equal(fields.find((field) => field.key === "model")?.value, "openai/gpt-5");
  assert.match(fields.find((field) => field.key === "cost")?.value ?? "", /1\.234567/);
});

test("only genuinely different fields are named, in a stable order", () => {
  const left = compareFields(turn(1, "same prompt", 100, "model-a"));
  const right = compareFields(turn(2, "same prompt", 200, "model-b"));
  assert.deepEqual(differingKeys(left, right), ["model", "cost"]);
  assert.deepEqual(differingKeys(left, left), []);
});

test("the two sides render side by side with named rows and highlighted differences", () => {
  const left = turn(1, "same prompt", 100, "model-a");
  const right = turn(2, "same prompt", 200, "model-b");
  const html = renderToStaticMarkup(<CompareTurns left={left} right={right} />);
  assert.match(html, /aria-label="Compare turns"/);
  assert.match(html, /Turn 1 vs turn 2/);
  assert.equal((html.match(/class="compare-column"/g) ?? []).length, 2);
  assert.equal((html.match(/class="compare-row differing"/g) ?? []).length, 4, "two per side");
  assert.match(html, /data-field="cost"/);
  assert.match(html, /2 fields differ/);
});
