#!/usr/bin/env node
// Build the Rigel b sidecar for one Rust target and place it where Tauri's `externalBin`
// resolves it: binaries/rigel-b-<target-triple>[.exe]. Tauri does not cross-compile installers,
// so CI runs this natively on each target; a macOS universal build needs a separate lipo step.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sidecarFileName } from "./lib/sidecar-name.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dryRun = process.argv.includes("--dry-run");
const targetFlag = process.argv.indexOf("--target");
const requested = targetFlag >= 0 ? process.argv[targetFlag + 1] : undefined;

if (targetFlag >= 0 && (!requested || requested.startsWith("--"))) {
  console.error("rigel-b: --target needs a Rust target triple");
  process.exit(1);
}
if (requested && !/^[a-z0-9_]+(?:-[a-z0-9_]+)+$/.test(requested)) {
  console.error(`rigel-b: invalid Rust target triple: ${requested}`);
  process.exit(1);
}

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" })
  .split(/\r?\n/)
  .find((line) => line.startsWith("host: "))
  ?.slice("host: ".length)
  .trim();

if (!host) {
  console.error("rigel-b: rustc -vV did not report a host target");
  process.exit(1);
}

const target = requested ?? host;
const extension = target.includes("windows") ? ".exe" : "";
const name = sidecarFileName(target);
const built = join(root, "vendor", "rigel-b", "target", target, "release", `rigel-b${extension}`);
const destination = join(root, "crates", "rigel-app", "binaries", name);
const args = [
  "build",
  "--release",
  "--manifest-path",
  "vendor/rigel-b/Cargo.toml",
  "--target-dir",
  "vendor/rigel-b/target",
  "--target",
  target,
  "-p",
  "rigel-b",
  "--features",
  "render",
];

console.log(`rigel-b: target ${target} (host ${host})`);
console.log(`rigel-b: cargo ${args.join(" ")}`);
console.log(`rigel-b: destination ${destination}`);

if (dryRun) {
  console.log("rigel-b: dry run; nothing was built or copied");
  process.exit(0);
}

execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
if (!existsSync(built)) {
  console.error(`rigel-b: build finished but ${built} does not exist`);
  process.exit(1);
}
if (statSync(built).size === 0) {
  console.error(`rigel-b: ${built} is empty`);
  process.exit(1);
}

mkdirSync(dirname(destination), { recursive: true });
copyFileSync(built, destination);
const digest = createHash("sha256").update(readFileSync(destination)).digest("hex");
console.log(`rigel-b: wrote ${destination} (${statSync(destination).size} bytes, sha256 ${digest})`);
