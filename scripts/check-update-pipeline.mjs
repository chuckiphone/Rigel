#!/usr/bin/env node
/**
 * Prove the release pipeline still publishes what the updater reads.
 *
 * The static pass is offline and runs in CI. `--live` additionally checks the current GitHub
 * release's `latest.json`, Ed25519 signature and asset names against the same repository and
 * publisher key the app uses. It never downloads an installer.
 */
import { createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

const ROOT = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, ROOT), "utf8");
const fail = (message) => {
  console.error(`update pipeline: FAIL ${message}`);
  process.exit(1);
};
let checks = 0;
const assert = (condition, message) => {
  if (!condition) fail(message);
  checks += 1;
};

const repo = "chuckiphone/Rigel";
const manifestUrl = `https://github.com/${repo}/releases/latest/download/latest.json`;
const cargo = read("Cargo.toml");
const core = read("crates/rigel-core/src/update.rs");
const workflow = read(".github/workflows/release.yml").replace(/\r\n/g, "\n");
const signer = read("scripts/sign-update-manifest.mjs");
const pubkey = read("crates/rigel-app/resources/update-pubkey.txt").trim();

assert(
  cargo.includes(`repository = "https://github.com/${repo}"`),
  `Cargo.toml repository is not https://github.com/${repo}`,
);
assert(
  core.includes(manifestUrl),
  `the app does not check ${manifestUrl}`,
);
assert(
  /^[0-9a-f]{64}$/i.test(pubkey),
  "the committed update public key is not 64 hex characters",
);

assert(workflow.includes("tags:"), "release.yml has no tag trigger");
assert(workflow.includes('"v*"'), "release.yml does not trigger on v* tags");
assert(workflow.includes("cargo tauri build"), "release.yml does not build Tauri bundles");
assert(
  workflow.includes("Stamp the release version"),
  "release.yml does not stamp the tag into the build",
);
assert(
  workflow.includes("RIGEL_UPDATE_SIGNING_KEY"),
  "release.yml does not read RIGEL_UPDATE_SIGNING_KEY",
);
assert(
  workflow.includes("node scripts/sign-update-manifest.mjs latest.json"),
  "release.yml does not sign latest.json",
);
assert(
  workflow.includes("RIGEL_UPDATE_VERSION") &&
    workflow.includes("RIGEL_UPDATE_URL") &&
    workflow.includes("RIGEL_UPDATE_SHA256"),
  "release.yml does not sign the NSIS version, URL and SHA-256",
);
assert(
  workflow.includes("RIGEL_UPDATE_MSI_URL") &&
    workflow.includes("RIGEL_UPDATE_MSI_SHA256"),
  "release.yml does not sign the optional MSI pair",
);
assert(
  workflow.includes("update check --manifest latest.json") &&
    workflow.includes("update verify $nsis.FullName --manifest latest.json") &&
    workflow.includes("update verify $msi.FullName --manifest latest.json"),
  "release.yml does not verify both installers against the manifest before publishing",
);
assert(
  workflow.includes("RIGEL_UPDATE_BIN_URL") &&
    workflow.includes("RIGEL_UPDATE_BIN_SHA256") &&
    workflow.includes("RIGEL_UPDATE_BIN_TARGET"),
  "release.yml does not sign the raw binary asset",
);
assert(
  workflow.includes("gh release upload $tag latest.json $nsis.FullName $msi.FullName $bin") &&
    workflow.includes("gh release create $tag latest.json $nsis.FullName $msi.FullName $bin"),
  "release.yml does not publish latest.json, NSIS, MSI and the binary together",
);
assert(
  workflow.includes("node scripts/release-notes-body.mjs") &&
    workflow.includes('$publishedNotes = Join-Path $env:RUNNER_TEMP "release-notes.md"') &&
    workflow.includes("gh release edit $tag --notes-file $publishedNotes") &&
    workflow.includes('@("--notes-file", $publishedNotes)'),
  "release.yml does not publish release notes with tag-anchored doc links",
);
assert(
  workflow.includes('$version -match "-"') && workflow.includes("--prerelease"),
  "release.yml does not keep prerelease tags out of releases/latest",
);
assert(workflow.includes('else { "--latest" }'), "release.yml no longer marks stable releases latest");

