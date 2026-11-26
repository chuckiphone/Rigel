import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { PREVIEW_ABOUT, type AboutInfo } from "../src/api";
import { AboutPanel, aboutText } from "../src/about";

const info: AboutInfo = { ...PREVIEW_ABOUT, version: "9.9.9", commit: "abc123def456" };

test("the diagnostics block is one deterministic string of local facts", () => {
  assert.equal(
    aboutText(info),
    [
      "Rigel 9.9.9",
      "commit abc123def456",
      `licences ${info.components} SBOM components · ${info.licences.length} expressions`,
      `channel ${info.update_channel}`,
      `privacy ${info.privacy}`,
    ].join("\n"),
  );
  assert.match(aboutText(info), /^Rigel /);
  assert.match(aboutText(info), /No telemetry/);
});

test("the About panel renders the exact bytes the Copy button writes", () => {
  const html = renderToStaticMarkup(<AboutPanel info={info} />);
  const block = /<pre[^>]*>([\s\S]*?)<\/pre>/.exec(html)?.[1];
  assert.ok(block, "the panel has a diagnostics block");
  assert.equal(block, aboutText(info));
  assert.match(html, /MIT OR Apache-2\.0/);
  assert.match(html, /abc123def456/);
});
