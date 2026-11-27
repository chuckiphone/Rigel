import assert from "node:assert/strict";
import test from "node:test";

import {
  PALETTE_SECTIONS,
  paletteEntries,
  sectionRanges,
  SETTINGS_TARGETS,
} from "../src/palette";
import type { PaletteResults } from "../src/api";
import { helpText, parseSlash, unknownCommandError } from "../src/slash";

const results: PaletteResults = {
  skills: [],
  sessions: [
    { id: "s1", title: "Fix the flaky resume test", status: "active", created_at: 0 },
  ],
  turns: [],
  patches: [],
  files: [{ path: "src/lib.rs", language: "rust" }],
  symbols: [{ name: "helper", path: "src/lib.rs", line: 12, language: "rust" }],
};

test("the palette reaches all five roadmap sections", () => {
  assert.deepEqual(PALETTE_SECTIONS, ["actions", "files", "symbols", "sessions", "settings"]);
  assert.ok(SETTINGS_TARGETS.some((target) => target.id === "notifications"));

  assert.ok(
    paletteEntries(results, "", "Ctrl K").some((entry) => entry.section === "actions"),
    "actions are present with no query",
  );
  const entries = paletteEntries(results, "helper", "Ctrl K");
  const sections = new Set(entries.map((entry) => entry.section));
  assert.ok(sections.has("files"), "files come from the code graph");
  assert.ok(sections.has("symbols"), "symbols come from the code graph");
  assert.ok(
    paletteEntries(results, "", "Ctrl K").some((entry) => entry.section === "settings"),
    "settings are reachable",
  );
  assert.ok(entries.some((entry) => entry.title === "helper" && entry.meta === "rust"));
  assert.deepEqual(
    sectionRanges(entries).map((section) => section.label),
    ["Files", "Symbols", "Sessions"],
    "sections stay contiguous and in order",
  );
});

test("sessions and settings are reachable without a code-graph query", () => {
  const entries = paletteEntries(results, "", "Ctrl K");
  assert.ok(entries.some((entry) => entry.section === "sessions"));
  assert.ok(entries.some((entry) => entry.section === "settings"));
  assert.equal(entries.some((entry) => entry.section === "files"), false, "no query, no file rows");
});

test("a known slash command parses; an unknown one gets a named error", () => {
  assert.deepEqual(parseSlash("plain prompt"), { kind: "prompt", text: "plain prompt" });
  const known = parseSlash("/skill pdf");
  assert.equal(known.kind, "command");
  if (known.kind === "command") {
    assert.equal(known.command.name, "skill");
    assert.equal(known.command.args, "pdf");
  }
  const unknown = parseSlash("/frobnicate now");
  assert.equal(unknown.kind, "unknown");
  if (unknown.kind === "unknown") {
    assert.equal(unknown.name, "frobnicate");
    assert.equal(unknown.error, unknownCommandError("frobnicate"));
    assert.match(unknown.error, /Unknown command \/frobnicate/);
  }
  assert.match(helpText(), /\/help — List the commands/);
  assert.deepEqual(parseSlash("use /help here"), { kind: "prompt", text: "use /help here" });
});
