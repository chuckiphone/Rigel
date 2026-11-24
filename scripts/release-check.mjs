#!/usr/bin/env node
/**
 * The one command that reproduces `docs/release-checklist.md` (roadmap 5).
 *
 * It checks all four places a version lives, then runs the code gate the checklist names: fmt,
 * clippy, the workspace tests, the UI build and the docs check. The checklist and the gates are
 * validated against each other, so a gate that stops being documented — or a checklist line that
 * stops having a gate — fails here rather than in a release postmortem.
 *
 * Usage:
 *   node scripts/release-check.mjs            # the whole gate
 *   node scripts/release-check.mjs --list     # print the gates, run nothing
 *   node scripts/release-check.mjs --only versions [--root <dir>]
 *
 * Time budget: this is a full release gate, not a pre-commit hook. A warm tree takes about two
 * to four minutes on the reference Windows machine; a cold build can take ten. Each gate gets a
 * twenty-minute ceiling (override with RIGEL_RELEASE_GATE_TIMEOUT_MS) so a hung test process fails
 * with a named gate instead of blocking the release forever. The earlier 30-second shell window
 * was shorter than the gate by design, not a script hang.
 */

import { readFileSync, existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";

import { assertVersions } from "./lib/versions.mjs";

const args = process.argv.slice(2);
const rootIndex = args.indexOf("--root");
const ROOT = rootIndex >= 0 ? path.resolve(args[rootIndex + 1]) : process.cwd();
const onlyIndex = args.indexOf("--only");
const ONLY = onlyIndex >= 0 ? args[onlyIndex + 1] : null;
const LIST = args.includes("--list");

const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const configuredTimeout = Number(process.env.RIGEL_RELEASE_GATE_TIMEOUT_MS);
const GATE_TIMEOUT_MS =
  Number.isFinite(configuredTimeout) && configuredTimeout > 0
    ? configuredTimeout
    : 20 * 60 * 1000;

const gates = [
  { name: "versions", checklist: "Version is", command: null },
  { name: "format", checklist: "cargo fmt --all -- --check", command: ["cargo", ["fmt", "--all", "--", "--check"]] },
  {
    name: "lints",
    checklist: "cargo clippy --workspace --all-targets -- -D warnings",
    command: ["cargo", ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]],
  },
  { name: "tests", checklist: "cargo test --workspace", command: ["cargo", ["test", "--workspace", "--no-fail-fast"]] },
  { name: "ui build", checklist: "npm --prefix ui run build", command: [npm, ["--prefix", "ui", "run", "build"]] },
  { name: "docs", checklist: "node scripts/check-docs.mjs", command: [process.execPath, ["scripts/check-docs.mjs"]] },
];

if (LIST) {
  for (const gate of gates) console.log(`${gate.name.padEnd(10)} ${gate.checklist}`);
  process.exit(0);
}

function fail(message) {
  console.error(`release-check: ${message}`);
  process.exit(1);
}

const checklistPath = path.join(ROOT, "docs", "release-checklist.md");
if (!existsSync(checklistPath)) fail(`docs/release-checklist.md is missing under ${ROOT}`);
const checklist = readFileSync(checklistPath, "utf8");
for (const gate of gates) {
  if (!checklist.includes(gate.checklist)) {
    fail(`docs/release-checklist.md no longer documents the ${gate.name} gate (${gate.checklist})`);
  }
}

console.log(`release-check: ${ROOT}`);

const versions = assertVersions(ROOT);
if (versions.note) console.log(`  note: ${versions.note}`);
console.log(`  versions: ${versions.version} in all four places`);

if (ONLY === "versions") process.exit(0);
if (ONLY) fail(`unknown --only value: ${ONLY}`);

const startedAll = Date.now();
for (const gate of gates.slice(1)) {
  const started = Date.now();
  console.log(`\n▸ ${gate.name}: ${gate.checklist}`);
  const [command, commandArgs] = gate.command;
  // Only a .cmd shim needs a shell on Windows; `C:\\Program Files\\nodejs\\node.exe` must be
  // spawned directly, or the shell splits the path at the space.
  const result = spawnSync(command, commandArgs, {
    cwd: ROOT,
    stdio: "inherit",
    shell: process.platform === "win32" && /\.cmd$/i.test(command),
    env: { ...process.env, CI: process.env.CI ?? "true" },
    timeout: GATE_TIMEOUT_MS,
    killSignal: "SIGTERM",
  });
  if (result.error) {
    const limit = `${Math.round(GATE_TIMEOUT_MS / 1000)}s`;
    fail(
      result.error.code === "ETIMEDOUT"
        ? `${gate.name} exceeded its ${limit} time budget; raise RIGEL_RELEASE_GATE_TIMEOUT_MS only after finding the slow step`
        : `${gate.name} could not start: ${result.error.message}`,
    );
  }
  if (result.status !== 0) fail(`${gate.name} failed with exit code ${result.status}`);
  console.log(`  ${gate.name} ok in ${((Date.now() - started) / 1000).toFixed(1)}s`);
}

console.log(
  `\nrelease-check: every gate passed in ${((Date.now() - startedAll) / 1000).toFixed(1)}s`,
);
