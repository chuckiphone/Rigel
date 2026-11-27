/**
 * Roadmap 179: three-way metadata conflicts.
 *
 * A field that both machines changed cannot be resolved by a timestamp: the reviewer sees base,
 * ours and theirs side by side and records an explicit choice with their identity. Publishing is
 * blocked until every conflicting field has a decision. The shell owns the same rule, so a button
 * is a convenience, not the enforcement point.
 */

import { useState, type ReactNode } from "react";

import type { MetadataChoice, MetadataField, MetadataView } from "./api";

/** A field only needs a decision when the two sides disagree. */
export function conflicts(fields: readonly MetadataField[]): MetadataField[] {
  return fields.filter((field) => field.ours !== field.theirs);
}

export function unresolvedFields(view: MetadataView): string[] {
  const decided = new Set(view.decisions.map((decision) => decision.field));
  return conflicts(view.fields)
    .filter((field) => !decided.has(field.name))
    .map((field) => field.name);
}

/** The same gate the shell applies: one unresolved field is enough to refuse a publish. */
export function canPublish(view: MetadataView): boolean {
  return unresolvedFields(view).length === 0;
}

export function resolvedValue(
  field: MetadataField,
  choice: MetadataChoice,
): string {
  if (choice === "base") return field.base;
  if (choice === "theirs") return field.theirs;
  return field.ours;
}

export function decisionTime(millis: number): string {
  return new Date(millis).toISOString();
}

const CHOICES: { choice: MetadataChoice; label: string }[] = [
  { choice: "base", label: "Base" },
  { choice: "ours", label: "Ours" },
  { choice: "theirs", label: "Theirs" },
];

export default function MetadataConflict({
  view,
  author,
  onDecide,
  busy = false,
}: {
  view: MetadataView;
  /** The signed-in identity, never a free-text guess the reviewer can leave blank. */
  author: string;
  onDecide: (field: string, choice: MetadataChoice) => Promise<void>;
  busy?: boolean;
}): ReactNode {
  const [picked, setPicked] = useState<Record<string, MetadataChoice>>({});
  const [error, setError] = useState<string | null>(null);
  const unresolved = unresolvedFields(view);
  const decided = new Map(view.decisions.map((decision) => [decision.field, decision]));

  return (
    <section className="metadata-conflict" aria-label="Metadata conflicts">
      <header>
        <strong>Cloud metadata conflicts</strong>
        <span className="pill" aria-label="Unresolved fields">
          {unresolved.length === 0 ? "resolved" : `${unresolved.length} unresolved`}
        </span>
      </header>
      <p className="fine">
        Each field shows the value both sides started from, this machine's value and the cloud's
        value. Pick one to record a decision with your identity; publishing waits for all of them.
      </p>

      {view.fields.length === 0 ? (
        <p className="fine">No metadata is under review.</p>
      ) : null}

      <ul className="metadata-fields" aria-label="Metadata fields">
        {view.fields.map((field) => {
          const decision = decided.get(field.name);
          const conflicting = field.ours !== field.theirs;
          return (
            <li
              className={`metadata-field${unresolved.includes(field.name) ? " unresolved" : " resolved"}`}
              data-field={field.name}
              key={field.name}
            >
              <div className="metadata-field-head">
                <code>{field.name}</code>
                {decision ? (
                  <span className="pill ok" aria-label={`Decision for ${field.name}`}>
                    {decision.choice} · {decision.author}
                  </span>
                ) : conflicting ? (
                  <span className="pill warn" aria-label={`Decision for ${field.name}`}>
                    needs a decision
                  </span>
                ) : (
                  <span className="pill ok" aria-label={`Decision for ${field.name}`}>
                    agreed
                  </span>
                )}
              </div>
              <dl className="metadata-sides">
                <div className="metadata-side">
                  <dt>Base</dt>
                  <dd>{field.base || "—"}</dd>
                </div>
                <div className="metadata-side">
                  <dt>Ours</dt>
                  <dd>{field.ours || "—"}</dd>
                </div>
                <div className="metadata-side">
                  <dt>Theirs</dt>
                  <dd>{field.theirs || "—"}</dd>
                </div>
              </dl>
              {decision ? (
                <p className="fine" aria-label={`Recorded decision ${field.name}`}>
                  {decision.choice} chosen by {decision.author} at {decisionTime(decision.decided_at)}
                </p>
              ) : conflicting ? (
                <div className="metadata-choices" role="group" aria-label={`Choose ${field.name}`}>
                  {CHOICES.map(({ choice, label }) => (
                    <label key={choice}>
                      <input
                        type="radio"
                        name={`metadata-${field.name}`}
                        aria-label={`${label} for ${field.name}`}
                        checked={(picked[field.name] ?? "ours") === choice}
                        onChange={() =>
                          setPicked((current) => ({ ...current, [field.name]: choice }))
                        }
                      />
                      {label}
                    </label>
                  ))}
                  <button
                    type="button"
                    disabled={busy || !author.trim()}
                    onClick={() => {
                      setError(null);
                      void onDecide(field.name, picked[field.name] ?? "ours").catch((failure) =>
                        setError(failure instanceof Error ? failure.message : String(failure)),
                      );
                    }}
                  >
                    Record decision
                  </button>
                </div>
              ) : null}
            </li>
          );
        })}
      </ul>

      {error ? (
        <p className="metadata-error" role="alert">
          {error}
        </p>
      ) : null}

      {unresolved.length > 0 ? (
        <p className="metadata-blocked" role="alert">
          Publishing is blocked until {unresolved.join(", ")} {unresolved.length === 1 ? "is" : "are"}{" "}
          resolved.
        </p>
      ) : (
        <p className="metadata-ready" role="status">
          Every conflict has a recorded decision; publishing is allowed.
        </p>
      )}
    </section>
  );
}
