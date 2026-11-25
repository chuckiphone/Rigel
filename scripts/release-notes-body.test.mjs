import assert from "node:assert/strict";
import { test } from "node:test";

import { publishableNotes } from "./release-notes-body.mjs";

const options = { repository: "chuckiphone/Rigel", tag: "v0.6.0" };

test("anchors relative doc links to the tagged repository", () => {
  const source =
    "See [sso](../sso.md), [audit](../audit.md#filters), and [readme](../../README.md).";
  const { body, rewritten } = publishableNotes(source, options);

  assert.equal(rewritten, 3);
  assert.equal(
    body,
    "See [sso](https://github.com/chuckiphone/Rigel/blob/v0.6.0/docs/sso.md), " +
      "[audit](https://github.com/chuckiphone/Rigel/blob/v0.6.0/docs/audit.md#filters), and " +
      "[readme](https://github.com/chuckiphone/Rigel/blob/v0.6.0/README.md).",
  );
});

test("keeps external links, anchors and non-markdown assets untouched", () => {
  const source =
    "[site](https://example.com/a.md) [jump](#install) [root](/docs/sso.md) " +
    "![shot](shot.png) [mail](mailto:a@b)";
  const { body, rewritten } = publishableNotes(source, options);

  assert.equal(rewritten, 0);
  assert.equal(body, source);
});

test("preserves a link title after the rewritten target", () => {
  const { body } = publishableNotes('[sso](../sso.md "Single sign-on")', options);

  assert.equal(
    body,
    '[sso](https://github.com/chuckiphone/Rigel/blob/v0.6.0/docs/sso.md "Single sign-on")',
  );
});

test("refuses to invent a repository or tag", () => {
  assert.throws(() => publishableNotes("[doc](../doc.md)", { tag: "v0.6.0" }));
  assert.throws(() =>
    publishableNotes("[doc](../doc.md)", { repository: "chuckiphone/Rigel" }),
  );
});
