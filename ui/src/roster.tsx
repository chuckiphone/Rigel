/**
 * Roadmap 131: the session roster.
 *
 * Sessions are grouped by project and status, but the list is virtualised: a 500-session project
 * keeps one honest scrollbar and mounts only the visible window (plus overscan). Every mutation
 * lives in one row menu, opened by a native button that is reachable with Tab and whose items can
 * be walked with the arrow keys.
 */

import { useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";

import type { Session } from "./api";
import ForkLineage from "./fork-lineage";
import VirtualTranscript, { virtualWindow, type VirtualWindow } from "./transcript/virtual";

export const ROSTER_ROW_HEIGHT = 40;

/** The same windowing maths the transcript uses, exposed for the roster's own tests. */
export function rosterWindow(
  count: number,
  scrollTop: number,
  viewport: number,
  overscan = 4,
): VirtualWindow {
  return virtualWindow(count, scrollTop, viewport, ROSTER_ROW_HEIGHT, overscan);
}

export type RosterEntry =
  | { kind: "header"; key: string; label: string; count: number }
  | { kind: "session"; key: string; session: Session };

const STATUS_ORDER = ["active", "idle", "interrupted", "ended", "failed", "cancelled"];

/** Group by project, then by status, preserving each group's incoming order. */
export function groupSessions(sessions: readonly Session[]): RosterEntry[] {
  const projects = new Map<string, Map<string, Session[]>>();
  for (const session of sessions) {
    const project = session.project_root.split(/[\\/]/).filter(Boolean).pop() || "project";
    const status = session.archived_at !== null ? "archived" : session.status;
    const byStatus = projects.get(project) ?? new Map<string, Session[]>();
    const rows = byStatus.get(status) ?? [];
    rows.push(session);
    byStatus.set(status, rows);
    projects.set(project, byStatus);
  }
  const entries: RosterEntry[] = [];
  for (const [project, byStatus] of projects) {
    const statuses = [
      ...STATUS_ORDER.filter((status) => byStatus.has(status)),
      ...[...byStatus.keys()].filter((status) => !STATUS_ORDER.includes(status)).sort(),
    ];
    entries.push({
      kind: "header",
      key: `project:${project}`,
      label: project,
      count: [...byStatus.values()].reduce((sum, rows) => sum + rows.length, 0),
    });
    for (const status of statuses) {
      const rows = byStatus.get(status)!;
      entries.push({
        kind: "header",
        key: `status:${project}:${status}`,
        label: status,
        count: rows.length,
      });
      for (const session of rows) {
        entries.push({ kind: "session", key: session.id, session });
      }
    }
  }
  return entries;
}

/** A deterministic 500-session fixture for the virtualisation test. */
export function rosterFixture(count = 500, projectRoot = "C:/Rigel/fixture"): Session[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `fixture-session-${String(index + 1).padStart(4, "0")}`,
    project_root: projectRoot,
    title: `Fixture session ${String(index + 1).padStart(4, "0")}`,
    title_source: null,
    status: (["active", "idle", "ended", "interrupted"] as const)[index % 4],
    budget_cents: null,
    spend_cents: 0,
    spend_micros: index * 137,
    interventions: 0,
    tasks_done: 0,
    created_at: 1_700_000_000_000 + index * 60_000,
    ended_at: null,
    pinned_at: null,
    archived_at: null,
  }));
}

export interface RosterActions {
  onOpen: (session: Session) => void;
  onRename: (session: Session) => void;
  onPin: (session: Session) => void;
  onArchive: (session: Session) => void;
  onDelete: (session: Session) => void;
  /** The existing context menu stays available; the row menu is its keyboard equivalent. */
  onContextMenu?: (event: MouseEvent, session: Session) => void;
}

