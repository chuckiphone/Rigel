/**
 * The guided three-way merge (roadmap 45).
 *
 * A conflict is a question with one answer per hunk. This editor shows the base, ours and
 * theirs for every block the line merge could not settle, and requires a named reviewer to
 * choose each one before the parent can write the file. It deliberately has no "accept all":
 * the marker text itself is not a resolution, and the core refuses to write it.
 *
 * The test hint is a path-derived command, not a claim that the command ran. The verdict stores
 * the hint and the reviewer's acknowledgement beside the exact merged hash.
 */

import type { MergedConflict } from "./api";

const OURS_MARKER = "<<<<<<< ours";
const SEPARATOR = "=======";
const THEIRS_MARKER = ">>>>>>> theirs";

export type HunkChoice = "ours" | "theirs" | "edited";

export interface HunkDecision {
  choice: HunkChoice;
  /** Only meaningful for `edited`; the exact text the reviewer wants in this block. */
  text?: string;
}

/** What the editor collects before the parent may write the merge. */
export interface ConflictReviewState {
  reviewer: string;
  testsAcknowledged: boolean;
  decisions: Record<number, HunkDecision>;
}

export interface ConflictTextPart {
  kind: "text";
  text: string;
}

export interface ConflictHunkPart {
  kind: "conflict";
  index: number;
  ours: string;
  theirs: string;
}

export type ConflictPart = ConflictTextPart | ConflictHunkPart;

export interface ParsedMerge {
  parts: ConflictPart[];
  conflicts: number;
  error: string | null;
}

export function emptyConflictReview(): ConflictReviewState {
  return { reviewer: "", testsAcknowledged: false, decisions: {} };
}

/** Split a merged file into unchanged text and marker blocks, keeping every line ending. */
export function parseMergeText(merged: string): ParsedMerge {
  const lines = merged.match(/[^\n]*\n|[^\n]+$/g) ?? [];
  const parts: ConflictPart[] = [];
  let text = "";
  let side: "ours" | "theirs" | null = null;
  let ours = "";
  let theirs = "";
  let index = 0;

  for (const line of lines) {
    const bare = line.replace(/\r?\n$/, "");
    if (side === null) {
      if (bare === OURS_MARKER) {
        if (text) parts.push({ kind: "text", text });
        text = "";
        side = "ours";
        ours = "";
        theirs = "";
      } else {
        text += line;
      }
      continue;
    }
    if (bare === SEPARATOR) {
      side = "theirs";
    } else if (bare === THEIRS_MARKER) {
      parts.push({ kind: "conflict", index, ours, theirs });
      index += 1;
      side = null;
    } else if (side === "ours") {
      ours += line;
    } else {
      theirs += line;
    }
  }

  if (side !== null) {
    return {
      parts: [],
      conflicts: 0,
      error: "This merge has an opening conflict marker without a closing one; Rigel will not write it.",
    };
  }
  if (text) parts.push({ kind: "text", text });
  return { parts, conflicts: index, error: null };
}

export function hasConflictMarkers(text: string): boolean {
  return text.split(/\r?\n/).some((line) =>
    line === OURS_MARKER || line === SEPARATOR || line === THEIRS_MARKER,
  );
}

