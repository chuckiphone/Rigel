import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import CostDashboard, {
  PREVIEW_SPEND,
  costCsv,
  costJson,
  dollars,
  reconcile,
} from "../src/cost-dashboard";

test("all four groupings reconcile with the stored total to the micro-dollar", () => {
  const check = reconcile(PREVIEW_SPEND);
  assert.equal(check.ok, true);
  assert.equal(check.total, 123_456);
  assert.equal(check.turns, 4);
  assert.deepEqual(
    check.groups.map((group) => [group.name, group.micros, group.turns, group.delta]),
    [
      ["day", 123_456, 4, 0],
      ["model", 123_456, 4, 0],
      ["agent", 123_456, 4, 0],
      ["session", 123_456, 4, 0],
    ],
  );
  const broken = {
    ...PREVIEW_SPEND,
    by_model: [{ ...PREVIEW_SPEND.by_model[0], micros: PREVIEW_SPEND.by_model[0].micros + 1 }, ...PREVIEW_SPEND.by_model.slice(1)],
  };
  const bad = reconcile(broken);
  assert.equal(bad.ok, false);
  assert.equal(bad.groups.find((group) => group.name === "model")?.delta, 1);
});

test("CSV and JSON carry the exact recorded report", () => {
  assert.equal(costJson(PREVIEW_SPEND), JSON.stringify(PREVIEW_SPEND, null, 2));
  const csv = costCsv(PREVIEW_SPEND);
  assert.match(csv, /^group,key,label,micros,turns\n/);
  for (const name of ["day", "model", "agent", "session"]) {
    assert.match(csv, new RegExp(`"${name}",`));
  }
  assert.match(csv, /"openrouter\/fast","openrouter\/fast","70000","2"/);
  assert.equal(dollars(123_456), "$0.12");
  assert.equal(dollars(1_000_000), "$1.00");
});

test("the panel shows total, groups, cap, ledger and reconciliation", () => {
  const html = renderToStaticMarkup(<CostDashboard report={PREVIEW_SPEND} capMicros={200_000} />);
  for (const text of [
    "recorded spend",
    "$0.12",
    "4 turns",
    "62% of $0.20 cap",
    "Totals reconcile",
    "by day",
    "by model",
    "by agent",
    "openrouter/fast",
    "Copy CSV",
    "Copy JSON",
    "Ledger",
  ]) {
    assert.ok(html.includes(text), `${text} is on screen`);
  }
  assert.doesNotMatch(html, /<svg/);
  assert.match(html, /aria-valuenow="62"/);
});
