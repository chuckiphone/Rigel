import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { SkillPin, TemplateSkill } from "../src/api";
import { TemplatePinBadge, changedTemplatePins } from "../src/template-pins";

const pinned = (skill_id: string, body_sha: string): TemplateSkill => ({ skill_id, body_sha });
const pin = (skill_id: string, body_sha: string): SkillPin => ({ skill_id, body_sha });

test("only moved or missing template hashes are reported as changed", () => {
  assert.deepEqual(changedTemplatePins([], []), []);
  assert.deepEqual(
    changedTemplatePins([pinned("pdf", "abc")], [pin("pdf", "abc")]),
    [],
  );
  assert.deepEqual(
    changedTemplatePins([pinned("pdf", "abc"), pinned("json", "xyz")], [pin("pdf", "moved"), pin("json", "xyz")]),
    ["pdf"],
  );
  assert.deepEqual(changedTemplatePins([pinned("pdf", "abc")], []), ["pdf"]);
});

test("the badge shows the count and names a moved hash", () => {
  assert.equal(renderToStaticMarkup(<TemplatePinBadge skills={0} changed={[]} />), "");

  const clean = renderToStaticMarkup(<TemplatePinBadge skills={2} changed={[]} />);
  assert.match(clean, /2 pinned/);
  assert.doesNotMatch(clean, /changed/);
  assert.match(clean, /class="template-pins"/);

  const moved = renderToStaticMarkup(<TemplatePinBadge skills={2} changed={["pdf"]} />);
  assert.match(moved, /2 pinned · 1 changed/);
  assert.match(moved, /class="template-pins changed"/);
  assert.match(moved, /Pinned hash no longer matches: pdf/);
  assert.doesNotMatch(moved, /<script/i);
});
