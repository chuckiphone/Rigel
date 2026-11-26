/**
 * Roadmap 95: the numbers and the age behind the claim dashboard.
 *
 * Kept free of React so the arithmetic is testable on its own. A claim is advice, never a
 * lock the window owns: a stale heartbeat is reported, and release is a separate, explicit
 * act with a reason that stays in the ledger (N1, I3).
 */

import type { Claim } from "./api";

export interface ClaimCounts {
  total: number;
  held: number;
  stale: number;
  released: number;
  holders: number;
}

/** Count the live ledger without double-counting an agent that holds several paths. */
export function claimCounts(claims: Claim[]): ClaimCounts {
  let held = 0;
  let stale = 0;
  let released = 0;
  const holders = new Set<string>();
  for (const claim of claims) {
    if (claim.released_at !== null) {
      released += 1;
      continue;
    }
    held += 1;
    if (claim.stale) stale += 1;
    if (claim.agent_id) holders.add(claim.agent_id);
  }
  return { total: claims.length, held, stale, released, holders: holders.size };
}

/** A heartbeat's age, clamped at zero so a clock skew never prints a negative age. */
export function claimAge(at: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/** The only gate on releasing a claim: a live row and a reason someone can read later. */
export function releaseBlocker(claim: Claim, reason: string): string | null {
  if (claim.released_at !== null) {
    return "This claim is already released; its reason is kept in the ledger.";
  }
  if (reason.trim().length < 8) {
    return "Type a reason of at least 8 characters before releasing.";
  }
  return null;
}
