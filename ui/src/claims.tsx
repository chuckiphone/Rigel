/**
 * Roadmap 95: the claim dashboard.
 *
 * One view of the ledger: counts, who holds each path, and how old the last heartbeat is.
 * The core never auto-releases a stale claim, and neither does this surface — release needs a
 * button, a typed reason and a confirmed action, and the reason is stored beside the release.
 * The browser preview is deliberately read-only, so no button here can change a real ledger.
 */

import { useState } from "react";
import { api, isTauri, type Claim } from "./api";
import { claimAge, claimCounts, releaseBlocker } from "./claim-data";

export default function ClaimsDashboard({
  claims,
  project,
  onReleased,
}: {
  claims: Claim[];
  project: string;
  /** The native host polls the ledger itself; this lets a release show up on the next read. */
  onReleased?: () => void;
}) {
  const [now] = useState(() => Date.now());
  const [pending, setPending] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const counts = claimCounts(claims);
  const native = isTauri();

  async function release(claim: Claim) {
    const blocker = releaseBlocker(claim, reason);
    if (blocker) {
      setNotice(blocker);
      return;
    }
    if (!native) {
      setNotice("Preview is read-only: no claim was released.");
      setPending(null);
      return;
    }
    setBusy(true);
    try {
      const released = await api.claimRelease(
        project,
        claim.agent_id,
        [claim.path],
        reason.trim(),
      );
      setNotice(
        released.length > 0
          ? `Released ${released.join(", ")}. The reason is in the ledger.`
          : "Nothing was released; the claim may have been released already.",
      );
      setPending(null);
      onReleased?.();
    } catch (problem) {
      setNotice(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="claims-dashboard" aria-label="Claim dashboard">
      <div className="settings-head">
        <h3>Claims</h3>
        <span className="pill">{counts.held} held</span>
      </div>
      <p className="fine claims-stats">
        {counts.total} {counts.total === 1 ? "claim" : "claims"} · {counts.held} held · {counts.stale}{" "}
        stale · {counts.holders} {counts.holders === 1 ? "holder" : "holders"}. A stale heartbeat is
        reported, never released automatically. Releasing is an explicit action that records a
        reason.
      </p>
      {notice ? (
        <p className="claims-notice" role="status">
          {notice}
        </p>
      ) : null}
      {claims.length === 0 ? (
        <p className="empty">No agent has claimed a file yet.</p>
      ) : (
        <ul className="claim-list">
          {claims.map((claim) => {
            const active = claim.released_at === null;
            const isPending = pending === claim.id;
            return (
              <li
                key={claim.id}
                className={claim.stale ? "claim-row stale claim-dashboard-row" : "claim-row claim-dashboard-row"}
              >
                <span className="mono claim-path">{claim.path}</span>
                <span className="claim-owner">
                  {claim.agent} <span className="faint">({claim.harness})</span>
                </span>
                <span className={`pill ${claim.stale ? "failed" : active ? "running" : "cancelled"}`}>
                  {claim.stale ? "stale" : active ? "claimed" : "released"}
                </span>
                <span className="faint mono claim-heartbeat" title="last heartbeat">
                  {claimAge(claim.heartbeat_at, now)}
                </span>
                {active ? (
                  <button
                    type="button"
                    className="chip"
                    onClick={() => {
                      setPending(isPending ? null : claim.id);
                      setReason("");
                      setNotice(null);
                    }}
                  >
                    {isPending ? "Cancel release" : "Release…"}
                  </button>
                ) : null}
                {isPending ? (
                  <form
                    className="release-form"
                    onSubmit={(event) => {
                      event.preventDefault();
                      void release(claim);
                    }}
                  >
                    <label>
                      Reason
                      <input
                        value={reason}
                        onChange={(event) => setReason(event.target.value)}
                        placeholder="why this claim is released"
                      />
                    </label>
                    <button type="submit" className="primary" disabled={busy}>
                      {busy ? "Releasing…" : "Release claim"}
                    </button>
                  </form>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
