/**
 * Roadmap 132: a review screen for recorded patch hunks.
 *
 * The same hunk can be read in unified form (one column of signs) or split form (old beside new).
 * Each hunk carries its own provenance rail — who wrote it, in which patch and file — and can be
 * accepted alone. The final verdict names the reviewer and is stored with the turn, not just held
 * in the window.
 */

import { useState, type ReactNode } from "react";

import type { AttributedHunk, Item } from "./api";

export type ReviewMode = "split" | "unified";

/** The two sides of one hunk, reconstructed from the hunk header for display. */
export function hunkSides(hunk: AttributedHunk): { oldLines: string[]; newLines: string[] } {
  const oldLines = Array.from({ length: Math.min(hunk.old_lines, 8) }, (_, index) =>
    `- ${hunk.path}:${hunk.old_start + index}`,
  );
  const newLines = Array.from({ length: Math.min(hunk.new_lines, 8) }, (_, index) =>
    `+ ${hunk.path}:${hunk.new_start + index}`,
  );
  return {
    oldLines: oldLines.length ? oldLines : ["- (no old lines)"],
    newLines: newLines.length ? newLines : ["+ (no new lines)"],
  };
}

/** One line of provenance per hunk: the agent, the patch, the file and the recorded status. */
export function provenance(hunk: AttributedHunk): string[] {
  return [
    `author ${hunk.agent_id}`,
    `patch ${hunk.patch_id.slice(0, 8)} · hunk ${hunk.ordinal}`,
    `${hunk.path} · ${hunk.status}`,
  ];
}

export default function DiffReview({
  hunks,
  turnId,
  reviewer,
  mode: initialMode = "unified",
  onVerdict,
}: {
  hunks: readonly AttributedHunk[];
  turnId: string;
  reviewer: string;
  mode?: ReviewMode;
  onVerdict: (
    accepted: string[],
    reviewer: string,
  ) => Promise<Item>;
}): ReactNode {
  const [mode, setMode] = useState<ReviewMode>(initialMode);
  const [accepted, setAccepted] = useState<string[]>([]);
  const [name, setName] = useState(reviewer);
  const [error, setError] = useState<string | null>(null);
  const [stored, setStored] = useState<Item | null>(null);

  const acceptAlone = (id: string) => setAccepted([id]);
  const toggle = (id: string) =>
    setAccepted((current) =>
      current.includes(id) ? current.filter((candidate) => candidate !== id) : [...current, id],
    );

  const save = async () => {
    setError(null);
    try {
      setStored(
        await onVerdict(accepted, name),
      );
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  };

  return (
    <section className="diff-review" aria-label="Diff review">
      <header className="diff-review-head">
        <strong>Diff review</strong>
        <div className="diff-modes" role="group" aria-label="Diff mode">
          {(["unified", "split"] as const).map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={mode === option}
              onClick={() => setMode(option)}
            >
              {option === "unified" ? "Unified" : "Split"}
            </button>
          ))}
        </div>
        <span className="fine">{hunks.length} hunk(s)</span>
      </header>

      {hunks.length === 0 ? (
        <p className="fine">No recorded hunks in this session yet.</p>
      ) : null}

      <ol className="review-hunks" aria-label="Recorded hunks">
        {hunks.map((hunk, index) => {
          const sides = hunkSides(hunk);
          const isAccepted = accepted.includes(hunk.id);
          return (
            <li className={`review-hunk${isAccepted ? " accepted" : ""}`} key={hunk.id} data-hunk={hunk.id}>
              <aside className="provenance-rail" aria-label={`Provenance for hunk ${index + 1}`}>
                <strong>{hunk.header}</strong>
                {provenance(hunk).map((line) => (
                  <span key={line}>{line}</span>
                ))}
              </aside>
              <div className="review-body">
                {mode === "split" ? (
                  <div className="review-split" aria-label={`Split view for hunk ${index + 1}`}>
                    <pre className="review-old">{sides.oldLines.join("\n")}</pre>
                    <pre className="review-new">{sides.newLines.join("\n")}</pre>
                  </div>
                ) : (
                  <pre
                    className="review-unified"
                    aria-label={`Unified view for hunk ${index + 1}`}
                  >
                    {[...sides.oldLines, ...sides.newLines].join("\n")}
                  </pre>
                )}
                <div className="review-actions">
                  <button type="button" onClick={() => toggle(hunk.id)} aria-pressed={isAccepted}>
                    {isAccepted ? "Accepted" : "Accept"} hunk {index + 1}
                  </button>
                  <button type="button" onClick={() => acceptAlone(hunk.id)}>
                    Accept hunk {index + 1} alone
                  </button>
                </div>
              </div>
            </li>
          );
        })}
      </ol>

      <footer className="review-verdict">
        <label>
          Reviewer
          <input
            aria-label="Reviewer name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="your name or account"
          />
        </label>
        <button
          type="button"
          disabled={!name.trim() || hunks.length === 0}
          onClick={() => void save()}
        >
          Save verdict with turn
        </button>
        <span className="fine">
          {accepted.length} of {hunks.length} hunk(s) accepted · turn {turnId.slice(0, 8)}
        </span>
      </footer>
      {error ? (
        <p className="review-error" role="alert">
          {error}
        </p>
      ) : null}
      {stored ? (
        <p className="review-stored" role="status">
          Verdict stored with the turn by {String(stored.body.reviewer)}.
        </p>
      ) : null}
    </section>
  );
}
