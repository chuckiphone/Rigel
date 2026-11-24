#!/usr/bin/env node
// The Tauri bundle consumes three icon formats: .ico on Windows, .icns on macOS and PNGs on
// Linux. A missing or malformed asset is a release failure, not a packaging surprise. This gate
// runs without a desktop toolchain so it can sit beside the docs and hygiene checks.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const app = join(root, "crates", "rigel-app");
const configPath = join(app, "tauri.conf.json");

const passed = [];
const failed = [];

function check(condition, message) {
  if (condition) passed.push(message);
  else failed.push(message);
}

function read(path) {
  try {
    return readFileSync(path);
  } catch {
    return null;
  }
}

function insideApp(path) {
  const tail = relative(app, path);
  return tail !== "" && !tail.startsWith("..") && !tail.startsWith("/") && !tail.startsWith("\\");
}

function png(path, label, width, height) {
  const bytes = read(path);
  check(bytes !== null && bytes.length > 24, `${label} exists`);
  if (!bytes || bytes.length < 24) return;
  check(
    bytes[0] === 0x89 &&
      bytes.subarray(1, 4).toString("ascii") === "PNG" &&
      bytes[4] === 0x0d &&
      bytes[5] === 0x0a &&
      bytes[6] === 0x1a &&
      bytes[7] === 0x0a,
    `${label} has a PNG signature`,
  );
  check(bytes.toString("ascii", 12, 16) === "IHDR", `${label} has an IHDR chunk`);
  check(
    bytes.readUInt32BE(16) === width && bytes.readUInt32BE(20) === height,
    `${label} is ${width}x${height} (found ${bytes.readUInt32BE(16)}x${bytes.readUInt32BE(20)})`,
  );
}

function ico(path, label) {
  const bytes = read(path);
  check(bytes !== null && bytes.length > 6, `${label} exists`);
  if (!bytes || bytes.length < 6) return;
  check(
    bytes.readUInt16LE(0) === 0 && bytes.readUInt16LE(2) === 1 && bytes.readUInt16LE(4) > 0,
    `${label} has an ICO header and at least one image`,
  );
}

function icns(path, label) {
  const bytes = read(path);
  check(bytes !== null && bytes.length > 8, `${label} exists`);
  if (!bytes || bytes.length < 8) return;
  check(bytes.toString("ascii", 0, 4) === "icns", `${label} has an ICNS signature`);

  const declared = bytes.readUInt32BE(4);
  check(declared === bytes.length, `${label} length matches its header (${declared} bytes)`);

  const required = new Set(["icp4", "icp5", "icp6", "ic07", "ic08", "ic09", "ic10"]);
  const seen = new Set();
  let offset = 8;
  let structural = true;

  while (offset + 8 <= bytes.length) {
    const kind = bytes.toString("ascii", offset, offset + 4);
    const length = bytes.readUInt32BE(offset + 4);
    if (length < 8 || offset + length > bytes.length) {
      structural = false;
      break;
    }
    seen.add(kind);
    check(
      bytes[offset + 8] === 0x89 && bytes.toString("ascii", offset + 9, offset + 12) === "PNG",
      `${label} ${kind} carries PNG data`,
    );
    offset += length;
  }

  check(structural, `${label} has valid chunk bounds`);
  check(offset === bytes.length, `${label} ends at its declared length`);
  for (const kind of required) {
    check(seen.has(kind), `${label} carries ${kind}`);
  }
}

check(existsSync(configPath), "tauri.conf.json exists");
if (!existsSync(configPath)) {
  for (const message of failed) console.error(`  FAIL  ${message}`);
  process.exit(1);
}

let config;
try {
  config = JSON.parse(readFileSync(configPath, "utf8"));
} catch (error) {
  console.error(`  FAIL  tauri.conf.json is valid JSON: ${error.message}`);
  process.exit(1);
}

const bundle = config.bundle ?? {};
const configured = Array.isArray(bundle.icon) ? bundle.icon : [];
const required = [
  { path: "icons/icon.ico", kind: "ico", label: "Windows icon" },
  { path: "icons/icon.icns", kind: "icns", label: "macOS icon" },
  { path: "icons/32x32.png", kind: "png", label: "Linux 32px icon", width: 32, height: 32 },
  { path: "icons/128x128.png", kind: "png", label: "Linux 128px icon", width: 128, height: 128 },
  { path: "icons/128x128@2x.png", kind: "png", label: "Linux 256px icon", width: 256, height: 256 },
  { path: "icons/icon.png", kind: "png", label: "Linux 1024px icon", width: 1024, height: 1024 },
];

check(bundle.active === true, "bundling is active");
check(new Set(configured).size === configured.length, "bundle icon list has no duplicates");

for (const entry of required) {
  const path = resolve(app, entry.path);
  check(configured.includes(entry.path), `${entry.path} is listed in bundle.icon`);
  check(insideApp(path), `${entry.path} stays inside crates/rigel-app`);
  if (entry.kind === "ico") ico(path, entry.label);
  if (entry.kind === "icns") icns(path, entry.label);
  if (entry.kind === "png") png(path, entry.label, entry.width, entry.height);
}

for (const message of passed) console.log(`  ok    ${message}`);
for (const message of failed) console.error(`  FAIL  ${message}`);
if (failed.length > 0) {
  console.error(`bundle assets: ${failed.length} check(s) failed`);
  process.exit(1);
}
console.log(`bundle assets: ${passed.length} check(s) passed`);
