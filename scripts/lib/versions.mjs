// One rule for the four places a version lives, shared by `check-versions.mjs` (the fast CI
// gate) and `release-check.mjs` (the full release gate). The three build files must agree
// exactly. The website's download button may advertise the last published build instead — the
// receipts directory is the record of what that is — but it may never advertise a version newer
// than the tree, and its label must match its own data-version.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

export class VersionError extends Error {}

export function readVersions(root) {
  const read = (rel) => readFileSync(path.join(root, rel), "utf8");

  const cargo = read("Cargo.toml").match(
    /^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
  );
  if (!cargo) throw new VersionError("Cargo.toml has no [workspace.package] version");

  let tauri;
  try {
    tauri = JSON.parse(read("crates/rigel-app/tauri.conf.json"));
  } catch {
    throw new VersionError("crates/rigel-app/tauri.conf.json is not valid JSON");
  }
  let ui;
  try {
    ui = JSON.parse(read("ui/package.json"));
  } catch {
    throw new VersionError("ui/package.json is not valid JSON");
  }

  const html = read("web/index.html");
  const site = html.match(/data-version="([^"]+)"/);
  if (!site) throw new VersionError("web/index.html has no data-version on the download button");
  const label = html.match(/Get Rigel\s+(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)/);

  return {
    build: [
      { file: "Cargo.toml", version: cargo[1] },
      { file: "crates/rigel-app/tauri.conf.json", version: tauri.version },
      { file: "ui/package.json", version: ui.version },
    ],
    site: { file: "web/index.html", version: site[1], label: label?.[1] ?? null },
  };
}

/** Latest version with a receipt under `docs/release-receipts/`, or null. */
export function published(root) {
  const dir = path.join(root, "docs", "release-receipts");
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".md"))
    .map((name) => name.slice(0, -3))
    .filter((version) => SEMVER.test(version));
}

function less(left, right) {
  const [leftCore, leftPre] = left.split("-", 2);
  const [rightCore, rightPre] = right.split("-", 2);
  const [a, b, c] = leftCore.split(".").map(Number);
  const [x, y, z] = rightCore.split(".").map(Number);
  const core = a - x || b - y || c - z;
  if (core !== 0) return core;
  // A pre-release sorts before its release; otherwise compare the identifiers the way semver
  // orders them (numeric identifiers numerically, text lexically).
  if (!leftPre && !rightPre) return 0;
  if (!leftPre) return 1;
  if (!rightPre) return -1;
  const leftParts = leftPre.split(".");
  const rightParts = rightPre.split(".");
  for (let index = 0; index < Math.max(leftParts.length, rightParts.length); index += 1) {
    const one = leftParts[index];
    const other = rightParts[index];
    if (one === undefined) return -1;
    if (other === undefined) return 1;
    if (one === other) continue;
    const oneNumber = /^\d+$/.test(one) ? Number(one) : null;
    const otherNumber = /^\d+$/.test(other) ? Number(other) : null;
    if (oneNumber !== null && otherNumber !== null) return oneNumber - otherNumber;
    if (oneNumber !== null) return -1;
    if (otherNumber !== null) return 1;
    return one < other ? -1 : 1;
  }
  return 0;
}

/**
 * Assert the four places agree. Returns `{ version, note }`; throws a `VersionError` naming the
 * file that is stale, which is what the release checklist asks the gate to do.
 */
export function assertVersions(root) {
  const { build, site } = readVersions(root);
  const [first] = build;
  if (!SEMVER.test(first.version)) {
    throw new VersionError(`${first.file} has version ${first.version}, which is not MAJOR.MINOR.PATCH`);
  }
  for (const entry of build.slice(1)) {
    if (entry.version !== first.version) {
      throw new VersionError(
        `version mismatch: ${entry.file} has ${entry.version}, expected ${first.version} from ${first.file}`,
      );
    }
  }

  if (!SEMVER.test(site.version)) {
    throw new VersionError(`${site.file} has version ${site.version}, which is not MAJOR.MINOR.PATCH`);
  }
  if (site.label !== site.version) {
    throw new VersionError(
      `${site.file} downloads ${site.version} but its label says ${site.label ?? "nothing"}`,
    );
  }
  if (site.version !== first.version) {
    const released = published(root);
    if (less(site.version, first.version) && released.includes(site.version)) {
      return {
        version: first.version,
        note: `${site.file} advertises the published ${site.version}; the tree is ${first.version}`,
      };
    }
    throw new VersionError(
      `${site.file} advertises ${site.version}, but the tree is ${first.version} and no receipt records ${site.version}`,
    );
  }
  return { version: first.version, note: null };
}