function newlineFor(text: string): string {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

function chosenText(
  part: ConflictHunkPart,
  decision: HunkDecision | undefined,
  newline: string,
  followedByText: boolean,
): string {
  if (!decision) return "";
  if (decision.choice === "ours") return part.ours;
  if (decision.choice === "theirs") return part.theirs;
  let text = decision.text ?? "";
  if (text && !text.endsWith("\n") && followedByText) text += newline;
  return text;
}

/** Build the file the reviewer is about to write, with one chosen hunk at a time. */
export function resolvedConflictText(
  merge: MergedConflict,
  review: ConflictReviewState,
): string {
  const parsed = parseMergeText(merge.merged);
  if (parsed.error) return "";
  const newline = newlineFor(merge.merged);
  return parsed.parts
    .map((part, position) => {
      if (part.kind === "text") return part.text;
      return chosenText(
        part,
        review.decisions[part.index],
        newline,
        position < parsed.parts.length - 1,
      );
    })
    .join("");
}

/** Why the write is still disabled, or `null` when every gate is satisfied. */
export function conflictReviewBlocker(
  merge: MergedConflict,
  review: ConflictReviewState,
): string | null {
  const parsed = parseMergeText(merge.merged);
  if (parsed.error) return parsed.error;
  if (parsed.conflicts !== merge.conflicts || merge.hunks.length !== merge.conflicts) {
    return "The merge preview and its hunk list disagree; reload the conflict before writing.";
  }
  for (const part of parsed.parts) {
    if (part.kind !== "conflict") continue;
    const decision = review.decisions[part.index];
    if (!decision) return `Decide conflict hunk ${part.index + 1} before writing.`;
    if (decision.choice === "edited" && hasConflictMarkers(decision.text ?? "")) {
      return `Conflict hunk ${part.index + 1} still contains a marker line.`;
    }
  }
  if (!review.reviewer.trim()) return "Name the reviewer before writing the merge.";
  if (!review.testsAcknowledged) return "Confirm the test hint before writing the merge.";
  if (hasConflictMarkers(resolvedConflictText(merge, review))) {
    return "The merged text still contains a conflict marker.";
  }
  return null;
}

export function conflictDecisions(merge: MergedConflict, review: ConflictReviewState) {
  const parsed = parseMergeText(merge.merged);
  return parsed.parts
    .filter((part): part is ConflictHunkPart => part.kind === "conflict")
    .map((part) => ({
      index: part.index,
      choice: review.decisions[part.index]?.choice ?? "edited",
    }));
}

/** A reproducible hint, derived from the path; it is shown, not run. */
export function conflictTestHint(path: string): string {
  const file = path.replace(/\\/g, "/");
  const test = /^crates\/([^/]+)\/tests\/(.+)\.rs$/.exec(file);
  if (test) return `cargo test -p ${test[1]} --test ${test[2]}`;
  const crate = /^crates\/([^/]+)\//.exec(file);
  if (crate) return `cargo test -p ${crate[1]}`;
  const uiTest = /^ui\/tests\/(.+)$/.exec(file);
  if (uiTest) return `cd ui && npx playwright test tests/${uiTest[1]}`;
  if (file.startsWith("ui/")) return "npm --prefix ui run test:e2e";
  if (file.startsWith("functions/")) return "npm --prefix functions test";
  if (file.endsWith(".rs")) return "cargo test --workspace";
  return "Run the build and the focused checks that cover this file.";
}

export function ConflictEditor({
  merge,
  review,
  onChange,
}: {
  merge: MergedConflict;
  review: ConflictReviewState;
  onChange: (next: ConflictReviewState) => void;
}) {
  const parsed = parseMergeText(merge.merged);
  const hint = conflictTestHint(merge.path);
  const conflicts = parsed.parts.filter(
    (part): part is ConflictHunkPart => part.kind === "conflict",
  );
  const decided = conflicts.filter((part) => review.decisions[part.index]).length;

  const choose = (part: ConflictHunkPart, choice: HunkChoice) => {
    const current = review.decisions[part.index];
    const text =
      choice === "edited"
        ? current?.choice === "edited"
          ? (current.text ?? "")
          : (part.ours || part.theirs)
        : undefined;
    onChange({
      ...review,
      decisions: { ...review.decisions, [part.index]: { choice, text } },
    });
  };

  const reset = (part: ConflictHunkPart) => {
    const decisions = { ...review.decisions };
    delete decisions[part.index];
    onChange({ ...review, decisions });
  };

  return (
    <section className="guided-conflict" aria-label="Guided merge editor">
      <div className="guided-conflict-head">
        <div>
          <h4>Guided three-way merge</h4>
          <p className="fine">
            Each conflicted hunk needs its own decision. There is no accept-all: the marker block
            itself can never be written.
          </p>
        </div>
        <span className={`pill ${conflicts.length === 0 ? "applied" : decided === conflicts.length ? "sent" : "failed"}`}>
          {conflicts.length === 0
            ? "clean merge"
            : `${decided} of ${conflicts.length} hunks decided`}
        </span>
      </div>

      {parsed.error ? (
        <p className="notice" role="alert">
          {parsed.error}
        </p>
      ) : null}

      {conflicts.length === 0 ? (
        <p className="fine guided-clean">
          The three versions merge cleanly; no conflict hunk needs a decision. Name the reviewer
          and confirm the hint before writing it.
        </p>
      ) : (
        <div className="guided-document">
          {parsed.parts.map((part, position) =>
            part.kind === "text" ? (
              <pre className="guided-context" key={`text-${position}`}>
                {part.text}
              </pre>
            ) : (
              <article
                className={`guided-hunk${review.decisions[part.index] ? " decided" : ""}`}
                key={`hunk-${part.index}`}
                aria-label={`Conflict ${part.index + 1} of ${conflicts.length}`}
              >
                <header className="guided-hunk-head">
                  <strong>Conflict {part.index + 1}</strong>
                  <span className="fine">
                    {review.decisions[part.index]
                      ? `accepted ${review.decisions[part.index].choice}`
                      : "needs a decision"}
                  </span>
                </header>
                <div className="guided-sides">
                  <section className="guided-side base" aria-label={`Base of conflict ${part.index + 1}`}>
                    <header>
                      <span>base</span>
                    </header>
                    <pre>{merge.hunks[part.index]?.base || " "}</pre>
                  </section>
                  <section className="guided-side ours" aria-label={`Ours of conflict ${part.index + 1}`}>
                    <header>
                      <span>ours</span>
                      <button
                        type="button"
                        className={`chip${review.decisions[part.index]?.choice === "ours" ? " on" : ""}`}
                        aria-pressed={review.decisions[part.index]?.choice === "ours"}
                        onClick={() => choose(part, "ours")}
                      >
                        accept ours
                      </button>
                    </header>
                    <pre>{part.ours || " "}</pre>
                  </section>
                  <section className="guided-side theirs" aria-label={`Theirs of conflict ${part.index + 1}`}>
                    <header>
                      <span>theirs</span>
                      <button
                        type="button"
                        className={`chip${review.decisions[part.index]?.choice === "theirs" ? " on" : ""}`}
                        aria-pressed={review.decisions[part.index]?.choice === "theirs"}
                        onClick={() => choose(part, "theirs")}
                      >
                        accept theirs
                      </button>
                    </header>
                    <pre>{part.theirs || " "}</pre>
                  </section>
                </div>
                <div className="guided-hunk-actions">
                  <button
                    type="button"
                    className={`chip${review.decisions[part.index]?.choice === "edited" ? " on" : ""}`}
                    aria-pressed={review.decisions[part.index]?.choice === "edited"}
                    onClick={() => choose(part, "edited")}
                  >
                    edit this hunk
                  </button>
                  {review.decisions[part.index] ? (
                    <button type="button" className="chip" onClick={() => reset(part)}>
                      reset decision
                    </button>
                  ) : null}
                </div>
                {review.decisions[part.index]?.choice === "edited" ? (
                  <textarea
                    className="guided-hunk-edit"
                    aria-label={`Edited text for conflict ${part.index + 1}`}
                    value={review.decisions[part.index]?.text ?? ""}
                    rows={Math.min(12, Math.max(3, (review.decisions[part.index]?.text ?? "").split("\n").length))}
                    spellCheck={false}
                    onChange={(event) =>
                      onChange({
                        ...review,
                        decisions: {
                          ...review.decisions,
                          [part.index]: { choice: "edited", text: event.target.value },
                        },
                      })
                    }
                  />
                ) : null}
              </article>
            ),
          )}
        </div>
      )}

      <div className="guided-verdict">
        <label className="guided-reviewer">
          <span>Reviewer</span>
          <input
            value={review.reviewer}
            maxLength={120}
            placeholder="Your name"
            autoComplete="name"
            onChange={(event) => onChange({ ...review, reviewer: event.target.value })}
          />
        </label>
        <div className="guided-test">
          <span className="eyebrow">Test hint</span>
          <code>{hint}</code>
          <label className="guided-ack">
            <input
              type="checkbox"
              checked={review.testsAcknowledged}
              onChange={(event) =>
                onChange({ ...review, testsAcknowledged: event.target.checked })
              }
            />
            I ran this check or a more focused one.
          </label>
        </div>
      </div>
    </section>
  );
}