/** One native button, one menu. Keyboard users never need the context menu. */
export function RosterRowMenu({
  session,
  actions,
  pinned,
  archived,
  busy,
}: {
  session: Session;
  actions: RosterActions;
  pinned: boolean;
  archived: boolean;
  busy: boolean;
}): ReactNode {
  const [open, setOpen] = useState(false);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  // Opening from the keyboard moves focus into the menu; a pointer click leaves focus alone.
  useEffect(() => {
    if (open) items.current[0]?.focus();
  }, [open]);

  const title = session.title ?? "session";
  const commands: { label: string; disabled?: boolean; run: () => void }[] = [
    { label: "Open session", run: () => actions.onOpen(session) },
    {
      label: pinned ? "Unpin from top" : "Pin to the top",
      disabled: busy,
      run: () => actions.onPin(session),
    },
    { label: `Rename ${title}`, run: () => actions.onRename(session) },
    {
      label: archived ? "Restore session" : "Archive session",
      disabled: busy,
      run: () => actions.onArchive(session),
    },
    {
      label: "Delete permanently…",
      disabled: busy || !archived,
      run: () => actions.onDelete(session),
    },
  ];

  return (
    <div className="roster-menu" onBlur={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
    }}>
      <button
        type="button"
        className="roster-menu-trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Actions for ${session.title ?? "session"}`}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        ⋯
      </button>
      {open ? (
        <div
          className="roster-menu-items"
          role="menu"
          aria-label={`Actions for ${session.title ?? "session"}`}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              setOpen(false);
              return;
            }
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            event.preventDefault();
            const current = items.current.findIndex((item) => item === document.activeElement);
            const next =
              event.key === "ArrowDown"
                ? Math.min(items.current.length - 1, current + 1)
                : Math.max(0, current - 1);
            items.current[next]?.focus();
          }}
        >
          {commands.map((command, index) => (
            <button
              key={command.label}
              ref={(element) => {
                items.current[index] = element;
              }}
              type="button"
              role="menuitem"
              disabled={command.disabled}
              onClick={() => {
                setOpen(false);
                command.run();
              }}
            >
              {command.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

export default function SessionRoster({
  sessions,
  selectedId,
  renamingId,
  renameText,
  actions,
  onRenameText,
  onCommitRename,
  onCancelRename,
  busyId,
}: {
  sessions: readonly Session[];
  selectedId: string | null;
  renamingId: string | null;
  renameText: string;
  actions: RosterActions;
  onRenameText: (text: string) => void;
  onCommitRename: (session: Session, text: string) => void;
  onCancelRename: () => void;
  busyId: string | null;
}): ReactNode {
  const entries = groupSessions(sessions);
  return (
    <VirtualTranscript
      className="roster-virtual"
      label="Session roster"
      rows={entries}
      rowHeight={ROSTER_ROW_HEIGHT}
      getKey={(entry) => entry.key}
      renderRow={(entry) =>
        entry.kind === "header" ? (
          <div className="roster-group" data-group={entry.label}>
            <strong>{entry.label}</strong>
            <span className="fine">{entry.count}</span>
          </div>
        ) : (
          <div
            className={`session-row${selectedId === entry.session.id ? " on" : ""}`}
            data-session={entry.session.id}
            onContextMenu={(event) => actions.onContextMenu?.(event, entry.session)}
          >
            {renamingId === entry.session.id ? (
              <input
                className="session-rename"
                autoFocus
                aria-label={`Rename ${entry.session.title ?? "session"}`}
                value={renameText}
                onChange={(event) => onRenameText(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    event.currentTarget.blur();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    onCancelRename();
                  }
                }}
                onBlur={() => onCommitRename(entry.session, renameText)}
              />
            ) : (
              <button
                type="button"
                className="session-open"
                onClick={() => actions.onOpen(entry.session)}
                title={`${entry.session.title ?? "Untitled session"} · ${entry.session.status}`}
              >
                <span className="row-text">
                  <span className="row-name">
                    {entry.session.pinned_at !== null ? <span aria-hidden>★</span> : null}
                    <span className="session-name">
                      {entry.session.title ?? "Untitled session"}
                    </span>
                    <ForkLineage
                      parentSessionId={entry.session.parent_session_id}
                      parentTurnId={entry.session.parent_turn_id}
                    />
                    {entry.session.archived_at !== null ? <span className="tag">archived</span> : null}
                  </span>
                  <span className="row-meta session-meta">
                    <span>{entry.session.status}</span>
                  </span>
                </span>
              </button>
            )}
            {renamingId !== entry.session.id ? (
              <div className="session-row-actions">
                <button
                  type="button"
                  className="session-act"
                  aria-pressed={entry.session.pinned_at !== null}
                  aria-label={entry.session.pinned_at !== null ? "★" : "☆"}
                  disabled={busyId === entry.session.id}
                  onClick={() => actions.onPin(entry.session)}
                >
                  {entry.session.pinned_at !== null ? "★" : "☆"}
                </button>
                <button
                  type="button"
                  className="session-act"
                  aria-label={`Rename ${entry.session.title ?? "session"}`}
                  onClick={() => actions.onRename(entry.session)}
                >
                  ✎
                </button>
                <button
                  type="button"
                  className="session-act"
                  aria-label={
                    entry.session.archived_at !== null ? "Restore session" : "Archive session"
                  }
                  disabled={busyId === entry.session.id}
                  onClick={() => actions.onArchive(entry.session)}
                >
                  {entry.session.archived_at !== null ? "restore" : "archive"}
                </button>
                <RosterRowMenu
                  session={entry.session}
                  actions={actions}
                  pinned={entry.session.pinned_at !== null}
                  archived={entry.session.archived_at !== null}
                  busy={busyId === entry.session.id}
                />
              </div>
            ) : null}
          </div>
        )
      }
    />
  );
}
