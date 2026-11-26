import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import RequestInspector, {
  PREVIEW_INSPECTION,
  fromUsage,
  inspectionJson,
  redactHeaders,
} from "../src/inspector/request";

test("credential headers lose their value but keep their name", () => {
  const rows = redactHeaders([
    { name: "Authorization", value: "Bearer sk-secret", redacted: false },
    { name: "x-api-key", value: "sk-secret", redacted: false },
    { name: "Set-Cookie", value: "session=sk-secret", redacted: false },
    { name: "content-type", value: "application/json", redacted: false },
  ]);
  assert.deepEqual(rows.slice(0, 3).map((row) => [row.name, row.value, row.redacted]), [
    ["Authorization", "", true],
    ["x-api-key", "", true],
    ["Set-Cookie", "", true],
  ]);
  assert.deepEqual(rows[3], { name: "content-type", value: "application/json", redacted: false });
  assert.doesNotMatch(inspectionJson({ ...PREVIEW_INSPECTION, request: { ...PREVIEW_INSPECTION.request, headers: rows } }), /sk-secret/);
});

test("a usage row builds the displayed struct with honest missing fields", () => {
  const inspection = fromUsage({
    input_tokens: 100,
    output_tokens: 40,
    cached_input_tokens: 20,
    duration_ms: 500,
    queue_ms: 25,
    route: { provider: "openrouter", model: "fast", resolved_model: "openai/gpt-5-nano", reason: "alias" },
    request_headers: [{ name: "Authorization", value: "sk-secret", redacted: false }],
  });
  assert.equal(inspection.tokens.total, 160);
  assert.deepEqual(inspection.timing, { duration_ms: 500, queue_ms: 25 });
  assert.equal(inspection.route.resolved_model, "openai/gpt-5-nano");
  assert.equal(inspection.route.resolved_provider, "not recorded");
  assert.deepEqual(inspection.request.headers[0], { name: "Authorization", value: "", redacted: true });
  assert.equal(inspection.tool_schema, null);
  assert.equal(inspectionJson(inspection), JSON.stringify(inspection, null, 2));
});

test("the panel shows route, tokens, timing, bodies and tool schema without a secret", () => {
  const inspection = {
    ...PREVIEW_INSPECTION,
    request: { ...PREVIEW_INSPECTION.request, headers: redactHeaders(PREVIEW_INSPECTION.request.headers) },
    response: { ...PREVIEW_INSPECTION.response, headers: redactHeaders(PREVIEW_INSPECTION.response.headers) },
  };
  const html = renderToStaticMarkup(<RequestInspector inspection={inspection} />);
  for (const text of [
    "Request inspector",
    "openrouter / fast",
    "openai/gpt-5-nano",
    "120 in · 45 out · 20 cached · 185 total",
    "820 ms model · 35 ms queue",
    "read_file",
    "redacted · no value stored",
    "Copy as JSON",
  ]) {
    assert.ok(html.includes(text), `${text} is on screen`);
  }
  assert.doesNotMatch(html, /sk-preview-secret/);
});
