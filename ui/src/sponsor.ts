/** The broker accepts only [A-Za-z0-9:_-] in a session id. A project path contains both a drive
 * colon and backslashes, and it should not leave this machine in an ad request anyway. Hash the
 * path into a stable id: the same project and slot keep their frequency cap without sending the
 * local folder name to Gravity.
 */
export function hashString(value: string): number {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

export function sponsorSession(project: string, slot: string): string {
  const clean = slot.replace(/[^A-Za-z0-9:_-]/g, "-").slice(0, 64) || "default";
  return `rigel-sponsor:${hashString(project).toString(16)}:${clean}`;
}

/** The advertiser's preferred mark, when the broker supplied a safe one. */
export function adArtwork(ad: { favicon?: string; brandImage?: string }): string {
  const value = (ad.favicon || ad.brandImage || "").trim();
  return /^(https:\/\/|data:image\/)/i.test(value) ? value : "";
}

/** A logo-shaped fallback for a fill with no usable artwork. */
export function adInitial(name?: string): string {
  const words = (name || "").trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "✦";
  return words
    .slice(0, 2)
    .map((word) => Array.from(word)[0]?.toUpperCase() ?? "")
    .join("");
}
