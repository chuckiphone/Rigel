/**
 * Roadmap 164: publication redaction preview.
 *
 * Before a session is sent to the user's cloud, every row it would expose is listed here. Tool
 * output and file diffs start switched off; the choice is stored per session, so touching one
 * session's switches cannot widen another's publication.
 */

import { useState, type ReactNode } from "react";

import type { PublishPreview, PublishRow, PublishScope, TurnTranscript } from "./api";

export const DEFAULT_PUBLISH_SCOPE: PublishScope = { tool_output: false, diffs: false };

function bodyText(row: TurnTranscript["items"][number], key: string): string {
  const value = row.body?.[key];
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim() : "";
}

/** Tool rows do not always carry prose; name the tool or command when they do not. */
function toolValue(row: TurnTranscript["items"][number]): string {
  return (
    bodyText(row, "text") ||
    bodyText(row, "tool") ||
    bodyText(row, "command") ||
    bodyText(row, "name") ||
    row.kind
  );
}

/** The renderer builds the same rows the core builds, from the transcript already on screen. */
export function previewRows(
  session: string,
  turns: readonly TurnTranscript[],
  scope: PublishScope,
): PublishPreview {
  const rows: PublishRow[] = [
    {
      kind: "session",
      label: "Session header",
      value: `session ${session} · ${turns.length} turn(s)`,
      redacted: false,
    },
    {
      kind: "scope",
      label: "Project path and local files",
      value: "not published",
      redacted: true,
    },
  ];
  for (const entry of turns) {
    const prefix = `turn ${entry.turn.seq}`;
    rows.push({
      kind: "prompt",
      label: `${prefix} · prompt`,
      value: entry.turn.prompt ?? "",
      redacted: false,
    });
    rows.push({
      kind: "reply",
      label: `${prefix} · assistant reply`,
      value: entry.items
        .filter((item) => item.kind === "message" || item.kind === "note")
        .map((item) => bodyText(item, "text"))
        .filter(Boolean)
        .join(" "),
      redacted: false,
    });
    for (const item of entry.items.filter((candidate) =>
      ["command", "mcp_call", "web_search", "usage"].includes(candidate.kind),
    )) {
      rows.push({
        kind: "tool_output",
        label: `${prefix} · ${item.kind} output`,
        value: scope.tool_output ? toolValue(item) : "[tool output withheld]",
        redacted: !scope.tool_output,
      });
    }
    for (const item of entry.items.filter((candidate) => candidate.kind === "file_change")) {
      rows.push({
        kind: "diff",
        label: `${prefix} · diff`,
        value: scope.diffs ? bodyText(item, "path") : "[diff withheld]",
        redacted: !scope.diffs,
      });
    }
    if (entry.turn.usage) {
      rows.push({
        kind: "usage",
        label: `${prefix} · cost and tokens`,
        value: `${entry.turn.usage.cost_micros} micros · ${entry.turn.usage.input_tokens} in · ${entry.turn.usage.output_tokens} out`,
        redacted: false,
      });
    }
  }
  return { session_id: session, scope, rows };
}

/** The kinds a reviewer can opt into, even while they are redacted. */
export function redactableKinds(preview: PublishPreview): string[] {
  return [...new Set(preview.rows.map((row) => row.kind))].filter((kind) =>
    ["tool_output", "diff", "scope"].includes(kind),
  );
}

export default function PublishPreviewPanel({
  preview,
  onScope,
  onClose,
  busy = false,
}: {
  preview: PublishPreview;
  /** Persist the scope and return the refreshed rows for this same session. */
  onScope: (scope: PublishScope) => Promise<PublishPreview>;
  onClose?: () => void;
  busy?: boolean;
}): ReactNode {
  const [current, setCurrent] = useState(preview);
  const [error, setError] = useState<string | null>(null);
  const scope = current.scope;

  const apply = (next: PublishScope) => {
    setError(null);
    void onScope(next)
      .then(setCurrent)
      .catch((failure) =>
        setError(failure instanceof Error ? failure.message : String(failure)),
      );
  };

  return (
    <section className="publish-preview" aria-label="Publication preview">
      <header>
        <strong>What publishing exposes</strong>
        <span className="fine">{current.rows.length} row(s)</span>
        {onClose ? (
          <button type="button" onClick={onClose} aria-label="Close publication preview">
            Close
          </button>
        ) : null}
      </header>
      <div className="publish-switches" role="group" aria-label="Publication redaction scope">
        <label>
          <input
            type="checkbox"
            aria-label="Include tool output"
            checked={scope.tool_output}
            disabled={busy}
            onChange={(event) => apply({ ...scope, tool_output: event.target.checked })}
          />
          include tool output (off by default)
        </label>
        <label>
          <input
            type="checkbox"
            aria-label="Include diffs"
            checked={scope.diffs}
            disabled={busy}
            onChange={(event) => apply({ ...scope, diffs: event.target.checked })}
          />
          include diffs (off by default)
        </label>
      </div>
      <table className="publish-rows">
        <thead>
          <tr>
            <th>Row</th>
            <th>Exposed value</th>
          </tr>
        </thead>
        <tbody>
          {current.rows.map((row, index) => (
            <tr key={`${row.kind}-${index}`} data-kind={row.kind}>
              <td>{row.label}</td>
              <td className={row.redacted ? "redacted" : ""}>
                {row.value}
                {row.redacted ? " · redacted" : ""}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {error ? (
        <p className="publish-error" role="alert">
          {error}
        </p>
      ) : null}
      <p className="fine">
        This scope is saved for this session only. Publishing carries exactly the rows shown here;
        tool output and diffs never leave this machine until you turn them on.
      </p>
    </section>
  );
}
