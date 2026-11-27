/**
 * Roadmap 82: safe mode is visible on every turn row and can only be left with the typed phrase
 * the core enforces. Entering is an explicit action too; no settings flip can change it quietly.
 */

import { useCallback, useEffect, useState } from "react";

import { api, type SafeModeView } from "./api";

export function canLeave(state: SafeModeView, phrase: string): boolean {
  return state.mode === "safe" && phrase === state.leave_phrase;
}

/** The mode badge used on every transcript row. */
export function SafeModeBadge({ mode }: { mode?: string }) {
  if (mode !== "safe") return null;
  return (
    <span className="turn-mode safe" aria-label="Turn ran in safe mode">
      safe
    </span>
  );
}

export default function SafeModePanel({
  project,
  session,
}: {
  project: string | null;
  session: string | null;
}) {
  const [state, setState] = useState<SafeModeView>({ mode: "normal", leave_phrase: "leave safe mode" });
  const [phrase, setPhrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    if (!project || !session) return;
    try {
      setState(await api.safeModeGet(project, session));
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    }
  }, [project, session]);

  useEffect(() => {
    void load();
  }, [load]);

  const change = async (enter: boolean) => {
    if (!project || !session) return;
    setBusy(true);
    setError(null);
    try {
      setState(await api.safeModeSet(project, session, enter, phrase));
      setPhrase("");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  };

  const safe = state.mode === "safe";
  return (
    <section className={`safe-mode-panel ${safe ? "safe" : "normal"}`} data-mode={state.mode} role="region" aria-label="Safe mode">
      <div className="safe-mode-head">
        <strong>{safe ? "Safe mode" : "Normal mode"}</strong>
        <span className="fine">
          {safe ? "Tools, network and scripts are off for this session." : "Tools, network and scripts are available."}
        </span>
      </div>
      {safe ? (
        <div className="safe-mode-leave">
          <label>
            Type <code>{state.leave_phrase}</code> to leave
            <input
              aria-label="Safe mode confirmation"
              value={phrase}
              onChange={(event) => setPhrase(event.target.value)}
              disabled={busy}
              spellCheck={false}
            />
          </label>
          <button
            type="button"
            className="primary"
            onClick={() => void change(false)}
            disabled={busy || !canLeave(state, phrase)}
          >
            Leave safe mode
          </button>
        </div>
      ) : (
        <button type="button" className="chip" onClick={() => void change(true)} disabled={busy}>
          Open this session in safe mode
        </button>
      )}
      {error ? (
        <p className="safe-mode-error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
