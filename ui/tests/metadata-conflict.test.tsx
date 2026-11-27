import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import MetadataConflict, {
  canPublish,
  conflicts,
  decisionTime,
  resolvedValue,
  unresolvedFields,
} from "../src/metadata-conflict";
import type { MetadataField, MetadataView } from "../src/api";

const fields: MetadataField[] = [
  { name: "publish_scope", base: "turns", ours: "all", theirs: "sessions" },
  { name: "shelf_scope", base: "skills", ours: "skills", theirs: "skills" },
];

const view = (withDecision = false): MetadataView => ({
  fields,
  decisions: withDecision
    ? [
        {
          field: "publish_scope",
          choice: "theirs",
          author: "reviewer@example.com",
          decided_at: 1_760_961_600_000,
        },
      ]
    : [],
});

test("only disagreeing fields are conflicts and unresolved fields gate publishing", () => {
  assert.deepEqual(
    conflicts(fields).map((field) => field.name),
    ["publish_scope"],
  );
  assert.deepEqual(unresolvedFields(view()), ["publish_scope"]);
  assert.equal(canPublish(view()), false);
  assert.deepEqual(unresolvedFields(view(true)), []);
  assert.equal(canPublish(view(true)), true);
});

test("a recorded choice names the exact side and is rendered with author and time", () => {
  assert.equal(resolvedValue(fields[0], "base"), "turns");
  assert.equal(resolvedValue(fields[0], "ours"), "all");
  assert.equal(resolvedValue(fields[0], "theirs"), "sessions");
  assert.equal(decisionTime(0), "1970-01-01T00:00:00.000Z");

  const html = renderToStaticMarkup(
    <MetadataConflict view={view(true)} author="reviewer@example.com" onDecide={async () => {}} />,
  );
  assert.match(html, /aria-label="Metadata conflicts"/);
  for (const side of ["Base", "Ours", "Theirs"]) assert.match(html, new RegExp(`<dt>${side}</dt>`));
  assert.match(html, /theirs chosen by reviewer@example.com at/);
  assert.match(html, /Every conflict has a recorded decision/);
  assert.doesNotMatch(html, /Publishing is blocked/);
});

test("an unresolved field shows all three sides and a blocked publish message", () => {
  const html = renderToStaticMarkup(
    <MetadataConflict view={view()} author="reviewer@example.com" onDecide={async () => {}} />,
  );
  assert.match(html, /data-field="publish_scope"/);
  assert.match(html, /aria-label="Base for publish_scope"/);
  assert.match(html, /aria-label="Ours for publish_scope"/);
  assert.match(html, /aria-label="Theirs for publish_scope"/);
  assert.match(html, /Publishing is blocked until publish_scope is resolved/);
});
