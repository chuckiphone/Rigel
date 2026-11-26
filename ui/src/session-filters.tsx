/**
 * Roadmap 102: colourless tags and saved session filters.
 *
 * The active filter is the page's address: `#sessions?query=…&status=…&archived=1&tag=…` restores
 * the same rows after a reload or a pasted link. Saved filters live in localStorage, so they
 * survive a restart. A tag is text, never a colour: an unrecognised tag is shown as plain text
 * instead of being rejected, because the window must not error on an address it does not know.
 */

import { useState } from "react";
import type { Session } from "./api";

export interface SessionFilter {
  query: string;
  status: "all" | Session["status"];
  /** Include archived rows; the default hides them. */
  archived: boolean;
  tags: string[];
}

export const EMPTY_SESSION_FILTER: SessionFilter = {
  query: "",
  status: "all",
  archived: false,
  tags: [],
};

export const KNOWN_TAGS = ["active", "idle", "interrupted", "ended", "archived"] as const;
const STATUSES = new Set<string>(KNOWN_TAGS);

/** Read only the session keys from a hash; unknown tags are kept verbatim as plain text. */
export function sessionFilterFromHash(hash: string): SessionFilter {
  const query = hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "";
  const params = new URLSearchParams(query);
  const status = params.get("status") ?? "all";
  return {
    query: params.get("query") ?? "",
    status: STATUSES.has(status) ? (status as SessionFilter["status"]) : "all",
    archived: params.get("archived") === "1",
    tags: params.getAll("tag").filter((tag) => tag.length > 0),
  };
}

/** Build the hash the roster owns. Empty fields stay out so a copied address stays readable. */
export function sessionHash(filter: SessionFilter): string {
  const params = new URLSearchParams();
  const query = filter.query.trim();
  if (query) params.set("query", query);
  if (filter.status !== "all") params.set("status", filter.status);
  if (filter.archived) params.set("archived", "1");
  for (const tag of filter.tags) if (tag.trim()) params.append("tag", tag.trim());
  const suffix = params.toString();
  return `#sessions${suffix ? `?${suffix}` : ""}`;
}

export function sessionFilterIsEmpty(filter: SessionFilter): boolean {
  return !filter.query.trim() && filter.status === "all" && !filter.archived && filter.tags.length === 0;
}

export interface SavedFilter {
  id: string;
  name: string;
  filter: SessionFilter;
}

const STORE_KEY = "rigel.session.saved-filters";

/** Saved filters are local preferences, never project data and never sent anywhere. */
export function loadSavedFilters(): SavedFilter[] {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    const rows: unknown = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(rows)) return [];
    return rows.filter(
      (row): row is SavedFilter =>
        typeof row?.id === "string" && typeof row?.name === "string" && row?.filter !== null,
    );
  } catch {
    return [];
  }
}

export function saveFilter(name: string, filter: SessionFilter): SavedFilter[] {
  const rows = loadSavedFilters();
  const next = [
    ...rows.filter((row) => row.name !== name),
    { id: `${Date.now().toString(36)}-${name}`, name, filter: { ...filter, tags: [...filter.tags] } },
  ];
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(next));
  } catch {
    // A window with storage disabled still keeps the filter for this session.
  }
  return next;
}

export default function SessionFilters({
  filter,
  onChange,
}: {
  filter: SessionFilter;
  onChange: (next: SessionFilter) => void;
}) {
  const [tagText, setTagText] = useState("");
  const [name, setName] = useState("");
  const [saved, setSaved] = useState<SavedFilter[]>(() => loadSavedFilters());

  return (
    <section className="session-filters-block" aria-label="Tags and saved filters">
      <form
        className="filter-add"
        onSubmit={(event) => {
          event.preventDefault();
          const tag = tagText.trim();
          if (tag && !filter.tags.includes(tag)) onChange({ ...filter, tags: [...filter.tags, tag] });
          setTagText("");
        }}
      >
        <input
          aria-label="Add a tag"
          value={tagText}
          onChange={(event) => setTagText(event.target.value)}
          placeholder="Add a tag"
        />
        <button type="submit" className="chip">
          Add tag
        </button>
      </form>
      <div className="filter-tags" role="group" aria-label="Current filter tags">
        {filter.tags.length === 0 ? (
          <p className="fine">No tags on this filter.</p>
        ) : (
          filter.tags.map((tag) => (
            <button
              key={tag}
              type="button"
              className={`filter-tag${KNOWN_TAGS.includes(tag as (typeof KNOWN_TAGS)[number]) ? " known" : ""}`}
              aria-label={`Remove tag ${tag}`}
              title={KNOWN_TAGS.includes(tag as (typeof KNOWN_TAGS)[number]) ? "Known tag" : "Plain text tag"}
              onClick={() => onChange({ ...filter, tags: filter.tags.filter((item) => item !== tag) })}
            >
              {tag}
            </button>
          ))
        )}
      </div>
      <form
        className="filter-add"
        onSubmit={(event) => {
          event.preventDefault();
          const label = name.trim();
          if (label) setSaved(saveFilter(label, filter));
          setName("");
        }}
      >
        <input
          aria-label="Filter name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="Name this filter"
        />
        <button type="submit" className="chip">
          Save filter
        </button>
      </form>
      <div className="filter-saved" role="group" aria-label="Saved filters">
        {saved.length === 0 ? (
          <p className="fine">No saved filters yet.</p>
        ) : (
          saved.map((row) => (
            <button
              key={row.id}
              type="button"
              className="filter-tag"
              aria-label={`Apply saved filter ${row.name}`}
              onClick={() => onChange({ ...row.filter })}
            >
              {row.name}
            </button>
          ))
        )}
      </div>
    </section>
  );
}
