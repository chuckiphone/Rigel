import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  DEFAULT_PROJECT_DEFAULTS,
  loadProjectDefaults,
  loadSessionDefaults,
  normalizeProjectDefaults,
  saveProjectDefaults,
  saveSessionDefaults,
  type ProjectDefaults,
} from "../src/settings/project-defaults";
import { ProjectDefaultsEditor } from "../src/settings/project-defaults";

function withStorage(run: () => void) {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
    },
  });
  run();
  delete (globalThis as { localStorage?: unknown }).localStorage;
}

test("project defaults persist independently for each project", () => {
  withStorage(() => {
    const first: ProjectDefaults = {
      model: "openai/gpt-5.6",
      effort: "high",
      max_steps: 48,
      approval_scope: "session",
    };
    saveProjectDefaults("/repo/one", first);
    assert.deepEqual(loadProjectDefaults("/repo/one"), first);
    assert.deepEqual(loadProjectDefaults("/repo/two"), DEFAULT_PROJECT_DEFAULTS);
  });
});

test("a session retains its creation snapshot after project defaults change", () => {
  withStorage(() => {
    const initial = normalizeProjectDefaults({ model: "model-a", effort: "low", max_steps: 12 });
    saveProjectDefaults("/repo", initial);
    saveSessionDefaults("session-1", initial);
    saveProjectDefaults("/repo", { ...initial, model: "model-b", approval_scope: "always" });
    assert.equal(loadSessionDefaults("session-1")?.model, "model-a");
    assert.equal(loadSessionDefaults("session-1")?.max_steps, 12);
    assert.equal(loadProjectDefaults("/repo").model, "model-b");
  });
});

test("invalid values fall back safely and step caps stay bounded", () => {
  assert.deepEqual(
    normalizeProjectDefaults({ model: " ", effort: "invalid" as ProjectDefaults["effort"], max_steps: 900 }),
    { ...DEFAULT_PROJECT_DEFAULTS, max_steps: 50 },
  );
});

test("the pre-creation editor displays all project defaults before a session starts", () => {
  const html = renderToStaticMarkup(
    <ProjectDefaultsEditor
      project="/repo"
      value={{ model: "model-a", effort: "high", max_steps: 40, approval_scope: "project" }}
      onChange={() => {}}
    />,
  );
  assert.match(html, /Project session defaults/);
  assert.match(html, /Default model/);
  assert.match(html, /value="model-a"/);
  assert.match(html, /value="high" selected/);
  assert.match(html, /value="40"/);
  assert.match(html, /value="project" selected/);
});
