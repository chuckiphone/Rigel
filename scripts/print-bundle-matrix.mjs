#!/usr/bin/env node
// Emit the GitHub Actions matrix from scripts/platform-targets.json. The workflow reads this
// through $GITHUB_OUTPUT, so the target list cannot drift between the config, the helper and CI.
// `bundles` is flattened for the Tauri CLI; the JSON file keeps the list structured for the gate.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const matrix = JSON.parse(readFileSync(join(root, "scripts", "platform-targets.json"), "utf8"));
const targets = Array.isArray(matrix.targets) ? matrix.targets : [];

if (targets.length === 0) {
  console.error("bundle matrix: scripts/platform-targets.json has no targets");
  process.exit(1);
}

const include = targets.map(({ id, platform, runner, target, bundles }) => ({
  id,
  platform,
  runner,
  target,
  bundles: bundles.join(","),
}));
process.stdout.write(`include=${JSON.stringify(include)}\n`);
