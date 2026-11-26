import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { MarkdownText, highlightCode, inlineTokens, parseMarkdown } from "../src/markdown";

const SCRIPT = "# Cause\n\nUse `tree_hash()` now.\n\n```rust\nfn go() -> u64 { 42 } // fine\n```\n\nA model can say <script>alert(1)</script> and it stays text.\n";

test("the renderer source has no HTML injection surface", () => {
  const source = readFileSync(new URL("../src/markdown.ts", import.meta.url), "utf8");
  for (const forbidden of ["innerHTML", "dangerouslySetInnerHTML", "insertAdjacentHTML"]) {
    assert.ok(!source.toLowerCase().includes(forbidden.toLowerCase()), `${forbidden} is absent`);
  }
});

test("the parser keeps fences, headings, lists and literal text in order", () => {
  const blocks = parseMarkdown("# Title\n\n- one\n- two\n\nplain <script>x</script>\n\n```ts\nconst x = 1;\n```\n");
  assert.deepEqual(
    blocks.map((block) => block.kind),
    ["heading", "list", "text", "code"],
  );
  assert.equal(blocks[0].kind === "heading" && blocks[0].level, 1);
  assert.deepEqual(blocks[1].kind === "list" && blocks[1].items, ["one", "two"]);
  assert.equal(blocks[2].kind === "text" && blocks[2].text, "plain <script>x</script>");
  assert.equal(blocks[3].kind === "code" && blocks[3].language, "ts");
});

test("inline tokens keep code and strong text but refuse non-http links", () => {
  assert.deepEqual(inlineTokens("a `code` and **bold**"), [
    { kind: "text", text: "a " },
    { kind: "code", text: "code" },
    { kind: "text", text: " and " },
    { kind: "strong", text: "bold" },
  ]);
  assert.deepEqual(inlineTokens("[safe](https://example.test)"), [
    { kind: "link", text: "safe", url: "https://example.test" },
  ]);
  const unsafe = inlineTokens("[bad](javascript:alert(1))");
  assert.ok(unsafe.every((token) => token.kind === "text"));
  assert.equal(unsafe.map((token) => token.text).join(""), "[bad](javascript:alert(1))");
});

test("the highlighter emits kinds without emitting markup", () => {
  const code = "fn go() -> u64 { let n = 42; let s = \"hi\"; // note\n}";
  const tokens = highlightCode(code, "rust");
  const kinds = new Set(tokens.map((token) => token.kind));
  for (const kind of ["keyword", "number", "string", "comment", "punctuation"]) {
    assert.ok(kinds.has(kind as never), `${kind} is highlighted`);
  }
  assert.equal(tokens.map((token) => token.text).join(""), code);
});

test("a script tag fixture renders strictly as text with a copy button and raw toggle", () => {
  const html = renderToStaticMarkup(<MarkdownText source={SCRIPT} />);
  assert.doesNotMatch(html, /<script/i);
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
  assert.match(html, /class="markdown-code"/);
  assert.match(html, /Copy code/);
  assert.match(html, /Raw/);
  assert.match(html, /class="markdown-tok-keyword"/);
  assert.doesNotMatch(html, /class="markdown-raw-body"/);
});
