import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { PREVIEW_ABOUT, PREVIEW_CRASH } from "../src/api";
import { CrashReporter, crashPayload, redactCrashText } from "../src/crash-report";

test("the preview redacts shaped secrets, bearer values and key blocks", () => {
  const redacted = redactCrashText([
    "key sk-abcdefghijklmnopqrstuvwxyz123456 leaked",
    "github ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
    "aws AKIAIOSFODNN7EXAMPLE",
    "Authorization: Bearer opaque-session-token",
    "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----",
    "a normal panic message stays readable",
  ].join("\n"));
  assert.doesNotMatch(redacted, /sk-abcdefghijklmnopqrstuvwxyz/);
  assert.doesNotMatch(redacted, /ghp_abcdefghijklmnopqrstuvwxyz/);
  assert.doesNotMatch(redacted, /AKIAIOSFODNN7EXAMPLE/);
  assert.doesNotMatch(redacted, /opaque-session-token/);
  assert.doesNotMatch(redacted, /BEGIN PRIVATE KEY/);
  assert.equal((redacted.match(/\[redacted:secret\]/g) ?? []).length, 4);
  assert.match(redacted, /\[redacted:key block\]/);
  assert.match(redacted, /a normal panic message stays readable/);
});

test("the payload is the preview and carries the build facts, not a machine identity", () => {
  const now = Date.parse("2026-11-25T09:14:02.000Z");
  const payload = crashPayload(PREVIEW_CRASH, { ...PREVIEW_ABOUT, commit: "abc123" }, now);
  assert.match(payload, /^Rigel crash report\n/);
  assert.match(payload, /commit abc123\n/);
  assert.match(payload, /version preview\n/);
  assert.match(payload, /created 2026-11-25T09:14:02\.000Z/);
  assert.match(payload, /Bearer \[redacted:secret\]/);
  assert.doesNotMatch(payload, /sk-xxxxxxxx/);
  assert.match(payload, /panic in coordinator/);

  const html = renderToStaticMarkup(
    <CrashReporter report={PREVIEW_CRASH} info={{ ...PREVIEW_ABOUT, commit: "abc123" }} now={now} />,
  );
  const preview = /<pre[^>]*>([\s\S]*?)<\/pre>/.exec(html)?.[1];
  assert.ok(preview, "the reporter has a preview");
  assert.equal(preview, payload);
});
