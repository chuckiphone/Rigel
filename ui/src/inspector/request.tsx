/**
 * Roadmap 119: one request inspector.
 *
 * The struct below is both what the window shows and what Copy as JSON writes, so a copied report
 * cannot drift from the screen. Header values for credential names are replaced by an empty value
 * and a `redacted` flag before either the DOM or the JSON sees them: no secret is rendered, copied
 * or stored here. Missing facts say "not recorded" instead of being guessed from a response.
 */

import { useState } from "react";

export interface HeaderRow {
  name: string;
  value: string;
  redacted: boolean;
}

export interface RequestInspection {
  request: { method: string; path: string; headers: HeaderRow[]; body: unknown };
  response: { status: number | null; headers: HeaderRow[]; body: unknown };
  route: {
    provider: string;
    model: string;
    resolved_provider: string;
    resolved_model: string;
    reason: string;
  };
  tokens: { input: number; output: number; cached: number; total: number };
  timing: { duration_ms: number; queue_ms: number };
  tool_schema: unknown;
}

const SECRET_NAMES = /^(authorization|proxy-authorization|api[-_]?key|x-api-key|x-auth-token|cookie|set-cookie|token|secret|rigel[-_]?update[-_]?pubkey)$/i;

/** A redacted row keeps the header name and drops the value entirely. */
export function redactHeaders(rows: readonly HeaderRow[]): HeaderRow[] {
  return rows.map((row) =>
    SECRET_NAMES.test(row.name.trim())
      ? { name: row.name, value: "", redacted: true }
      : { ...row, redacted: false },
  );
}

/** The exact bytes Copy as JSON writes; the canonical struct is this object. */
export function inspectionJson(inspection: RequestInspection): string {
  return JSON.stringify(inspection, null, 2);
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
}

function headerRows(value: unknown): HeaderRow[] {
  if (Array.isArray(value)) {
    return value
      .filter((row): row is Record<string, unknown> => Boolean(row) && typeof row === "object")
      .map((row) => ({
        name: String(row.name ?? ""),
        value: String(row.value ?? ""),
        redacted: row.redacted === true,
      }));
  }
  return Object.entries(record(value)).map(([name, item]) => ({ name, value: String(item ?? ""), redacted: false }));
}

