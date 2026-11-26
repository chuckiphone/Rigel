import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import SessionFilters, {
  EMPTY_SESSION_FILTER,
  loadSavedFilters,
  saveFilter,
  sessionFilterFromHash,
  sessionFilterIsEmpty,
  sessionHash,
  type SessionFilter,
} from "../src/session-filters";

test("a session filter round-trips through its URL, including tag order", () => {
  const filter: SessionFilter = {
    query: "flaky resume",
    status: "active",
    archived: true,
    tags: ["review", "mystery-tag"],
  };
  const hash = sessionHash(filter);
  assert.match(hash, /^#sessions\?/);
  assert.deepEqual(sessionFilterFromHash(hash), filter);
  assert.deepEqual(sessionFilterFromHash("#sessions"), EMPTY_SESSION_FILTER);
  assert.deepEqual(sessionFilterFromHash(""), EMPTY_SESSION_FILTER);
  assert.ok(sessionFilterIsEmpty(EMPTY_SESSION_FILTER));
  assert.ok(!sessionFilterIsEmpty(filter));
});

test("an unknown tag is kept as text, not an error or a status", () => {
  const parsed = sessionFilterFromHash("#sessions?tag=made-up&status=nonsense");
  assert.deepEqual(parsed.tags, ["made-up"]);
  assert.equal(parsed.status, "all");
  const html = renderToStaticMarkup(<SessionFilters filter={parsed} onChange={() => {}} />);
  assert.match(html, />made-up</);
  assert.match(html, /class="filter-tag"/);
  assert.doesNotMatch(html, /class="filter-tag known"/);
  assert.doesNotMatch(html, /error/i);
});

test("saved filters survive a fresh read of the store", () => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, value),
      removeItem: (key: string) => void values.delete(key),
    },
  });
  assert.deepEqual(loadSavedFilters(), []);
  const filter: SessionFilter = { ...EMPTY_SESSION_FILTER, query: "review", tags: ["ended"] };
  const rows = saveFilter("review", filter);
  assert.equal(rows.length, 1);
  assert.deepEqual(loadSavedFilters()[0].filter, filter);
  assert.equal(loadSavedFilters()[0].name, "review");
});
