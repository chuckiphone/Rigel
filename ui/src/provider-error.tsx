/**
 * Roadmap 118: the provider error panel inside a turn row.
 *
 * A 401, 429 or 5xx stays in the transcript turn that produced it. The panel quotes the provider's
 * own message, gives one remedy that names the credential to set or the wait before retrying,
 * shows the request id the provider reported, and copies one diagnostics block. It never retries
 * on its own and it never hides the row: the failed turn is the evidence.
 */

import { useState } from "react";

export interface ProviderError {
  status: number;
  message: string;
  provider: string;
  model: string;
  requestId: string;
  remedy: string;
}

const PROVIDER_STATUS = /^(401|429|5\d\d)$/;

function string(body: Record<string, unknown>, ...keys: string[]): string {
  for (const key of keys) {
    const value = body[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

/** A status is taken from the recorded field first, then from the provider's own message. */
export function providerStatus(body: Record<string, unknown>): number | null {
  const raw = body.status ?? body.status_code;
  const direct = typeof raw === "number" ? String(raw) : typeof raw === "string" ? raw.trim() : "";
  if (PROVIDER_STATUS.test(direct)) return Number(direct);
  // A bare number only counts at the start of a provider message or after an HTTP cue, so a
  // line like "stopped after 500 steps" is not mistaken for a server error.
  const found = /(?:^|(?:provider returned|http(?: error)?|status)\s*:?\s*)(401|429|5\d\d)\b/i.exec(
    string(body, "message", "detail", "error"),
  );
  return found ? Number(found[1]) : null;
}

function providerName(body: Record<string, unknown>): string {
  return string(body, "provider", "key", "door") || string(body, "model").split("/")[0] || "the selected provider";
}

function waitText(body: Record<string, unknown>): string {
  const raw = body.retry_after ?? body.retryAfter ?? body.retry_after_seconds;
  if (typeof raw === "number" && Number.isFinite(raw) && raw > 0) return `Wait ${raw} s`;
  if (typeof raw === "string" && /^\d+$/.test(raw.trim()) && Number(raw) > 0) {
    return `Wait ${Number(raw)} s`;
  }
  return "Wait a moment";
}

/** One honest remedy. A 401 names the credential; a 429 or 5xx names the wait. */
export function providerRemedy(body: Record<string, unknown>, status: number): string {
  if (status === 401) {
    return `The credential for ${providerName(body)} was rejected. Open Settings → Keys, set the ${providerName(body)} key again, then retry this turn.`;
  }
  if (status === 429) {
    return `${waitText(body)} before retrying this turn; the provider rate-limited this request.`;
  }
  return `${waitText(body)} and retry this turn; the provider returned a server error. Rigel did not switch doors.`;
}

/** Only the fields a bug report needs. No credential bytes are ever taken from the body. */
export function parseProviderError(body: Record<string, unknown>): ProviderError | null {
  const status = providerStatus(body);
  if (status === null) return null;
  return {
    status,
    message:
      string(body, "message", "detail", "error") ||
      `The provider returned ${status} without a message.`,
    provider: providerName(body),
    model: string(body, "model"),
    requestId: string(body, "request_id", "requestId", "id") || "not reported by the provider",
    remedy: providerRemedy(body, status),
  };
}

/** The exact bytes the Copy button writes; nothing is uploaded. */
export function providerErrorDiagnostics(error: ProviderError, turnId = ""): string {
  return [
    `provider error ${error.status}`,
    `provider ${error.provider}${error.model ? ` / ${error.model}` : ""}`,
    `request ${error.requestId}`,
    `message ${error.message}`,
    `remedy ${error.remedy}`,
    ...(turnId ? [`turn ${turnId}`] : []),
  ].join("\n");
}

export function ProviderErrorPanel({
  body,
  turnId,
}: {
  body: Record<string, unknown>;
  turnId?: string;
}) {
  const [copied, setCopied] = useState(false);
  const error = parseProviderError(body);
  if (!error) return null;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(providerErrorDiagnostics(error, turnId));
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <section
      className="provider-error"
      role="alert"
      aria-label={`Provider error ${error.status}`}
      data-status={error.status}
    >
      <div className="provider-error-head">
        <strong>Provider error · {error.status}</strong>
        {error.model ? <span className="fine mono">{error.model}</span> : null}
      </div>
      <blockquote className="provider-error-message">{error.message}</blockquote>
      <p className="provider-error-remedy">Remedy: {error.remedy}</p>
      <p className="provider-error-request fine mono">
        Request id: <code>{error.requestId}</code>
      </p>
      <div className="provider-error-actions">
        <button type="button" className="chip" onClick={() => void copy()}>
          Copy diagnostics
        </button>
        {copied ? (
          <span className="fine" role="status">
            Copied.
          </span>
        ) : null}
      </div>
    </section>
  );
}
