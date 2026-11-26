import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import EmptyState, { emptyAction, type EmptySurface } from "../src/empty-state";

const SURFACES: EmptySurface[] = [
  "sessions",
  "claims",
  "conflicts",
  "queue",
  "plugins",
  "memory",
  "chat",
  "ledger",
  "patches",
  "worktree",
  "concord",
];

test("every empty surface has one real action derived from its state", () => {
  for (const surface of SURFACES) {
    const action = emptyAction(surface);
    assert.ok(action, `${surface} has an action`);
    assert.ok(action.label.length > 0, `${surface} action is labelled`);
    assert.deepEqual(emptyAction(surface, { filtered: true }), {
      id: "clear-filter",
      label: "Clear filters",
    });
    assert.equal(emptyAction(surface, { loading: true }), null);
  }
});

test("an empty state renders a button, never an illustration", () => {
  for (const surface of SURFACES) {
    const html = renderToStaticMarkup(<EmptyState surface={surface} onAction={() => {}} />);
    assert.match(html, /<button[^>]*class="chip empty-action"/, `${surface} has a control`);
    assert.doesNotMatch(html, /<svg/, `${surface} has no illustration`);
    assert.match(html, new RegExp(`data-surface="${surface}"`));
    assert.match(html, new RegExp(emptyAction(surface)?.label ?? ""));
  }
  const loading = renderToStaticMarkup(
    <EmptyState surface="sessions" signals={{ loading: true }} onAction={() => {}} />,
  );
  assert.doesNotMatch(loading, /<button/, "a loading state offers no action");
});
