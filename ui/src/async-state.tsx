/**
 * Roadmap 138: one kit for every asynchronous surface.
 *
 * Loading, empty, error, retry and success use the same token-only styling and each writes its
 * sentence into a live region. Errors and retries are assertive because they need a response;
 * the other three are polite. A retry is a real button that only appears when the caller can
 * actually retry, and the preview fixture exposes all five states for the tests.
 */

import { useState, type ReactNode } from "react";

export type AsyncStateKind = "loading" | "empty" | "error" | "retry" | "success";

export const ASYNC_KINDS: readonly AsyncStateKind[] = [
  "loading",
  "empty",
  "error",
  "retry",
  "success",
];

export const ASYNC_COPY: Record<AsyncStateKind, string> = {
  loading: "Working…",
  empty: "Nothing here yet.",
  error: "Something went wrong.",
  retry: "Ready to try again.",
  success: "Done.",
};

/** The exact sentence the live region announces; a detail is appended, never substituted. */
export function asyncAnnouncement(kind: AsyncStateKind, label?: string, detail?: string): string {
  const base = label ?? ASYNC_COPY[kind];
  return detail ? `${base} · ${detail}` : base;
}

export default function AsyncState({
  state,
  label,
  detail,
  onRetry,
  children,
}: {
  state: AsyncStateKind;
  label?: string;
  detail?: string;
  onRetry?: () => void;
  children?: ReactNode;
}) {
  const urgent = state === "error" || state === "retry";
  return (
    <div className={`async-state ${state}`} data-state={state}>
      <div
        className="async-live"
        role={urgent ? "alert" : "status"}
        aria-live={urgent ? "assertive" : "polite"}
        aria-atomic="true"
      >
        {asyncAnnouncement(state, label, detail)}
      </div>
      {state === "loading" ? <span className="async-spinner" aria-hidden="true" /> : null}
      {children ? <div className="async-detail">{children}</div> : null}
      {urgent && onRetry ? (
        <button type="button" className="chip async-retry" onClick={onRetry}>
          Try again
        </button>
      ) : null}
    </div>
  );
}

/** Preview-only fixture: every state is one click away, including a retry that runs. */
export function AsyncStatePreview() {
  const [state, setState] = useState<AsyncStateKind>("loading");
  const [tries, setTries] = useState(0);
  return (
    <section className="async-preview" aria-label="Async state kit">
      <div role="group" aria-label="Choose async state">
        {ASYNC_KINDS.map((kind) => (
          <button
            key={kind}
            type="button"
            className="chip"
            aria-pressed={state === kind}
            onClick={() => setState(kind)}
          >
            {kind}
          </button>
        ))}
      </div>
      <AsyncState
        state={state}
        label={`${state} state`}
        detail={state === "success" ? `${tries} tries` : undefined}
        onRetry={() => {
          setTries((value) => value + 1);
          setState("loading");
        }}
      />
    </section>
  );
}
