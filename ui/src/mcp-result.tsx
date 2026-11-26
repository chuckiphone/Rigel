/**
 * Roadmap 145: MCP results as typed rows.
 *
 * A tool result is either text, an image or structured content. Each becomes a labelled row, and
 * one longer body never pushes the rest of the transcript off screen: text is capped with a
 * visible marker saying how much was hidden. The exact recorded body stays one click away in the
 * raw details, and no image URL outside `data:`/`http(s)` is ever handed to an `<img>`.
 */

import { useState } from "react";

/** Characters of visible text; images and row labels are not counted. */
export const RESULT_CHAR_CAP = 2_000;

export type ResultRow =
  | { kind: "text"; text: string }
  | { kind: "image"; url: string; mime: string }
  | { kind: "structured"; label: string; value: string };

function printable(value: unknown): string | null {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return null;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return null;
  }
}

/** Flatten one recorded MCP body into ordered typed rows. Unknown shapes become text rows. */
export function resultRows(body: Record<string, unknown>): ResultRow[] {
  const rows: ResultRow[] = [];
  const result = body.result;
  const content = result && typeof result === "object" ? (result as Record<string, unknown>) : {};
  const blocks = Array.isArray(content.content) ? content.content : [];

  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const item = block as Record<string, unknown>;
    if (item.type === "image") {
      const url = typeof item.data === "string" ? item.data : "";
      const mime = typeof item.mimeType === "string" ? item.mimeType : "image";
      rows.push({ kind: "image", url, mime });
      continue;
    }
    if (item.type === "resource") {
      const resource = item.resource && typeof item.resource === "object" ? item.resource : {};
      const text = printable((resource as Record<string, unknown>).text) ?? "";
      rows.push({ kind: "structured", label: "resource", value: text });
      continue;
    }
    const text = printable(item.text) ?? printable(item);
    if (text !== null) rows.push({ kind: "text", text });
  }

  const structured = content.structuredContent ?? body.structuredContent;
  if (structured && typeof structured === "object") {
    for (const [label, value] of Object.entries(structured as Record<string, unknown>)) {
      const text = printable(value);
      if (text !== null) rows.push({ kind: "structured", label, value: text });
    }
  }

  if (rows.length === 0) {
    const text = printable(result) ?? printable(body.result) ?? "";
    if (text) rows.push({ kind: "text", text });
  }
  return rows;
}

export interface CappedRows {
  visible: ResultRow[];
  truncated: boolean;
  hidden: number;
  used: number;
}

/** Keep whole rows while their text fits; the marker names what was left out. */
export function capRows(rows: readonly ResultRow[], cap = RESULT_CHAR_CAP): CappedRows {
  const visible: ResultRow[] = [];
  let used = 0;
  let hidden = 0;
  for (const row of rows) {
    const size = row.kind === "text" ? row.text.length : row.kind === "structured" ? row.value.length : 0;
    if (used + size > cap) {
      hidden += 1;
      continue;
    }
    visible.push(row);
    used += size;
  }
  return { visible, truncated: hidden > 0, hidden, used };
}

/** Only a data URL or an ordinary http(s) URL may reach the DOM as an image source. */
export function imageSource(row: Extract<ResultRow, { kind: "image" }>): string | null {
  return /^data:image\/[a-z0-9+.-]+;base64,/i.test(row.url) || /^https?:\/\//i.test(row.url)
    ? row.url
    : null;
}

export default function McpResult({ body }: { body: Record<string, unknown> }) {
  const [rawOpen, setRawOpen] = useState(false);
  const rows = resultRows(body);
  const capped = capRows(rows);
  const tool = typeof body.tool === "string" ? body.tool : "mcp tool";

  return (
    <section className="mcp-result" aria-label={`MCP result · ${tool}`}>
      <div className="mcp-result-head">
        <span className="tool-name">{tool}</span>
        <span className="mcp-row-count">
          {rows.length} {rows.length === 1 ? "row" : "rows"}
        </span>
        <button
          type="button"
          className="chip"
          aria-expanded={rawOpen}
          onClick={() => setRawOpen((open) => !open)}
        >
          Raw payload
        </button>
      </div>
      <ul className="mcp-rows">
        {capped.visible.map((row, index) => (
          <li key={index} className={`mcp-row ${row.kind}`}>
            <span className="mcp-kind">{row.kind}</span>
            {row.kind === "text" ? <span className="mcp-text">{row.text}</span> : null}
            {row.kind === "structured" ? (
              <span className="mcp-text">
                <span className="mcp-label">{row.label}</span> {row.value}
              </span>
            ) : null}
            {row.kind === "image" ? (
              imageSource(row) ? (
                <img className="mcp-image" src={imageSource(row) ?? ""} alt={`${row.mime} result`} />
              ) : (
                <span className="mcp-text">image · source refused</span>
              )
            ) : null}
          </li>
        ))}
      </ul>
      {capped.truncated ? (
        <p className="mcp-truncated" role="status">
          Truncated at {RESULT_CHAR_CAP} characters · {capped.hidden} more{" "}
          {capped.hidden === 1 ? "row" : "rows"} hidden. Open Raw payload for all of it.
        </p>
      ) : null}
      {rawOpen ? <pre className="mcp-raw">{JSON.stringify(body, null, 2)}</pre> : null}
    </section>
  );
}
