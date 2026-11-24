#!/usr/bin/env node
// One source of truth for the platform bundle matrix. This gate checks the Tauri configs,
// sidecar names, icon sets and CI matrix against scripts/platform-targets.json, then checks the
// filesystem in both directions so a sidecar or CI target cannot be orphaned.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sidecarFileName } from "./lib/sidecar-name.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const app = join(root, "crates", "rigel-app");
const matrixPath = join(root, "scripts", "platform-targets.json");
const ciPath = join(root, ".github", "workflows", "ci.yml");
const releasePath = join(root, ".github", "workflows", "release.yml");
const dump = process.argv.includes("--dump");

const EXPECTED = {
  windows: {
    bundles: ["msi", "nsis"],
    icons: ["icons/icon.ico"],
    config: "tauri.conf.json",
    artifacts: [
      { dir: "msi", suffix: ".msi" },
      { dir: "nsis", suffix: "-setup.exe" },
    ],
  },
  macos: {
    bundles: ["app", "dmg"],
    icons: ["icons/icon.icns"],
    config: "tauri.macos.conf.json",
    artifacts: [
      { dir: "macos", suffix: ".app" },
      { dir: "dmg", suffix: ".dmg" },
    ],
  },
  linux: {
    bundles: ["deb", "rpm", "appimage"],
    icons: [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.png",
    ],
    config: "tauri.linux.conf.json",
    artifacts: [
      { dir: "deb", suffix: ".deb" },
      { dir: "rpm", suffix: ".rpm" },
      { dir: "appimage", suffix: ".AppImage" },
    ],
  },
};

const passed = [];
const failed = [];
const notes = [];

function check(condition, message) {
  if (condition) passed.push(message);
  else failed.push(message);
}

function sameList(actual, expected) {
  return (
    Array.isArray(actual) &&
    actual.length === expected.length &&
    actual.every((value, index) => value === expected[index])
  );
}

function read(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

function readJson(path, label) {
  const text = read(path);
  check(text !== null, `${label} exists`);
  if (text === null) return null;
  try {
    return JSON.parse(text);
  } catch (error) {
    check(false, `${label} is valid JSON: ${error.message}`);
    return null;
  }
}

// RFC 7396 merge patch: objects merge by key, arrays and scalars replace.
function mergePatch(base, patch) {
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) return patch;
  const out = base && typeof base === "object" && !Array.isArray(base) ? { ...base } : {};
  for (const [key, value] of Object.entries(patch)) {
    out[key] = mergePatch(out[key], value);
  }
  return out;
}

function insideApp(path) {
  const tail = relative(app, path);
  return tail !== "" && !tail.startsWith("..") && !tail.startsWith("/") && !tail.startsWith("\\");
}

function sidecarPath(name) {
  return join(app, "binaries", name);
}

function requireSidecar(id) {
  const target = matrix.targets.find((row) => row.id === id);
  if (!target || typeof target.sidecar !== "string") {
    check(false, `--require-sidecar ${id}: target is declared`);
    return;
  }
  const path = sidecarPath(target.sidecar);
  check(existsSync(path), `${id} sidecar exists after its build`);
  if (existsSync(path)) check(statSync(path).size > 0, `${id} sidecar is non-empty`);
}

function requireArtifacts(id) {
  const target = matrix.targets.find((row) => row.id === id);
  if (!target || !EXPECTED[target.platform]) {
    check(false, `--artifacts ${id}: target is declared`);
    return;
  }
  const base = join(root, "target", target.target, "release", "bundle");
  for (const artifact of EXPECTED[target.platform].artifacts) {
    const dir = join(base, artifact.dir);
    check(existsSync(dir), `${id} ${artifact.dir} bundle directory exists`);
    if (!existsSync(dir)) continue;
    const entries = readdirSync(dir);
    check(
      entries.some((name) => name.endsWith(artifact.suffix)),
      `${id} produced a ${artifact.suffix} artifact in ${artifact.dir}`,
    );
  }
}

const matrix = readJson(matrixPath, "scripts/platform-targets.json") ?? { targets: [] };
const baseConfig = readJson(join(app, "tauri.conf.json"), "tauri.conf.json") ?? {};
const ci = read(ciPath) ?? "";
const release = read(releasePath) ?? "";

check(Array.isArray(matrix.targets) && matrix.targets.length > 0, "the matrix has targets");
check(ci.length > 0, ".github/workflows/ci.yml is readable");
check(release.length > 0, ".github/workflows/release.yml is readable");

const ids = new Set();
const targets = new Set();
const sidecars = new Set();
const targetRows = Array.isArray(matrix.targets) ? matrix.targets : [];

