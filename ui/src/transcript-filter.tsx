/**
 * Roadmap 111: one filter for the transcript rows, the turn outline and in-session search.
 *
 * A filter is a view of the same recorded rows, never a second store: the bar, the outline and
 * the search all call `visibleItems`, so a row that is hidden in one place is hidden in the others
 * and the hidden-row count is exact. The choice is remembered per project and session.
 */

import type { Item, TurnTranscript } from "./api";

export type TranscriptFilter =
  | "all"
  | "yours"
  | "model"
  | "tools"
  | "diffs"
  | "errors"
  | "approvals"
  | "cost";

export interface FilterOption {
  id: TranscriptFilter;
  label: string;
  title: string;
}

export const FILTERS: FilterOption[] = [
  { id: "all", label: "all", title: "Every recorded row" },
  { id: "yours", label: "yours", title: "Role: your prompts and notes" },
  { id: "model", label: "model", title: "Role: model reasoning and answers" },
  { id: "tools", label: "tools", title: "Tool calls: commands, MCP and web search" },
  { id: "diffs", label: "diffs", title: "File changes" },
  { id: "errors", label: "errors", title: "Errors and failed states" },
  { id: "approvals", label: "approvals", title: "Approval questions and decisions" },
  { id: "cost", label: "cost", title: "Usage and billed cost rows" },
];

const TOOL_KINDS = new Set(["command", "mcp_call", "web_search"]);
const MODEL_KINDS = new Set(["reasoning"]);
const APPROVAL_KINDS = new Set(["approval", "plan"]);
const BAD_STATES = new Set([
  "error",
  "failed",
  "cancelled",
  "denied",
  "timeout",
  "timed_out",
  "interrupted",
]);

function bodyText(item: Item, key: string): string {
  const value = item.body[key];
  return typeof value === "string" ? value : "";
}

export function isErrorItem(item: Item): boolean {
  return item.kind === "error" || BAD_STATES.has(item.status);
}

/** A model row is reasoning or an assistant message; your own message belongs to `yours`. */
function isModelItem(item: Item): boolean {
  return MODEL_KINDS.has(item.kind) || (item.kind === "message" && bodyText(item, "role") !== "user");
}

/** The role filter separates your words from the model's, using the recorded message role. */
export function matchesFilter(item: Item, filter: TranscriptFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "yours":
      return item.kind === "note" || (item.kind === "message" && bodyText(item, "role") === "user");
    case "model":
      return isModelItem(item);
    case "tools":
      return TOOL_KINDS.has(item.kind);
    case "diffs":
      return item.kind === "file_change";
    case "errors":
      return isErrorItem(item);
    case "approvals":
      return APPROVAL_KINDS.has(item.kind);
    case "cost":
      return item.kind === "usage";
  }
}

/** A prompt is yours, and it is the one line that is not an item. */
export function filterShowsPrompt(filter: TranscriptFilter): boolean {
  return filter === "all" || filter === "yours";
}

export function matchesText(text: string, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  return needle.length === 0 || text.toLocaleLowerCase().includes(needle);
}

/** Search reads the recorded body, so it sees paths, tool names, errors and model text alike. */
export function itemSearchText(item: Item): string {
  return [
    item.kind,
    item.status,
    ...Object.values(item.body).map((value) =>
      typeof value === "string" ? value : JSON.stringify(value),
    ),
  ]
    .join(" ")
    .toLocaleLowerCase();
}

export function visibleItems(
  entry: TurnTranscript,
  filter: TranscriptFilter,
  query = "",
): Item[] {
  const needle = query.trim().toLocaleLowerCase();
  return entry.items.filter(
    (item) =>
      matchesFilter(item, filter) &&
      (needle.length === 0 || itemSearchText(item).includes(needle)),
  );
}

/** Prompt and item lines the current filter and search keep out of the transcript. */
export function hiddenCount(
  entry: TurnTranscript,
  filter: TranscriptFilter,
  query = "",
): number {
  const promptHidden = entry.turn.prompt
    ? !filterShowsPrompt(filter) || !matchesText(entry.turn.prompt, query)
      ? 1
      : 0
    : 0;
  return entry.items.length - visibleItems(entry, filter, query).length + promptHidden;
}