assert(
  signer.includes("RIGEL_UPDATE_SIGNING_KEY") &&
    signer.includes("createPrivateKey") &&
    signer.includes("const manifest =") &&
    signer.includes("signature") &&
    signer.includes("msi_url") &&
    signer.includes("msi_sha256") &&
    signer.includes("bin_signature") &&
    signer.includes("bin_target"),
  "sign-update-manifest.mjs does not produce the signed MSI/NSIS/binary manifest",
);

const staticChecks = checks;
console.log(`update pipeline: ${staticChecks} static check(s) passed`);

if (process.argv.includes("--live")) {
  const headers = { Accept: "application/vnd.github+json", "User-Agent": "rigel-update-check" };
  const releaseResponse = await fetch(
    `https://api.github.com/repos/${repo}/releases/latest`,
    { headers },
  );
  assert(releaseResponse.ok, `GitHub release lookup returned HTTP ${releaseResponse.status}`);
  const release = await releaseResponse.json();
  const tag = release.tag_name;
  assert(/^v\d+\.\d+\.\d+/.test(tag), "the latest release has no v-prefixed version tag");
  const assets = new Map((release.assets ?? []).map((asset) => [asset.name, asset]));
  const names = new Set(assets.keys());
  assert(names.has("latest.json"), "the latest GitHub release has no latest.json asset");
  assert(
    [...names].some((name) => name.endsWith("-setup.exe")),
    "the latest GitHub release has no NSIS setup asset",
  );
  assert(
    [...names].some((name) => name.endsWith(".msi")),
    "the latest GitHub release has no MSI asset",
  );

  const manifestResponse = await fetch(
    `https://github.com/${repo}/releases/latest/download/latest.json`,
    { headers },
  );
  assert(manifestResponse.ok, `latest.json returned HTTP ${manifestResponse.status}`);
  const manifest = await manifestResponse.json();
  const version = tag.slice(1);
  assert(manifest.version === version, `latest.json says ${manifest.version}, release is ${version}`);
  assert(/^[0-9a-f]{64}$/i.test(manifest.sha256), "latest.json has no SHA-256");
  assert(/^[0-9a-f]{128}$/i.test(manifest.signature), "latest.json has no Ed25519 signature");
  assert(
    manifest.url.includes(`/releases/download/${tag}/`),
    "latest.json installer URL is not on this release",
  );
  const installerName = manifest.url.split("/").pop();
  assert(
    assets.get(installerName)?.browser_download_url === manifest.url,
    "latest.json installer URL does not name a published release asset",
  );
  if (manifest.msi_url) {
    assert(
      manifest.msi_url.includes(`/releases/download/${tag}/`),
      "latest.json MSI URL is not on this release",
    );
    const msiName = manifest.msi_url.split("/").pop();
    assert(
      assets.get(msiName)?.browser_download_url === manifest.msi_url,
      "latest.json MSI URL does not name a published release asset",
    );
    assert(/^[0-9a-f]{64}$/i.test(manifest.msi_sha256), "latest.json MSI has no SHA-256");
  }
  if (manifest.bin_url) {
    assert(
      manifest.bin_url.includes(`/releases/download/${tag}/`),
      "latest.json binary URL is not on this release",
    );
    const binName = manifest.bin_url.split("/").pop();
    assert(
      assets.get(binName)?.browser_download_url === manifest.bin_url,
      "latest.json binary URL does not name a published release asset",
    );
    assert(/^[0-9a-f]{64}$/i.test(manifest.bin_sha256), "latest.json binary has no SHA-256");
    assert(/\S+-\S+/.test(manifest.bin_target ?? ""), "latest.json binary has no platform tag");
    assert(
      verify(
        null,
        Buffer.from(`${manifest.bin_url}\n${manifest.bin_sha256}\n${manifest.bin_target}`),
        publicKey,
        Buffer.from(manifest.bin_signature, "hex"),
      ),
      "latest.json binary signature does not verify against the committed public key",
    );
  }

  const payload = [manifest.version, manifest.url, manifest.sha256];
  if (manifest.msi_url && manifest.msi_sha256) {
    payload.push(manifest.msi_url, manifest.msi_sha256);
  }
  const publicKey = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      Buffer.from(pubkey, "hex"),
    ]),
    format: "der",
    type: "spki",
  });
  assert(
    verify(null, Buffer.from(payload.join("\n")), publicKey, Buffer.from(manifest.signature, "hex")),
    "latest.json signature does not verify against the committed public key",
  );
  console.log(`update pipeline: live ${tag} assets=${[...names].sort().join(",")}`);
  console.log(`update pipeline: live manifest ${manifest.version} signature verified`);
}
