import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import DropZone, {
  MAX_ATTACHMENT_BYTES,
  validateAttachment,
  validateDrop,
} from "../src/composer/dropzone";

const file = (name: string, size = 12, type = "text/plain") => ({ name, size, type });

test("a normal text or image drop is accepted before any upload starts", () => {
  assert.deepEqual(validateAttachment(file("notes.txt")), { ok: true });
  assert.deepEqual(validateAttachment(file("shot.png", 4_096, "image/png")), { ok: true });
  assert.deepEqual(validateAttachment(file("notes.txt", 12, "")), { ok: true });
  assert.equal(MAX_ATTACHMENT_BYTES, 25 * 1024 * 1024);
});

test("refused type and size are named and never enter the accepted list", () => {
  const badType = validateAttachment(file("malware.exe", 12, "application/x-msdownload"));
  assert.equal(badType.ok, false);
  assert.match(badType.ok ? "" : badType.reason, /exe files cannot be attached/);

  const wrongMime = validateAttachment(file("notes.txt", 12, "application/x-msdownload"));
  assert.equal(wrongMime.ok, false);
  assert.match(wrongMime.ok ? "" : wrongMime.reason, /not a txt file/);

  const tooBig = validateAttachment(file("huge.txt", MAX_ATTACHMENT_BYTES + 1));
  assert.equal(tooBig.ok, false);
  assert.match(tooBig.ok ? "" : tooBig.reason, /25 MiB limit/);

  assert.deepEqual(validateAttachment(file("empty.txt", 0)), {
    ok: false,
    reason: "Empty files cannot be attached",
  });
  assert.equal(validateAttachment(file("../escape.txt")).ok, false);

  const mixed = validateDrop([file("notes.txt"), file("malware.exe"), file("huge.txt", MAX_ATTACHMENT_BYTES + 1)]);
  assert.deepEqual(mixed.accepted.map((item) => item.name), ["notes.txt"]);
  assert.deepEqual(mixed.refused.map((item) => item.name), ["malware.exe", "huge.txt"]);
});

test("the drop wrapper only appears when a session owns the upload", () => {
  const html = renderToStaticMarkup(
    <DropZone upload={async () => ""} onAttached={() => {}}>
      <span>child</span>
    </DropZone>,
  );
  assert.match(html, /class="composer-dropzone"/);
  assert.match(html, /<span>child<\/span>/);

  const quiet = renderToStaticMarkup(
    <DropZone upload={async () => ""} onAttached={() => {}} active={false}>
      <span>child</span>
    </DropZone>,
  );
  assert.doesNotMatch(quiet, /composer-dropzone/);
  assert.match(quiet, /<span>child<\/span>/);
});