for (const row of targetRows) {
  if (!row || typeof row !== "object" || Array.isArray(row)) {
    check(false, "each matrix row is an object");
    continue;
  }
  const platform = row.platform;
  const expected = EXPECTED[platform];
  const runner = typeof row.runner === "string" ? row.runner : "";
  check(typeof row.id === "string" && /^[a-z0-9-]+$/.test(row.id), `target id ${row.id} is a slug`);
  check(!ids.has(row.id), `target id ${row.id} is unique`);
  ids.add(row.id);
  check(Boolean(expected), `${row.id} has a known platform`);
  if (!expected) continue;

  check(runner.length > 0, `${row.id} has a runner`);
  const targetTriple = typeof row.target === "string" ? row.target : "";
  check(targetTriple.length > 0, `${row.id} has a Rust target`);
  check(targetTriple.length === 0 || !targets.has(targetTriple), `${row.target} is declared once`);
  if (targetTriple) targets.add(targetTriple);
  check(row.config === expected.config, `${row.id} uses ${expected.config}`);
  check(sameList(row.bundles, expected.bundles), `${row.id} declares ${expected.bundles.join(", ")}`);
  check(sameList(row.icons, expected.icons), `${row.id} declares its complete icon set`);
  check(
    targetTriple.length > 0 && row.sidecar === sidecarFileName(targetTriple),
    `${row.id} sidecar follows the shared rigel-b-<target> rule for ${row.target}`,
  );
  check(!sidecars.has(row.sidecar), `${row.sidecar} is declared once`);
  sidecars.add(row.sidecar);

  if (platform === "windows") {
    check(targetTriple === "x86_64-pc-windows-msvc", "Windows target is x64 MSVC");
    check(/^windows-/.test(runner), `${row.id} uses a Windows runner`);
  } else if (platform === "macos") {
    check(
      targetTriple === "aarch64-apple-darwin" || targetTriple === "x86_64-apple-darwin",
      "macOS target is Apple silicon or Intel",
    );
    check(/^macos-/.test(runner), `${row.id} uses a macOS runner`);
    if (targetTriple === "aarch64-apple-darwin") {
      check(
        !runner.includes("intel") && !runner.includes("large"),
        `${row.id} uses a native Apple-silicon runner`,
      );
    } else {
      check(
        runner.includes("intel") || runner.includes("large"),
        `${row.id} uses an Intel macOS runner`,
      );
    }
  } else {
    check(targetTriple === "x86_64-unknown-linux-gnu", "Linux target is x64 GNU");
    check(/^ubuntu-/.test(runner), `${row.id} uses an Ubuntu runner`);
    check(!runner.includes("-arm"), `${row.id} uses an x64 Linux runner`);
  }

  const configName = typeof row.config === "string" ? row.config : "";
  const overlayPath = join(app, configName || "missing-config.json");
  const overlay = configName === "tauri.conf.json" ? {} : (readJson(overlayPath, row.config) ?? {});
  const resolved = mergePatch(baseConfig, overlay);
  const bundle = resolved.bundle ?? {};
  if (dump) {
    console.log(
      `resolved ${row.id}: ${JSON.stringify({
        targets: bundle.targets,
        externalBin: bundle.externalBin,
        icon: bundle.icon,
      })}`,
    );
  }
  check(sameList(bundle.targets, expected.bundles), `${row.id} resolved targets match`);
  check(
    Array.isArray(bundle.externalBin) && bundle.externalBin.includes("binaries/rigel-b"),
    `${row.id} keeps the rigel-b externalBin entry`,
  );
  for (const icon of expected.icons) {
    check(
      Array.isArray(bundle.icon) && bundle.icon.includes(icon),
      `${row.id} resolved icon list includes ${icon}`,
    );
    const iconPath = join(app, icon);
    check(existsSync(iconPath) && statSync(iconPath).size > 0, `${row.id} icon ${icon} is present`);
  }

  const sidecar = typeof row.sidecar === "string" ? row.sidecar : "__missing-sidecar__";
  const path = sidecarPath(sidecar);
  check(insideApp(path), `${row.id} sidecar stays inside crates/rigel-app`);
  if (!existsSync(path)) {
    notes.push(`${row.id} sidecar is configured but not built here: ${sidecar}`);
  } else {
    check(statSync(path).size > 0, `${row.id} committed sidecar is non-empty`);
  }
}

// Reverse checks: the matrix is the only CI list, and every sidecar on disk is declared.
const output = (() => {
  try {
    return execFileSync(process.execPath, [join(root, "scripts", "print-bundle-matrix.mjs")], {
      encoding: "utf8",
    }).trim();
  } catch (error) {
    check(false, `print-bundle-matrix.mjs runs: ${error.message}`);
    return "";
  }
})();
check(output.startsWith("include="), "the CI matrix starts with include=");
if (output.startsWith("include=")) {
  try {
    const ciTargets = JSON.parse(output.slice("include=".length));
    const normalized = (rows) =>
      rows
        .map(({ id, platform, runner, target, bundles }) => ({
          id,
          platform,
          runner,
          target,
          bundles: Array.isArray(bundles) ? bundles.join(",") : bundles,
        }))
        .sort((a, b) => a.id.localeCompare(b.id));
    check(
      JSON.stringify(normalized(ciTargets)) === JSON.stringify(normalized(targetRows)),
      "the CI matrix matches every target, runner and bundle list, with no orphans",
    );
  } catch (error) {
    check(false, `the CI matrix is valid JSON: ${error.message}`);
  }
}

