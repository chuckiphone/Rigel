import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import AsyncState, {
  ASYNC_KINDS,
  ASYNC_COPY,
  asyncAnnouncement,
} from "../src/async-state";

test("the announcement always names the state and keeps an optional detail", () => {
  assert.equal(asyncAnnouncement("loading"), "Working…");
  assert.equal(asyncAnnouncement("loading", "Reading the ledger…"), "Reading the ledger…");
  assert.equal(
    asyncAnnouncement("success", "Done.", "3 turns"),
    "Done. · 3 turns",
  );
  assert.deepEqual(Object.keys(ASYNC_COPY), [...ASYNC_KINDS]);
});

test("each state announces to a live region with the right urgency", () => {
  for (const kind of ASYNC_KINDS) {
    const html = renderToStaticMarkup(<AsyncState state={kind} label={`${kind} state`} />);
    const urgent = kind === "error" || kind === "retry";
    assert.match(html, new RegExp(`data-state="${kind}"`));
    assert.match(html, /class="async-live"/);
    assert.match(html, new RegExp(`aria-live="${urgent ? "assertive" : "polite"}"`));
    assert.match(html, new RegExp(`role="${urgent ? "alert" : "status"}"`));
    assert.match(html, new RegExp(`${kind} state`));
    assert.doesNotMatch(html, /<svg/, `${kind} has no illustration`);
    assert.equal(html.includes("class=\"async-spinner\""), kind === "loading");
  }
});

test("a retry button appears only for retryable states with a handler", () => {
  const withRetry = renderToStaticMarkup(
    <AsyncState state="error" label="failed" onRetry={() => {}}>detail</AsyncState>,
  );
  assert.match(withRetry, /Try again/);
  assert.match(withRetry, /async-detail/);
  const withoutRetry = renderToStaticMarkup(<AsyncState state="error" label="failed" />);
  assert.doesNotMatch(withoutRetry, /Try again/);
  const done = renderToStaticMarkup(
    <AsyncState state="success" label="done" onRetry={() => {}} />,
  );
  assert.doesNotMatch(done, /Try again/);
});