function number(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** Build an inspection from a recorded usage row; absent provider details stay honest. */
export function fromUsage(body: Record<string, unknown>): RequestInspection {
  const route = record(body.route);
  const input = number(body.input_tokens);
  const output = number(body.output_tokens);
  const cached = number(body.cached_input_tokens);
  const inspection: RequestInspection = {
    request: {
      method: "POST",
      path: typeof body.path === "string" ? body.path : "not recorded",
      headers: redactHeaders(headerRows(body.request_headers)),
      body: body.request ?? null,
    },
    response: {
      status: typeof body.status === "number" ? body.status : null,
      headers: redactHeaders(headerRows(body.response_headers)),
      body: body.response ?? null,
    },
    route: {
      provider: typeof route.provider === "string" ? route.provider : "not recorded",
      model: typeof route.model === "string" ? route.model : "not recorded",
      resolved_provider:
        typeof route.resolved_provider === "string" ? route.resolved_provider : "not recorded",
      resolved_model:
        typeof route.resolved_model === "string" ? route.resolved_model : "not recorded",
      reason: typeof route.reason === "string" ? route.reason : "not recorded",
    },
    tokens: { input, output, cached, total: input + output + cached },
    timing: { duration_ms: number(body.duration_ms), queue_ms: number(body.queue_ms) },
    tool_schema: body.tool_schema ?? null,
  };
  return inspection;
}

function bodyText(value: unknown): string {
  if (value === null || value === undefined) return "not recorded";
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function Headers({ title, rows }: { title: string; rows: HeaderRow[] }) {
  return (
    <div className="request-headers">
      <h4>{title}</h4>
      {rows.length === 0 ? (
        <p className="fine">not recorded</p>
      ) : (
        <dl>
          {rows.map((row, index) => (
            <div key={`${row.name}-${index}`} className={row.redacted ? "redacted" : ""}>
              <dt className="mono">{row.name}</dt>
              <dd className="mono">
                {row.redacted ? "redacted · no value stored" : row.value || "(empty)"}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  );
}

export function RequestInspector({ inspection }: { inspection: RequestInspection }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(inspectionJson(inspection));
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };
  const route = inspection.route;
  return (
    <section className="request-inspector" aria-label="Request inspector">
      <div className="request-inspector-head">
        <h3>Request inspector</h3>
        <button type="button" className="chip" onClick={() => void copy()}>
          Copy as JSON
        </button>
      </div>
      {copied ? (
        <p className="fine" role="status">
          Copied the exact struct shown here.
        </p>
      ) : null}

      <p className="request-route mono">
        {route.provider} / {route.model} → {route.resolved_provider} / {route.resolved_model} · {route.reason}
      </p>
      <dl className="request-stats">
        <div>
          <dt>tokens</dt>
          <dd>
            {inspection.tokens.input} in · {inspection.tokens.output} out · {inspection.tokens.cached}{" "}
            cached · {inspection.tokens.total} total
          </dd>
        </div>
        <div>
          <dt>timing</dt>
          <dd>
            {inspection.timing.duration_ms} ms model · {inspection.timing.queue_ms} ms queue
          </dd>
        </div>
      </dl>

      <div className="request-bodies">
        <div>
          <h4>Request · {inspection.request.method} {inspection.request.path}</h4>
          <pre>{bodyText(inspection.request.body)}</pre>
        </div>
        <div>
          <h4>Response · {inspection.response.status ?? "not recorded"}</h4>
          <pre>{bodyText(inspection.response.body)}</pre>
        </div>
      </div>
      <Headers title="Request headers" rows={inspection.request.headers} />
      <Headers title="Response headers" rows={inspection.response.headers} />

      <div className="request-schema">
        <h4>Tool schema</h4>
        <pre>{bodyText(inspection.tool_schema)}</pre>
      </div>
    </section>
  );
}

/** Preview-only fixture: a shaped secret must disappear from both the view and the JSON. */
export default RequestInspector;

export const PREVIEW_INSPECTION: RequestInspection = {
  request: {
    method: "POST",
    path: "/v1/chat/completions",
    headers: [
      { name: "Authorization", value: "Bearer sk-preview-secret-value", redacted: false },
      { name: "x-api-key", value: "sk-preview-secret-value", redacted: false },
      { name: "content-type", value: "application/json", redacted: false },
    ],
    body: { model: "openai/gpt-5-nano", stream: true, messages: [{ role: "user", content: "hello" }] },
  },
  response: {
    status: 200,
    headers: [
      { name: "set-cookie", value: "session=sk-preview-secret-value", redacted: false },
      { name: "x-request-id", value: "req-42", redacted: false },
    ],
    body: { id: "chat-42", choices: [{ message: { role: "assistant", content: "hi" } }] },
  },
  route: {
    provider: "openrouter",
    model: "fast",
    resolved_provider: "openrouter",
    resolved_model: "openai/gpt-5-nano",
    reason: "alias",
  },
  tokens: { input: 120, output: 45, cached: 20, total: 185 },
  timing: { duration_ms: 820, queue_ms: 35 },
  tool_schema: {
    name: "read_file",
    parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
  },
};

export function RequestInspectorPreview({ enabled }: { enabled: boolean }) {
  const [inspection] = useState(() => ({
    ...PREVIEW_INSPECTION,
    request: { ...PREVIEW_INSPECTION.request, headers: redactHeaders(PREVIEW_INSPECTION.request.headers) },
    response: { ...PREVIEW_INSPECTION.response, headers: redactHeaders(PREVIEW_INSPECTION.response.headers) },
  }));
  if (!enabled) return null;
  return <RequestInspector inspection={inspection} />;
}
