import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import DiffReview, { hunkSides, provenance } from "../src/diff-review";
import type { AttributedHunk } from "../src/api";

const hunk: AttributedHunk = {
  id: "hunk-1",
  patch_id: "patch-12345678",
  path: "src/lib.rs",
  ordinal: 1,
  agent_id: "writer",
  header: "@@ -10,2 +10,3 @@",
  old_start: 10,
  old_lines: 2,
  new_start: 10,
  new_lines: 3,
  created_at: 0,
  status: "applied",
};

test("hunk sides and provenance come from the recorded hunk", () => {
  const sides = hunkSides(hunk);
  assert.deepEqual(sides.oldLines, ["- src/lib.rs:10", "- src/lib.rs:11"]);
  assert.deepEqual(sides.newLines, ["+ src/lib.rs:10", "+ src/lib.rs:11", "+ src/lib.rs:12"]);
  assert.deepEqual(provenance(hunk), [
    "author writer",
    "patch patch-12 · hunk 1",
    "src/lib.rs · applied",
  ]);
});

test("the panel starts unified and exposes split, provenance and both accept actions", () => {
  const html = renderToStaticMarkup(
    <DiffReview hunks={[hunk]} turnId="turn-1" reviewer="reviewer@example.com" onVerdict={async () => ({ id: "i", turn_id: "turn-1", seq: 1, kind: "note", status: "verdict", body: {}, created_at: 0 })} />,
  );
  assert.match(html, /aria-label="Diff review"/);
  assert.match(html, /aria-label="Unified view for hunk 1"/);
  assert.equal((html.match(/aria-label="Split view/g) ?? []).length, 0);
  assert.match(html, /aria-label="Provenance for hunk 1"/);
  assert.match(html, /author writer/);
  assert.match(html, /Accept hunk 1 alone/);
  assert.match(html, /aria-label="Reviewer name"/);
  assert.match(html, /reviewer@example.com/);
});
