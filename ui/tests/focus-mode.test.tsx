import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import FocusChip, { FOCUS_HIDDEN, focusLabel, shellClass } from "../src/focus-mode";

test("focus drops chrome classes only, leaving the detail pane with the composer and approvals", () => {
  assert.deepEqual(FOCUS_HIDDEN, [
    "rail",
    "list-pane",
    "inspector",
    "utility-cluster",
    "pane-handle",
  ]);
  assert.ok(!FOCUS_HIDDEN.includes("detail"), "the detail pane survives focus");
  assert.equal(shellClass({ native: false, view: "session", focus: false }), "shell session");
  assert.equal(shellClass({ native: true, view: "session", focus: true }), "shell chrome session focus");
  assert.equal(shellClass({ native: false, view: "account", focus: false }), "shell settings");
  assert.equal(shellClass({ native: false, view: "diff", focus: false }), "shell diff");
  assert.equal(shellClass({ native: false, view: "session", focus: true }), "shell session focus");
});

test("the exit chip keeps its pressed state, label and advertised shortcut without an illustration", () => {
  const on = renderToStaticMarkup(
    <FocusChip focus shortcut="Ctrl Shift F" onToggle={() => {}} />,
  );
  assert.match(on, /class="chip focus-chip"/);
  assert.match(on, /aria-pressed="true"/);
  assert.match(on, /Exit focus · Ctrl Shift F/);
  assert.match(on, /Hide the rail, list and inspector \(Ctrl Shift F\)/);
  assert.doesNotMatch(on, /<svg/);

  const off = renderToStaticMarkup(
    <FocusChip focus={false} shortcut="Ctrl Shift F" onToggle={() => {}} />,
  );
  assert.match(off, /aria-pressed="false"/);
  assert.match(off, /Focus · Ctrl Shift F/);
  assert.equal(focusLabel(true), "Exit focus");
  assert.equal(focusLabel(false), "Focus");
});
