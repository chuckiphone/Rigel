import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api, type PaletteResults } from "./api";
import {
  paletteEntries,
  sectionRanges,
  type PaletteAction,
  type PaletteEntry as Entry,
} from "./palette";

/**
 * Roadmap 113: the palette component. Data and section rules live in `palette.ts`; this file owns
 * the dialog, the keyboard path and the callbacks back into the shell.
 *
 * Keyboard is the primary path: ↑/↓ move, Enter opens, Escape closes. Rows are buttons so a
 * pointer works too, and the active row is scrolled into view rather than the list jumping.
 */

interface Props {
  open: boolean;
  project: string | null;
  currentSession: string | null;
  /** The binding that opens this palette, shown in the footer while it is open. */
  shortcut: string;
  /** The current binding for focus mode, shown beside its palette action. */
  focusShortcut: string;
  onClose: () => void;
  onAction: (action: PaletteAction) => void;
  onSkill: (id: string) => void;
  onSession: (id: string) => void;
  onTurn: (sessionId: string, turnId: string) => void;
  onPatch: (sessionId: string, patchId: string) => void;
}

const EMPTY: PaletteResults = {
  skills: [],
  sessions: [],
  turns: [],
  patches: [],
  files: [],
  symbols: [],
};

export default function Palette({
  open,
  project,
  currentSession,
  shortcut,
  focusShortcut,
  onClose,
  onAction,
  onSkill,
  onSession,
  onTurn,
  onPatch,
}: Props) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<PaletteResults>(EMPTY);
  const [loading, setLoading] = useState(false);
  const [active, setActive] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const restoreFocus = useRef<HTMLElement | null>(null);

  // Opening resets the query; closing hands focus back to whatever had it. The palette is a
  // layer, not a route, so it must not leave the keyboard stranded in a hidden input.
  useEffect(() => {
    if (!open) return;
    restoreFocus.current = document.activeElement as HTMLElement | null;
    setQuery("");
    setResults(EMPTY);
    setActive(0);
    setError(null);
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => {
      window.clearTimeout(timer);
      restoreFocus.current?.focus?.();
    };
  }, [open]);

  // The search box autofocuses a frame after the dialog mounts. Escape must close the palette
  // even in that gap, so the binding lives on the document rather than only on the input.
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, onClose]);

  // One debounced read. A dead project database or a missing skill index leaves the other
  // sections usable: the command answers with empty groups rather than failing the palette.
  useEffect(() => {
    if (!open) return;
    let live = true;
    setLoading(true);
    // Results for the previous query must not be Enter-able under the new one.
    setResults(EMPTY);
    const timer = window.setTimeout(() => {
      api
        .searchAll(project, query, currentSession)
        .then((next) => {
          if (!live) return;
          setResults(next);
          setError(null);
          setActive(0);
        })
        .catch((failure) => {
          if (!live) return;
          setResults(EMPTY);
          setError(failure instanceof Error ? failure.message : String(failure));
        })
        .finally(() => live && setLoading(false));
    }, 140);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [open, project, query, currentSession]);

  const entries = useMemo<Entry[]>(
    () => paletteEntries(results, query, focusShortcut),
    [query, results, focusShortcut],
  );

  const run = useCallback(
    (entry: Entry | undefined) => {
      if (!entry) return;
      onClose();
      if (entry.kind === "action" && entry.action) onAction(entry.action);
      else if (entry.kind === "setting") onAction("settings");
      else if (entry.kind === "file" || entry.kind === "symbol") onAction("diff");
      else if (entry.kind === "skill" && entry.id) onSkill(entry.id);
      else if (entry.kind === "session" && entry.id) onSession(entry.id);
      else if (entry.kind === "turn" && entry.sessionId && entry.turnId)
        onTurn(entry.sessionId, entry.turnId);
      else if (entry.kind === "patch" && entry.sessionId && entry.patchId)
        onPatch(entry.sessionId, entry.patchId);
    },
    [onAction, onClose, onPatch, onSession, onSkill, onTurn],
  );

  const move = useCallback(
    (delta: number) => {
      setActive((current) => {
        if (entries.length === 0) return 0;
        return (current + delta + entries.length) % entries.length;
      });
    },
    [entries.length],
  );

  // The active row follows the keyboard even when the list is longer than the panel.
  useEffect(() => {
    document.getElementById(`palette-row-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [active, entries]);

  if (!open) return null;

  const sections = sectionRanges(entries);

  return (
    <div
      className="palette-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="palette"
        role="dialog"
        aria-modal="true"
        aria-label="Search actions, skills, sessions, turns and patches"
      >
        <div className="palette-input-row">
          <span aria-hidden className="palette-glyph">
            ⌕
          </span>
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                onClose();
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                move(1);
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                move(-1);
              } else if (event.key === "Enter") {
                event.preventDefault();
                run(entries[active]);
              }
            }}
            placeholder="Search actions, skills, sessions, turns, patches…"
            aria-label="Search Rigel"
            aria-controls="palette-list"
            aria-activedescendant={entries.length > 0 ? `palette-row-${active}` : undefined}
            autoComplete="off"
            spellCheck={false}
          />
          <kbd>esc</kbd>
        </div>

        <div className="palette-list" id="palette-list" role="listbox" aria-label="Results">
          {sections.map((section) => (
            <div key={section.label} className="palette-section-block">
              <p className="palette-section" role="presentation">
                {section.label}
              </p>
              {entries.slice(section.from, section.to + 1).map((entry, offset) => {
                const index = section.from + offset;
                return (
                  <button
                    key={entry.key}
                    id={`palette-row-${index}`}
                    type="button"
                    role="option"
                    aria-selected={index === active}
                    className={`palette-row${index === active ? " on" : ""}`}
                    onMouseEnter={() => setActive(index)}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      run(entry);
                    }}
                  >
                    <span className="palette-row-text">
                      <span className="palette-row-title">{entry.title}</span>
                      {entry.subtitle ? (
                        <span className="palette-row-subtitle">{entry.subtitle}</span>
                      ) : null}
                    </span>
                    <span className="palette-row-meta mono">{entry.meta}</span>
                  </button>
                );
              })}
            </div>
          ))}

          {entries.length === 0 && !loading ? (
            <p className="palette-empty">
              {error
                ? error
                : query
                  ? `Nothing matches “${query}”.`
                  : "No sessions or patches in this project yet."}
            </p>
          ) : null}
          {loading ? <p className="palette-empty subtle">Searching…</p> : null}
        </div>

        <footer className="palette-foot">
          <span>↑↓ move</span>
          <span>↵ open</span>
          <span>esc close</span>
          <span className="grow" />
          <span>{shortcut}</span>
          <span>{project ? "this project + local skills" : "local skills"}</span>
        </footer>
      </div>
    </div>
  );
}
