import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import McpResult, {
  RESULT_CHAR_CAP,
  capRows,
  imageSource,
  resultRows,
  type ResultRow,
} from "../src/mcp-result";

const rich = {
  tool: "read_file",
  arguments: { path: "a.rs" },
  result: {
    content: [
      { type: "text", text: "first line" },
      { type: "image", mimeType: "image/png", data: "data:image/png;base64,AAAA" },
      { type: "resource", resource: { text: "line 12" } },
    ],
    structuredContent: { path: "a.rs", bytes: 12 },
  },
};

test("content blocks and structured content become typed rows in order", () => {
  const rows = resultRows(rich);
  assert.deepEqual(
    rows.map((row) => row.kind),
    ["text", "image", "structured", "structured", "structured"],
  );
  assert.equal(rows[0].kind === "text" && rows[0].text, "first line");
  assert.equal(rows[1].kind === "image" && rows[1].mime, "image/png");
  assert.equal(rows[2].kind === "structured" && rows[2].value, "line 12");
  assert.deepEqual(
    rows.slice(3).map((row) => (row.kind === "structured" ? [row.label, row.value] : null)),
    [
      ["path", "a.rs"],
      ["bytes", "12"],
    ],
  );
  assert.deepEqual(resultRows({ tool: "plain", result: { path: "a.rs" } }).length, 1);
  assert.deepEqual(resultRows({ tool: "empty" }), []);
});

test("the size cap keeps whole rows and names what it hid", () => {
  const rows: ResultRow[] = [
    { kind: "text", text: "x".repeat(1_500) },
    { kind: "text", text: "y".repeat(900) },
    { kind: "image", url: "data:image/png;base64,AAAA", mime: "image/png" },
  ];
  const capped = capRows(rows, 2_000);
  assert.equal(capped.visible.length, 2, "the image row has no text and still fits");
  assert.equal(capped.hidden, 1);
  assert.equal(capped.truncated, true);
  assert.equal(capped.used, 1_500);
  assert.equal(capRows(rows, 3_000).visible.length, 3);
});

test("only data and http(s) images are safe to render", () => {
  assert.equal(imageSource({ kind: "image", url: "data:image/png;base64,AAAA", mime: "image/png" }) !== null, true);
  assert.equal(imageSource({ kind: "image", url: "https://example.test/a.png", mime: "image/png" }) !== null, true);
  assert.equal(imageSource({ kind: "image", url: "javascript:alert(1)", mime: "image/png" }), null);
  assert.equal(imageSource({ kind: "image", url: "file:///etc/passwd", mime: "image/png" }), null);
});

test("the panel shows typed rows, a visible cap marker and no raw body until asked", () => {
  const html = renderToStaticMarkup(<McpResult body={rich} />);
  assert.match(html, /MCP result · read_file/);
  assert.match(html, /class="mcp-row text"/);
  assert.match(html, /class="mcp-row image"/);
  assert.match(html, /class="mcp-row structured"/);
  assert.match(html, /Raw payload/);
  assert.doesNotMatch(html, /class="mcp-raw"/);

  const long = {
    tool: "search_repo",
    result: { content: [{ type: "text", text: "z".repeat(RESULT_CHAR_CAP + 1) }] },
  };
  const cappedHtml = renderToStaticMarkup(<McpResult body={long} />);
  assert.match(cappedHtml, /class="mcp-truncated"/);
  assert.match(cappedHtml, new RegExp(`Truncated at ${RESULT_CHAR_CAP} characters`));
  assert.match(cappedHtml, /1 more row hidden/);
});

test("an unsafe image in a transcript body is refused, never rendered as an img", () => {
  const html = renderToStaticMarkup(
    <McpResult
      body={{
        tool: "unsafe_image",
        result: {
          content: [
            { type: "image", mimeType: "image/png", data: "javascript:alert(1)" },
          ],
        },
      }}
    />,
  );
  assert.match(html, /image · source refused/);
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /MCP result · unsafe_image/);
});
