import assert from "node:assert/strict";
import test from "node:test";

import {
  SETTING_ROWS,
  buildIndex,
  resolveLink,
  searchSettings,
  settingsHash,
  settingsLinkFromHash,
  type SettingsPageMeta,
} from "../src/settings/search";

const pages: SettingsPageMeta[] = [
  {
    id: "appearance",
    label: "Appearance",
    blurb: "Dark or light",
    group: "Window",
    keywords: "theme dark light scale",
  },
  {
    id: "audit",
    label: "Audit log",
    blurb: "Every approval",
    group: "Guardrails",
    keywords: "approval denials csv",
  },
];
const index = buildIndex(pages);

test("the index covers pages and named rows, and a query finds either", () => {
  assert.ok(index.length > pages.length);
  assert.ok(SETTING_ROWS.some((row) => row.key === "theme" && row.page === "appearance"));
  const byLabel = searchSettings(index, "THEME");
  assert.ok(byLabel.some((row) => row.key === "theme"));
  assert.ok(byLabel.some((row) => row.key === "page:appearance"));
  const byKeyword = searchSettings(index, "keyboard binding");
  assert.ok(byKeyword.some((row) => row.key === "search-shortcut"));
  assert.deepEqual(searchSettings(index, ""), index);
  assert.deepEqual(searchSettings(index, "no-such-setting"), []);
});

test("a deep link round-trips through the hash without eating page-owned query keys", () => {
  const hash = settingsHash("appearance", "theme");
  assert.equal(hash, "#settings/appearance?focus=theme");
  const link = settingsLinkFromHash(hash);
  assert.equal(link.page, "appearance");
  assert.equal(link.focus, "theme");
  const resolved = resolveLink(link, pages, index);
  assert.equal(resolved.focus, "theme");
  assert.equal(resolved.unknownKey, null);

  const audit = settingsLinkFromHash("#settings/audit?project=rigel&q=echo");
  assert.equal(audit.page, "audit");
  assert.equal(audit.focus, null, "audit owns its query string");
  assert.equal(settingsHash("audit"), "#settings/audit");
});

test("an unknown page or setting key becomes a named not-found state", () => {
  const unknownKey = resolveLink(
    settingsLinkFromHash("#settings/appearance?focus=made-up"),
    pages,
    index,
  );
  assert.equal(unknownKey.page, "appearance");
  assert.equal(unknownKey.focus, null);
  assert.equal(unknownKey.unknownKey, "made-up");

  const unknownPage = resolveLink(
    settingsLinkFromHash("#settings/not-a-page?focus=theme"),
    pages,
    index,
  );
  assert.equal(unknownPage.page, null);
  assert.equal(unknownPage.knownPage, false);
  assert.deepEqual(settingsLinkFromHash(""), { page: null, focus: null, unknownKey: null });
});
