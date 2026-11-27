import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { TrayAttention } from "../src/tray";

test("the tray badge names the unresolved work and offers one pause-all control", () => {
  const html = renderToStaticMarkup(
    <TrayAttention badge={2} onPauseAll={async () => {}} />,
  );
  assert.match(html, /aria-label="Tray attention"/);
  assert.match(html, /aria-label="Unresolved items"/);
  assert.match(html, />2</);
  assert.match(html, /2 unresolved approval\(s\) or failure\(s\)/);
  assert.match(html, /Pause all/);
});

test("an empty badge says nothing is waiting and still allows pause-all", () => {
  const html = renderToStaticMarkup(
    <TrayAttention badge={0} onPauseAll={async () => {}} />,
  );
  assert.match(html, /Nothing waiting on you/);
  assert.match(html, /Pause all/);
});
