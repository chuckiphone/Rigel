import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { aboutLicences } from "./make-about-licences.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = readFileSync(path.join(ROOT, "ui", "src", "about-licences.ts"), "utf8");
const CHECKED_IN = JSON.parse(SOURCE.slice(SOURCE.indexOf("{"), SOURCE.indexOf("} as const;") + 1));

test("the About licence list is the licence set of the release SBOM", () => {
  assert.deepEqual(CHECKED_IN, aboutLicences(ROOT));
  assert.equal(CHECKED_IN.source, "CycloneDX 1.5 SBOM from scripts/make-sbom.mjs");
  assert.ok(CHECKED_IN.components > 0, "the SBOM has components");
  for (const expression of ["MIT", "Apache-2.0", "BSD-3-Clause"]) {
    assert.ok(CHECKED_IN.expressions.includes(expression), `${expression} is in the SBOM`);
  }
});
