import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import ComposerBar, { COMPOSER_CONTROLS, EFFORT_OPTIONS, MIN_COMPOSER_WIDTH } from "../src/composer";

test("the control list names every part of the v2 row", () => {
  assert.deepEqual([...COMPOSER_CONTROLS], [
    "attachments",
    "skills",
    "model",
    "effort",
    "context",
    "mode",
  ]);
  assert.deepEqual([...EFFORT_OPTIONS], ["low", "medium", "high"]);
  assert.equal(MIN_COMPOSER_WIDTH, 320);
});

test("the bar renders six reachable controls with honest values", () => {
  const html = renderToStaticMarkup(
    <ComposerBar
      attachments={2}
      skills={1}
      model="openai/gpt-5-nano"
      provider="openrouter"
      effort="high"
      contextPercent={64}
      mode="plan"
    />,
  );
  for (const label of [
    "Attach",
    "Skills",
    "Model",
    "gpt-5-nano",
    "Effort",
    "Composer effort",
    "Context",
    "64%",
    "Act",
    "Plan",
  ]) {
    assert.ok(html.includes(label), `${label} is present`);
  }
  assert.match(html, /title="Model for the next turn: openrouter \/ openai\/gpt-5-nano"/);
  assert.match(html, /title="64% of the window used"/);
  assert.equal((html.match(/aria-labelledby=/g) ?? []).length, 4, "four label-derived controls");
  assert.match(html, /aria-pressed="false"[^>]*>\s*<span class="composer-v2-label">Act/);
  assert.match(html, /aria-pressed="true"[^>]*>\s*<span class="composer-v2-label">Plan/);
  assert.match(html, /<option value="high" selected/);
  assert.match(html, /class="composer-v2-value mono"[^>]*>64%<\/span>/);
});

test("an unknown context window shows a question mark, not a guessed percentage", () => {
  const html = renderToStaticMarkup(
    <ComposerBar
      attachments={0}
      skills={0}
      model="mystery/model"
      effort="medium"
      contextPercent={null}
      mode="act"
    />,
  );
  assert.match(html, /title="No published context window for this model"/);
  assert.match(html, /class="composer-v2-value mono"[^>]*>\?<\/span>/);
  assert.doesNotMatch(html, /class="composer-v2-value mono">\d+%<\/span>/);
});
