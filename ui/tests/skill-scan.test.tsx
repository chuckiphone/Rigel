import assert from "node:assert/strict";
import test from "node:test";

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { SkillInstallReport, SkillScan } from "../src/api";
import {
  installReceiptText,
  scanReceiptText,
  SkillScanReceipt,
} from "../src/skill-scan";

const caution: SkillScan = {
  score: 45,
  severity: "MEDIUM",
  recommendation: "CAUTION",
  findings: 3,
  scanner: "skillspector 2.13.7",
};

test("the CAUTION receipt renders through React DOM", () => {
  const html = renderToStaticMarkup(<SkillScanReceipt scan={caution} />);
  assert.match(html, /skill-scan-receipt/);
  assert.match(html, /SkillSpector CAUTION 45\/100/);
  assert.doesNotMatch(html, /blocked/);
});

test("the index receipt carries the blocked count beside a CAUTION verdict", () => {
  const html = renderToStaticMarkup(<SkillScanReceipt scan={caution} blocked={2} />);
  assert.match(html, /SkillSpector CAUTION 45\/100 · 2 blocked/);
  assert.equal(scanReceiptText(caution, 2), "SkillSpector CAUTION 45/100 · 2 blocked");
});

test("the install notice renders the CAUTION receipt for the bytes written", () => {
  const report: SkillInstallReport = {
    name: "PDF Tools",
    directory: "pdf-tools",
    paths: ["/tmp/agents/pdf-tools/SKILL.md", "/tmp/claude/pdf-tools/SKILL.md"],
    replaced: false,
    scan: caution,
  };
  const text = installReceiptText(report);
  assert.match(text, /PDF Tools installed in 2 skill stores/);
  assert.match(text, /SkillSpector CAUTION 45\/100/);
});
