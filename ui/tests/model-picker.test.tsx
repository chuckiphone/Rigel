import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { DoorHealth, ModelOption } from "../src/api";
import {
  ModelRow,
  groupByProvider,
  latencyHint,
  priceHint,
  resolvedPair,
} from "../src/model-picker";

const paid: ModelOption = {
  id: "anthropic/claude-sonnet-4.5",
  provider: "openrouter",
  free: false,
  chat: true,
  output_micros_per_mtok: 15_000_000,
};
const unknown: ModelOption = {
  id: "mystery/model",
  provider: "openrouter",
  free: false,
  chat: true,
};
const free: ModelOption = { id: "free/model", provider: "local", free: true, chat: true };

test("the resolved pair is provider / model, exactly as the router knows it", () => {
  assert.equal(resolvedPair(paid), "openrouter / anthropic/claude-sonnet-4.5");
  assert.deepEqual(groupByProvider([paid, free, unknown]).map((group) => [group.provider, group.rows.length]), [
    ["openrouter", 2],
    ["local", 1],
  ]);
});

test("price and latency say unknown when the build has no fact", () => {
  assert.equal(priceHint(free).label, "free");
  assert.equal(priceHint(paid).label, "$15.00/M out");
  const missing = priceHint(unknown);
  assert.equal(missing.label, "unknown");
  assert.match(missing.detail, /does not guess/);
  const health: DoorHealth[] = [
    { provider: "openrouter", name: "OpenRouter", state: "ready", latency_ms: 627 },
  ];
  assert.equal(latencyHint(paid, health).label, "627 ms");
  const missingLatency = latencyHint(paid, null);
  assert.equal(missingLatency.label, "unknown");
  assert.match(missingLatency.detail, /does not guess/);
});

test("a row shows the pair on hover and in the accessibility tree, never a guessed price", () => {
  const html = renderToStaticMarkup(
    <ModelRow
      option={unknown}
      pairId="pair-test"
      onChoose={() => {}}
      onFavourite={() => {}}
    />,
  );
  assert.match(html, /price · unknown/);
  assert.match(html, /latency · unknown/);
  assert.match(html, /aria-describedby="pair-test"/);
  assert.match(html, /class="model-pair" id="pair-test">openrouter \/ mystery\/model/);
  assert.match(html, /title="[^"]*openrouter \/ mystery\/model"/);
  assert.doesNotMatch(html, /\$[0-9]/, "no invented price");
});
