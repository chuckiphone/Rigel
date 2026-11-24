/**
 * The Audit log's address (roadmap 83).
 *
 * The filter is the page's state and its URL is the same object: opening `#settings/audit?...`
 * restores the view, and every edit replaces the hash so a copied address carries the rows that
 * were on screen. Pure functions so the round-trip is testable without a window or a Tauri host.
 */

import type { AuditFilter } from "./api";

export const EMPTY_AUDIT_FILTER: AuditFilter = {
  project: "",
  q: "",
  session: "",
  agent: "",
  turn: "",
  decision: "",
};

const KEYS: (keyof AuditFilter)[] = ["project", "q", "session", "agent", "turn", "decision"];

/** Read only the six audit keys from a settings hash; anything else is ignored. */
export function auditFilterFromHash(hash: string): AuditFilter {
  const query = hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "";
  const params = new URLSearchParams(query);
  const filter = { ...EMPTY_AUDIT_FILTER };
  for (const key of KEYS) filter[key] = params.get(key) ?? "";
  return filter;
}

/** Build the hash the page owns. Empty fields stay out so the address stays readable. */
export function auditHash(filter: AuditFilter): string {
  const params = new URLSearchParams();
  for (const key of KEYS) {
    const value = filter[key].trim();
    if (value) params.set(key, value);
  }
  const query = params.toString();
  return `#settings/audit${query ? `?${query}` : ""}`;
}

export function auditFilterIsEmpty(filter: AuditFilter): boolean {
  return KEYS.every((key) => !filter[key].trim());
}