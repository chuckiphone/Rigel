import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import ForkLineage from "../src/fork-lineage";

test("fork lineage displays the parent session and selected parent turn", () => {
  const html = renderToStaticMarkup(
    <ForkLineage parentSessionId="parent-session-id" parentTurnId="selected-turn-id" />,
  );
  assert.match(html, /fork of parent-/);
  assert.match(html, /title="Forked at turn selected-turn-id"/);
});

test("ordinary sessions do not render fork lineage", () => {
  assert.equal(renderToStaticMarkup(<ForkLineage />), "");
});
