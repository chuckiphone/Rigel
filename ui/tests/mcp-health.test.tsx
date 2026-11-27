import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { McpServerHealth } from "../src/api";
import { HealthRow, healthSummary } from "../src/mcp-health";

const ready: McpServerHealth = {
  name: "filesystem",
  state: "ready",
  latency_ms: 64,
  protocol_version: "2025-06-18",
  tool_count: 11,
  last_error: null,
};

const failed: McpServerHealth = {
  name: "broken-server",
  state: "failed",
  latency_ms: 37,
  protocol_version: null,
  tool_count: 0,
  last_error: "broken-server did not answer in time",
};

test("the summary counts all three states", () => {
  assert.deepEqual(healthSummary([ready, failed]), { ready: 1, failed: 1, unsupported: 0 });
  assert.deepEqual(
    healthSummary([{ ...failed, state: "unsupported" }]),
    { ready: 0, failed: 0, unsupported: 1 },
  );
});

test("a ready row shows latency, protocol and tool count without an error", () => {
  const html = renderToStaticMarkup(<HealthRow row={ready} />);
  assert.match(html, /data-state="ready"/);
  assert.match(html, /filesystem/);
  assert.match(html, /64 ms/);
  assert.match(html, /protocol 2025-06-18/);
  assert.match(html, /11 tools/);
  assert.doesNotMatch(html, /class="mcp-health-error"/);
});

test("a failed row keeps its error visible and does not hide the other row", () => {
  const html = renderToStaticMarkup(
    <span>
      <HealthRow row={failed} />
      <HealthRow row={ready} />
    </span>,
  );
  assert.match(html, /data-state="failed"/);
  assert.match(html, /broken-server did not answer in time/);
  assert.match(html, /data-state="ready"/);
  assert.match(html, /filesystem/);
  assert.doesNotMatch(html, /<script/i);
});
