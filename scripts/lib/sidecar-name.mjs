// Tauri resolves `externalBin: binaries/rigel-b` to binaries/rigel-b-<rust-target>[.exe].
// Both the builder and the target-matrix gate import this rule, so they cannot drift.
export function sidecarFileName(target) {
  if (typeof target !== "string" || target.length === 0) {
    throw new Error("sidecarFileName needs a Rust target triple");
  }
  return `rigel-b-${target}${target.includes("windows") ? ".exe" : ""}`;
}
