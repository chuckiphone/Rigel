/**
 * Roadmap 125: the Settings index and its address.
 *
 * The nav is not the index: each page carries its own rows, and a deep link names the exact row
 * to focus. Both the search results and the link parser are pure so a pasted address can be
 * checked without mounting the window. An unrecognised key is reported as not-found rather than
 * being silently ignored or treated as an error.
 */

export interface SettingsPageMeta {
  id: string;
  label: string;
  blurb: string;
  group: string;
  /** Extra words the page should answer to, beyond its label and blurb. */
  keywords: string;
}

export interface SettingEntry {
  key: string;
  label: string;
  page: string;
  keywords: string;
}

/** The named rows of the settings tree. Each key is a `data-setting-key` in settings.tsx. */
export const SETTING_ROWS: readonly SettingEntry[] = [
  { key: "model", label: "Model", page: "agent", keywords: "provider model picker agent" },
  { key: "effort", label: "Reasoning effort", page: "agent", keywords: "thinking depth" },
  { key: "step-cap", label: "Step cap", page: "agent", keywords: "max steps budget" },
  { key: "writable-agents", label: "Writable agents", page: "safety", keywords: "write access" },
  { key: "daily-cap", label: "Daily cap", page: "safety", keywords: "spend money limit" },
  { key: "permissions", label: "Tool grants", page: "permissions", keywords: "matrix roles ceiling" },
  { key: "kill-switch", label: "Network kill switch", page: "network", keywords: "offline egress" },
  { key: "audit-filter", label: "Audit filter", page: "audit", keywords: "approvals denials csv" },
  { key: "theme", label: "Theme", page: "appearance", keywords: "dark light system contrast" },
  { key: "density", label: "Density", page: "appearance", keywords: "compact display scale" },
  {
    key: "search-shortcut",
    label: "Search shortcut",
    page: "shortcuts",
    keywords: "keyboard binding palette",
  },
  {
    key: "focus-shortcut",
    label: "Focus shortcut",
    page: "shortcuts",
    keywords: "keyboard binding focus mode",
  },
  {
    key: "notify-approval",
    label: "Approval notification",
    page: "notifications",
    keywords: "tray desktop alert",
  },
  {
    key: "updates-enabled",
    label: "Update checks",
    page: "updates",
    keywords: "signed installer auto update",
  },
  { key: "diagnostics", label: "Diagnostics", page: "diagnostics", keywords: "bundle log crash" },
];

/** One searchable entry per page plus one per named row. */
export function buildIndex(pages: readonly SettingsPageMeta[]): SettingEntry[] {
  return [
    ...pages.map((page) => ({
      key: `page:${page.id}`,
      label: page.label,
      page: page.id,
      keywords: `${page.group} ${page.blurb} ${page.keywords}`,
    })),
    ...SETTING_ROWS,
  ];
}

/** Case-insensitive: a query matches a label, a page or any indexed keyword. */
export function searchSettings(index: readonly SettingEntry[], query: string): SettingEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...index];
  return index.filter((entry) =>
    `${entry.label} ${entry.key} ${entry.keywords}`.toLowerCase().includes(needle),
  );
}

export interface SettingsLink {
  page: string | null;
  focus: string | null;
  /** A focus key that the index does not know; the page should say so, not guess. */
  unknownKey: string | null;
}

/**
 * Read `#settings/<page>?focus=<key>`. Other query keys belong to the target page (the audit
 * filter uses them), so they are deliberately ignored here.
 */
export function settingsLinkFromHash(hash: string): SettingsLink {
  const match = /^#settings\/([a-z0-9-]+)/.exec(hash);
  if (!match) return { page: null, focus: null, unknownKey: null };
  const query = hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "";
  const focus = new URLSearchParams(query).get("focus");
  return { page: match[1], focus: focus && focus.trim() ? focus.trim() : null, unknownKey: null };
}

/** Resolve a link against the index: unknown pages and unknown keys become explicit states. */
export function resolveLink(
  link: SettingsLink,
  pages: readonly SettingsPageMeta[],
  index: readonly SettingEntry[],
): SettingsLink & { knownPage: boolean } {
  const knownPage = pages.some((page) => page.id === link.page);
  const knownKey = !link.focus || index.some((entry) => entry.key === link.focus);
  return {
    page: knownPage ? link.page : null,
    focus: knownPage && knownKey ? link.focus : null,
    unknownKey: knownPage && !knownKey ? link.focus : null,
    knownPage,
  };
}

/** `#settings/<page>?focus=<key>`; the focus is omitted when there is none. */
export function settingsHash(page: string, focus?: string | null): string {
  const key = focus?.trim();
  return `#settings/${page}${key ? `?focus=${encodeURIComponent(key)}` : ""}`;
}
