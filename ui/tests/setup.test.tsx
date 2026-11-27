import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import SetupWizard, {
  SETUP_STEPS,
  answeredSteps,
  nextStep,
  progressPercent,
} from "../src/onboarding/setup";
import type { SetupFinding } from "../src/api";

const finding = (step: SetupFinding["step"], skipped = false): SetupFinding => ({
  step,
  detail: skipped ? "skipped by the user" : `observed ${step}`,
  skipped,
});

test("every step has a name and progress counts only answered steps", () => {
  assert.deepEqual(SETUP_STEPS.map((step) => step.id), [
    "os",
    "shell",
    "git",
    "providers",
    "harnesses",
  ]);
  assert.equal(answeredSteps([]), 0);
  assert.equal(progressPercent([]), 0);
  assert.equal(nextStep([]), "os");
  assert.equal(answeredSteps([finding("os"), finding("os")]), 1, "a repeated probe cannot double count");
  assert.equal(progressPercent([finding("os")]), 20);
  assert.equal(nextStep([finding("os")]), "shell");
  assert.equal(nextStep(SETUP_STEPS.map((step) => finding(step.id))), null);
});

test("a skip is an answer and every card has one", () => {
  const all = SETUP_STEPS.map((step) => finding(step.id, true));
  assert.equal(answeredSteps(all), 5);
  assert.equal(progressPercent(all), 100);
  const html = renderToStaticMarkup(
    <SetupWizard probe={async (step) => finding(step)} />,
  );
  assert.match(html, /aria-label="First-run setup"/);
  assert.match(html, /aria-label="Setup progress"/);
  assert.equal((html.match(/>Skip<\/button>/g) ?? []).length, 5);
  for (const step of SETUP_STEPS) assert.match(html, new RegExp(`data-step="${step.id}"`));
  assert.equal((html.match(/value="0" max="5"/g) ?? []).length, 1, "no check has completed yet");
});
