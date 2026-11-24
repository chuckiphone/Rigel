// Tauri runs its hooks from different directories depending on whether the CLI is invoked at
// the repository root or beside tauri.conf.json. Resolve the repository from this file instead
// of assuming either cwd, so the dev window never points at a Vite server that was never started.
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const task = process.argv[2] === "build" ? "build" : "dev";
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const result = spawnSync(npm, ["--prefix", join(root, "ui"), "run", task], {
  cwd: root,
  stdio: "inherit",
  shell: process.platform === "win32",
});

if (result.error) {
  console.error(`rigel: could not start the UI ${task}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