export type OutlineMarkKind = "prompt" | "diff" | "error" | "approval" | "model";

export interface OutlineMark {
  key: string;
  kind: OutlineMarkKind;
  title: string;
}

export interface OutlineGroup {
  turnId: string;
  seq: number;
  marks: OutlineMark[];
}

/** The outline is built from the visible rows only: it cannot point at a hidden line. */
export function outlineGroups(
  entries: TurnTranscript[],
  filter: TranscriptFilter,
  query = "",
): OutlineGroup[] {
  const groups: OutlineGroup[] = [];
  for (const entry of entries) {
    const marks: OutlineMark[] = [];
    if (entry.turn.prompt && filterShowsPrompt(filter) && matchesText(entry.turn.prompt, query)) {
      marks.push({
        key: `prompt-${entry.turn.id}`,
        kind: "prompt",
        title: `turn ${entry.turn.seq} · your prompt: ${entry.turn.prompt.slice(0, 90)}`,
      });
    }
    for (const item of visibleItems(entry, filter, query)) {
      if (item.kind === "file_change") {
        const result = item.body.result as { path?: unknown } | undefined;
        const path =
          typeof result?.path === "string" ? result.path : bodyText(item, "path");
        marks.push({
          key: `diff-${item.id}`,
          kind: "diff",
          title: `turn ${entry.turn.seq} · patch ${path}`.trim(),
        });
      } else if (isErrorItem(item)) {
        const message = bodyText(item, "message") || `${item.kind} ${item.status}`;
        marks.push({
          key: `error-${item.id}`,
          kind: "error",
          title: `turn ${entry.turn.seq} · error: ${message.slice(0, 80)}`,
        });
      } else if (APPROVAL_KINDS.has(item.kind)) {
        marks.push({
          key: `approval-${item.id}`,
          kind: "approval",
          title: `turn ${entry.turn.seq} · approval ${bodyText(item, "kind") || item.status}`,
        });
      } else if (isModelItem(item)) {
        marks.push({
          key: `model-${item.id}`,
          kind: "model",
          title: `turn ${entry.turn.seq} · ${item.kind}`,
        });
      }
    }
    if (marks.length > 0) {
      groups.push({ turnId: entry.turn.id, seq: entry.turn.seq, marks });
    }
  }
  return groups;
}

/** Counts for the chips, over all rows regardless of the current view. */
export function filterCounts(entries: TurnTranscript[]): Record<TranscriptFilter, number> {
  const counts: Record<TranscriptFilter, number> = {
    all: 0,
    yours: 0,
    model: 0,
    tools: 0,
    diffs: 0,
    errors: 0,
    approvals: 0,
    cost: 0,
  };
  for (const entry of entries) {
    if (entry.turn.prompt) {
      counts.all += 1;
      counts.yours += 1;
    }
    for (const item of entry.items) {
      counts.all += 1;
      for (const option of FILTERS) {
        if (option.id !== "all" && matchesFilter(item, option.id)) counts[option.id] += 1;
      }
    }
  }
  return counts;
}

export interface FilterStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export const filterKey = (project: string, session: string) =>
  `rigel.transcriptFilter.${project}::${session}`;

function storageOrNull(storage?: FilterStorage): FilterStorage | null {
  if (storage) return storage;
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readFilter(
  project: string,
  session: string,
  storage?: FilterStorage,
): TranscriptFilter {
  const store = storageOrNull(storage);
  const saved = store?.getItem(filterKey(project, session));
  return FILTERS.some((row) => row.id === saved) ? (saved as TranscriptFilter) : "all";
}

export function writeFilter(
  project: string,
  session: string,
  filter: TranscriptFilter,
  storage?: FilterStorage,
): void {
  try {
    storageOrNull(storage)?.setItem(filterKey(project, session), filter);
  } catch {
    // A blocked store forgets the choice; the transcript still filters for this run.
  }
}
