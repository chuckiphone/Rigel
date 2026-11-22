/**
 * D159: window scale and row density.
 *
 * Both are written to `<html>` so the CSS owns the layout and components keep no copy. The scale
 * is a `zoom` on `#root`, with its box divided by the same factor so the shell still fills the
 * window.
 */
export const UI_SCALES = [90, 100, 110, 125] as const;
export type UiScale = (typeof UI_SCALES)[number];

/** Source ids are storage keys; these are the names a reader sees. */
const SOURCE_NAMES: Record<string, string> = {
  github: "GitHub",
  skillsmp: "SkillsMP",
  skillhub: "SkillHub",
  registry: "Registry",
  smithery: "Smithery",
  awesome: "Awesome",
  local: "Local",
  bundled: "Bundled",
  rigel: "Rigel",
};

export function sourceLabel(id: string | null | undefined): string {
  if (!id) return "—";
  return SOURCE_NAMES[id] ?? id;
}

const ACRONYMS = new Set(["api", "cli", "http", "https", "id", "mcp", "ok", "pdf", "ui"]);

/** A storage value such as `streamable-http` as a reader-facing label. */
export function humanize(value: string | null | undefined): string {
  if (!value) return "—";
  return value
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((word) =>
      ACRONYMS.has(word.toLowerCase())
        ? word.toUpperCase()
        : `${word.charAt(0).toUpperCase()}${word.slice(1)}`,
    )
    .join(" ");
}

export function isUiScale(value: number | null | undefined): value is UiScale {
  return UI_SCALES.some((scale) => scale === Number(value));
}

/** Apply the saved values; an unrecognised scale is 100 rather than a fraction of one. */
export function applyDisplay(
  scale: number | null | undefined,
  compact: boolean | null | undefined,
): void {
  const next = isUiScale(scale) ? scale : 100;
  document.documentElement.style.setProperty("--ui-scale", String(next / 100));
  document.documentElement.dataset.density = compact ? "compact" : "roomy";
  document.documentElement.dataset.scale = String(next);
}
