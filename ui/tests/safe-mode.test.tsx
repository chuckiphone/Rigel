import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { SafeModeView } from "../src/api";
import SafeModePanel, { SafeModeBadge, canLeave } from "../src/safe-mode";

const safe: SafeModeView = { mode: "safe", leave_phrase: "leave safe mode" };

test("leaving needs the exact phrase and a safe session", () => {
  assert.equal(canLeave(safe, "leave safe mode"), true);
  assert.equal(canLeave(safe, "Leave safe mode"), false);
  assert.equal(canLeave(safe, ""), false);
  assert.equal(canLeave({ mode: "normal", leave_phrase: safe.leave_phrase }, "leave safe mode"), false);
});

test("a turn badge appears only for safe rows", () => {
  assert.equal(renderToStaticMarkup(<SafeModeBadge mode="normal" />), "");
  assert.equal(renderToStaticMarkup(<SafeModeBadge />), "");
  const html = renderToStaticMarkup(<SafeModeBadge mode="safe" />);
  assert.match(html, /class="turn-mode safe"/);
  assert.match(html, /aria-label="Turn ran in safe mode"/);
});

test("the panel's initial view exposes the explicit way in and no HTML injection", () => {
  const html = renderToStaticMarkup(<SafeModePanel project={null} session={null} />);
  assert.match(html, /aria-label="Safe mode"/);
  assert.match(html, /data-mode="normal"/);
  assert.match(html, /Open this session in safe mode/);
  assert.doesNotMatch(html, /<script/i);
});
