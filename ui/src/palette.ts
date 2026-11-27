/**
 * Roadmap 113: the command palette's data model.
 *
 * One ordered list of entries, grouped into five sections: actions, files, symbols, sessions and
 * settings. Skills, turns and patches remain in their own sections because the core already
 * searches them; the tsx component only renders and runs entries, it does not decide what exists.
 */

import { ACTIONS, type PaletteAction } from "./palette-actions";
import type { PaletteResults } from "./api";

export type { PaletteAction } from "./palette-actions";
export { ACTIONS } from "./palette-actions";
// The component lives in `palette.tsx`; this module owns its data. Re-exporting the default keeps
// callers on one import path without duplicating either file.
export { default } from "./palette-view";

export type PaletteSection =
  | "actions"
  | "files"
  | "symbols"
  | "sessions"
  | "settings"
  | "skills"
  | "turns"
  | "patches";

export const SECTION_LABELS: Record<PaletteSection, string> = {
  actions: "Actions",
  files: "Files",
  symbols: "Symbols",
  sessions: "Sessions",
  settings: "Settings",
  skills: "Skills",
  turns: "Turns",
  patches: "Patches",
};

/** The five roadmap sections, in the order they are shown. */
export const PALETTE_SECTIONS: PaletteSection[] = [
  "actions",
  "files",
  "symbols",
  "sessions",
  "settings",
];

export interface PaletteEntry {
  section: PaletteSection;
  kind: string;
  key: string;
  title: string;
  subtitle: string;
  meta: string;
  action?: PaletteAction;
  id?: string;
  sessionId?: string;
  turnId?: string;
  patchId?: string;
}

/** Settings rows are real destinations, not search placeholders. */
export const SETTINGS_TARGETS: { id: string; label: string; detail: string }[] = [
  { id: "providers", label: "Providers", detail: "Model doors and credentials" },
  { id: "notifications", label: "Notifications", detail: "Routing and quiet hours" },
  { id: "safety", label: "Safety", detail: "Approvals and writable agents" },
  { id: "appearance", label: "Appearance", detail: "Theme, contrast and density" },
  { id: "diagnostics", label: "Diagnostics", detail: "Bug-report bundle and setup checks" },
];

export function matches(query: string, ...values: string[]): boolean {
  const haystack = values.join(" ").toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((token) => haystack.includes(token));
}

export function shortId(id: string): string {
  return id.slice(0, 8);
}

/** Build the palette in section order. An empty query keeps actions and settings only. */
export function paletteEntries(
  results: PaletteResults,
  query: string,
  focusShortcut: string,
): PaletteEntry[] {
  const rows: PaletteEntry[] = [];

  for (const action of ACTIONS) {
    if (!matches(query, action.label, action.detail)) continue;
    rows.push({
      section: "actions",
      kind: "action",
      key: `action:${action.id}`,
      title: action.label,
      subtitle: action.detail,
      meta: action.id === "focus" ? focusShortcut : "action",
      action: action.id,
    });
  }

  if (query.trim()) {
    for (const file of results.files ?? []) {
      rows.push({
        section: "files",
        kind: "file",
        key: `file:${file.path}`,
        title: file.path,
        subtitle: `${file.language} · project file`,
        meta: "file",
        id: file.path,
      });
    }
    for (const symbol of results.symbols ?? []) {
      rows.push({
        section: "symbols",
        kind: "symbol",
        key: `symbol:${symbol.path}:${symbol.line ?? 0}:${symbol.name}`,
        title: symbol.name,
        subtitle: `${symbol.path}${symbol.line ? `:${symbol.line}` : ""}`,
        meta: symbol.language,
        id: symbol.path,
      });
    }
  }

  for (const session of results.sessions) {
    rows.push({
      section: "sessions",
      kind: "session",
      key: `session:${session.id}`,
      title: session.title || "Untitled session",
      subtitle: `${session.status} · ${session.id.slice(0, 8)}`,
      meta: "session",
      id: session.id,
    });
  }
  for (const target of SETTINGS_TARGETS) {
    if (!matches(query, target.label, target.detail)) continue;
    rows.push({
      section: "settings",
      kind: "setting",
      key: `setting:${target.id}`,
      title: target.label,
      subtitle: target.detail,
      meta: "settings",
      id: target.id,
    });
  }

  for (const skill of results.skills) {
    rows.push({
      section: "skills",
      kind: "skill",
      key: `skill:${skill.id}`,
      title: skill.name,
      subtitle: skill.description,
      meta: skill.grade ? `skill · ${skill.grade}` : "skill · unscanned",
      id: skill.id,
    });
  }
  for (const turn of results.turns) {
    rows.push({
      section: "turns",
      kind: "turn",
      key: `turn:${turn.turn_id}`,
      title: turn.snippet || turn.prompt || `Turn ${turn.seq}`,
      subtitle: `${turn.role || turn.harness} · turn ${turn.seq}`,
      meta: "turn",
      sessionId: turn.session_id,
      turnId: turn.turn_id,
    });
  }
  for (const patch of results.patches) {
    rows.push({
      section: "patches",
      kind: "patch",
      key: `patch:${patch.id}`,
      title: patch.path,
      subtitle: patch.status,
      meta: "patch",
      sessionId: patch.session_id,
      patchId: patch.id,
    });
  }
  return rows;
}

/** Contiguous section ranges for rendering headings without reordering entries. */
export function sectionRanges(
  entries: readonly PaletteEntry[],
): { label: string; from: number; to: number }[] {
  const sections: { label: string; from: number; to: number }[] = [];
  entries.forEach((entry, index) => {
    const last = sections.at(-1);
    if (last && last.label === SECTION_LABELS[entry.section]) last.to = index;
    else sections.push({ label: SECTION_LABELS[entry.section], from: index, to: index });
  });
  return sections;
}
