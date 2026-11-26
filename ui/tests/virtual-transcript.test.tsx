import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { VirtualTranscriptPreview, centeredIndex, centeredScroll, virtualWindow } from "../src/transcript/virtual";

test("the window keeps the scrollbar honest and renders only a bounded slice", () => {
  const top = virtualWindow(10_000, 0, 400, 40);
  assert.deepEqual(
    { start: top.start, end: top.end, paddingTop: top.paddingTop },
    { start: 0, end: 19, paddingTop: 0 },
  );
  assert.equal(top.paddingBottom, (10_000 - 19) * 40);
  assert.equal(top.paddingTop + (top.end - top.start) * 40 + top.paddingBottom, 400_000);

  const middle = virtualWindow(10_000, 200_000, 400, 40);
  assert.equal(middle.start, 4_996);
  assert.equal(middle.end, 5_015);
  assert.equal(middle.paddingTop, 4_996 * 40);
  assert.equal(virtualWindow(0, 100, 400, 40).end, 0);
  assert.equal(virtualWindow(10, 100, 400, 0).paddingBottom, 0);
});

test("resize anchoring keeps the centred row centred", () => {
  const before = centeredIndex(centeredScroll(9_000, 40, 500), 40, 500);
  assert.equal(before, 9_000);
  const after = centeredIndex(centeredScroll(9_000, 40, 300), 40, 300);
  assert.equal(after, before, "a smaller viewport still centres turn 9000");
  assert.equal(centeredScroll(9_000, 40, 300), 359_870);
  assert.equal(centeredIndex(0, 40, 500), 6);
});

test("the preview fixture mounts only the visible rows", () => {
  const html = renderToStaticMarkup(<VirtualTranscriptPreview enabled />);
  const rows = html.match(/class="virtual-row"/g) ?? [];
  assert.ok(rows.length > 0);
  assert.ok(rows.length < 60, `${rows.length} rows are in the DOM, not 10,000`);
  assert.match(html, /10,000 turns/);
  assert.match(html, /Jump to turn 9000/);
  assert.equal(renderToStaticMarkup(<VirtualTranscriptPreview enabled={false} />), "");
});