const binaries = join(app, "binaries");
if (existsSync(binaries)) {
  for (const name of readdirSync(binaries)) {
    if (!name.startsWith("rigel-b-")) continue;
    check(sidecars.has(name), `sidecar ${name} is declared in the matrix`);
  }
}

// CI wiring checks. The matrix is generated from the same JSON, so there is no second target list.
check(ci.includes("bundle-matrix:"), "CI has a bundle-matrix job");
check(ci.includes("node scripts/print-bundle-matrix.mjs"), "CI reads the shared matrix");
check(
  ci.includes("fromJSON(needs.bundle-matrix.outputs.include)"),
  "CI expands the shared matrix",
);
check(ci.includes("name: bundle-${{ matrix.id }}"), "CI names each bundle instance");
check(ci.includes("runs-on: ${{ matrix.runner }}"), "CI uses the matrix runner");
const hardcodedBundleJobs = [...ci.matchAll(/^  (bundle-[a-z0-9-]+):\s*$/gm)]
  .map((match) => match[1])
  .filter((name) => name !== "bundle-matrix");
check(hardcodedBundleJobs.length === 0, "CI has no hardcoded bundle-* job keys");
const ciTargetArguments = [...ci.matchAll(/--target\s+(\$\{\{[^}]+\}\}|\S+)/g)].map(
  (match) => match[1].trim(),
);
check(
  ciTargetArguments.length === 2 && ciTargetArguments.every((value) => value === "${{ matrix.target }}"),
  "CI passes only the matrix target to sidecar and bundle builds",
);
check(
  ci.includes("node scripts/build-rigel-b.mjs --target ${{ matrix.target }}"),
  "CI builds the sidecar for the matrix target",
);
check(
  ci.includes("node scripts/check-platform-targets.mjs --require-sidecar ${{ matrix.id }}"),
  "CI requires the sidecar after building it",
);
check(
  ci.includes("cargo tauri build --target ${{ matrix.target }} --bundles ${{ matrix.bundles }}"),
  "CI builds the bundle for the matrix target",
);
check(
  ci.includes("node scripts/check-platform-targets.mjs --artifacts ${{ matrix.id }}"),
  "CI checks the bundle artifacts after building",
);
check(ci.includes("if: matrix.platform == 'linux'"), "CI has a Linux-only dependency step");
for (const packageName of [
  "libwebkit2gtk-4.1-dev",
  "libayatana-appindicator3-dev",
  "libxdo-dev",
  "librsvg2-dev",
  "patchelf",
]) {
  check(ci.includes(packageName), `CI installs ${packageName}`);
}
check(ci.includes("timeout-minutes: 120"), "the bundle job has a 120-minute timeout");
check(
  release.includes("node scripts/build-rigel-b.mjs --target x86_64-pc-windows-msvc"),
  "the Windows release builds its own sidecar",
);
check(
  release.includes("node scripts/check-platform-targets.mjs --require-sidecar windows-x64"),
  "the Windows release requires its sidecar",
);
const releaseTargetArguments = [...release.matchAll(/--target\s+(\$\{\{[^}]+\}\}|\S+)/g)].map(
  (match) => match[1].trim(),
);
check(
  releaseTargetArguments.every((value) => value === "x86_64-pc-windows-msvc"),
  "the Windows release builds only the Windows target",
);
check(release.includes("timeout-minutes: 180"), "the release gate has a 180-minute timeout");
check(release.includes("+ 150 * 60"), "the release gate waits 150 minutes for CI");

if (process.argv.includes("--require-sidecar")) {
  const id = process.argv[process.argv.indexOf("--require-sidecar") + 1];
  check(Boolean(id), "--require-sidecar names a target");
  if (id) requireSidecar(id);
}
if (process.argv.includes("--artifacts")) {
  const id = process.argv[process.argv.indexOf("--artifacts") + 1];
  check(Boolean(id), "--artifacts names a target");
  if (id) requireArtifacts(id);
}

for (const message of passed) console.log(`  ok    ${message}`);
for (const message of notes) console.log(`  note  ${message}`);
for (const message of failed) console.error(`  FAIL  ${message}`);
if (failed.length > 0) {
  console.error(`platform targets: ${failed.length} check(s) failed`);
  process.exit(1);
}
console.log(
  `platform targets: ${passed.length} check(s) passed, ${notes.length} not built on this host`,
);
