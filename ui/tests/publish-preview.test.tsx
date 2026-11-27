import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import PublishPreviewPanel, {
  DEFAULT_PUBLISH_SCOPE,
  previewRows,
  redactableKinds,
} from "../src/publish-preview";
import type { PublishPreview, TurnTranscript } from "../src/api";

const turns: TurnTranscript[] = [
  {
    turn: {
      id: "t1",
      agent_id: "a",
      parent_turn_id: null,
      seq: 1,
      status: "completed",
      prompt: "why is the build slow?",
      started_at: 0,
      completed_at: 1,
      usage: { input_tokens: 10, cached_input_tokens: 0, output_tokens: 5, reasoning_tokens: 0, cost_micros: 123, provider_micros: null, source: "provider" },
      stop_reason: null,
    },
    items: [
      { id: "m", turn_id: "t1", seq: 1, kind: "message", status: "ok", body: { text: "it compiles twice" }, created_at: 0 },
      { id: "c", turn_id: "t1", seq: 2, kind: "command", status: "ok", body: { text: "secret command output" }, created_at: 0 },
      { id: "f", turn_id: "t1", seq: 3, kind: "file_change", status: "ok", body: { path: "src/private.rs" }, created_at: 0 },
    ],
  },
];

test("tool output and diffs are off by default and every row is still listed", () => {
  const preview = previewRows("s1", turns, DEFAULT_PUBLISH_SCOPE);
  const kinds = preview.rows.map((row) => row.kind);
  for (const kind of ["session", "scope", "prompt", "reply", "tool_output", "diff", "usage"]) {
    assert.ok(kinds.includes(kind), `${kind} is listed`);
  }
  const tool = preview.rows.find((row) => row.kind === "tool_output")!;
  assert.equal(tool.value, "[tool output withheld]");
  assert.equal(tool.redacted, true);
  assert.equal(preview.rows.find((row) => row.kind === "diff")!.value, "[diff withheld]");
  assert.equal(preview.rows.find((row) => row.kind === "prompt")!.value, "why is the build slow?");
  assert.deepEqual(redactableKinds(preview), ["scope", "tool_output", "diff"]);
});

test("opting in reveals only the chosen kind", () => {
  const preview = previewRows("s1", turns, { tool_output: true, diffs: false });
  assert.equal(preview.rows.find((row) => row.kind === "tool_output")!.value, "secret command output");
  assert.equal(preview.rows.find((row) => row.kind === "diff")!.value, "[diff withheld]");
});

test("the panel renders the switches unchecked by default", () => {
  const preview: PublishPreview = previewRows("s1", turns, DEFAULT_PUBLISH_SCOPE);
  const html = renderToStaticMarkup(
    <PublishPreviewPanel preview={preview} onScope={async () => preview} />,
  );
  assert.match(html, /aria-label="Publication preview"/);
  assert.match(html, /aria-label="Include tool output"/);
  assert.match(html, /aria-label="Include diffs"/);
  assert.equal((html.match(/checked=""/g) ?? []).length, 0);
  assert.match(html, /\[tool output withheld\] · redacted/);
  assert.match(html, /off by default/);
});
