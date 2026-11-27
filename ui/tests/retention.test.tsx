import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { RetentionPreview } from "../src/api";
import {
  RetentionTable,
  canApply,
  confirmationText,
  formatBytes,
} from "../src/settings/retention";

const preview: RetentionPreview = {
  project: "C:/repo",
  policy: { turns_days: 90, blobs_days: 90, logs_days: 90, snapshots_days: 90 },
  turns: { count: 3, bytes: 1_200 },
  blobs: { count: 2, bytes: 500 },
  logs: { count: 1, bytes: 100 },
  snapshots: { count: 1, bytes: 2_000 },
  total_count: 7,
  total_bytes: 3_800,
};

test("bytes render in the unit that keeps the number readable", () => {
  assert.equal(formatBytes(12), "12 B");
  assert.equal(formatBytes(1_024), "1.0 KB");
  assert.equal(formatBytes(2_048 * 1_024), "2.0 MB");
});

test("apply needs the exact displayed totals", () => {
  const phrase = confirmationText(preview);
  assert.equal(phrase, "7 rows / 3800 bytes");
  assert.equal(canApply(preview, phrase), true);
  assert.equal(canApply(preview, "7 rows / 3800 Bytes"), false);
  assert.equal(canApply(preview, ""), false);
  assert.equal(canApply(null, phrase), false);
  assert.equal(canApply({ ...preview, total_count: 0 }, confirmationText({ ...preview, total_count: 0 })), false);
});

test("the preview table shows counts, bytes and the total", () => {
  const html = renderToStaticMarkup(<RetentionTable preview={preview} />);
  assert.match(html, /data-category="turns"/);
  assert.match(html, /data-category="blobs"/);
  assert.match(html, /data-category="logs"/);
  assert.match(html, /data-category="snapshots"/);
  assert.match(html, /data-category="total"/);
  for (const value of ["1.2 KB", "500 B", "100 B", "2.0 KB", "3.7 KB"]) {
    assert.match(html, new RegExp(value.replace(".", "\\.")), `${value} is shown`);
  }
  assert.match(html, />7</);
  assert.doesNotMatch(html, /<script/i);
});
