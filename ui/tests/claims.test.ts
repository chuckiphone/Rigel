import test from "node:test";
import assert from "node:assert/strict";
import type { Claim } from "../src/api";
import { claimAge, claimCounts, releaseBlocker } from "../src/claim-data";

function claim(overrides: Partial<Claim> = {}): Claim {
  return {
    id: "cl-1",
    path: "ui/src/session.tsx",
    agent_id: "agent-1",
    agent: "worker",
    harness: "native",
    base_hash: null,
    acquired_at: 0,
    heartbeat_at: 0,
    released_at: null,
    release_reason: null,
    stale: false,
    held: true,
    ...overrides,
  };
}

test("the dashboard counts held, stale and released claims without double-counting a holder", () => {
  const counts = claimCounts([
    claim({ id: "cl-1", agent_id: "a" }),
    claim({ id: "cl-2", agent_id: "a", stale: true }),
    claim({ id: "cl-3", agent_id: "b" }),
    claim({
      id: "cl-4",
      agent_id: "c",
      released_at: 1,
      release_reason: "done",
      stale: false,
      held: false,
    }),
  ]);
  assert.deepEqual(counts, { total: 4, held: 3, stale: 1, released: 1, holders: 2 });
});

test("a heartbeat age is clamped and names seconds, minutes, hours and days", () => {
  const now = 1_000_000_000;
  assert.equal(claimAge(now + 5_000, now), "0s ago");
  assert.equal(claimAge(now - 2_000, now), "2s ago");
  assert.equal(claimAge(now - 300_000, now), "5m ago");
  assert.equal(claimAge(now - 7_200_000, now), "2h ago");
  assert.equal(claimAge(now - 172_800_000, now), "2d ago");
});

test("release needs a live claim and a readable reason, and whitespace alone is not a reason", () => {
  assert.equal(releaseBlocker(claim(), "done with the edit"), null);
  assert.equal(releaseBlocker(claim(), "  abcdefgh  "), null);
  assert.match(String(releaseBlocker(claim(), "short")), /at least 8 characters/);
  assert.match(String(releaseBlocker(claim(), "        ")), /at least 8 characters/);
  assert.match(
    String(releaseBlocker(claim({ released_at: 1, release_reason: "done" }), "a real reason")),
    /already released/,
  );
});
