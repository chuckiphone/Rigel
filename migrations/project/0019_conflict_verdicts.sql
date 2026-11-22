-- A merge is a reviewed write, not a file dumped from a diff tool.
--
-- The conflict row records that a collision happened and `conflict_sides` keeps the three text
-- versions. This table records the human answer: who accepted the resolution, which choice was
-- made for every conflicted hunk, the test hint that was shown, and whether that check was
-- acknowledged. The merged hash is the exact bytes the verdict was about, so a later write
-- cannot borrow a verdict for a different result.
--
-- `decisions_json` is an ordered array of `{"index": 0, "choice": "ours"}` objects. The core
-- validates that it covers each conflict exactly once before inserting the row.
CREATE TABLE IF NOT EXISTS conflict_verdicts (
  conflict_id        TEXT PRIMARY KEY REFERENCES claim_conflicts(id) ON DELETE CASCADE,
  reviewer           TEXT NOT NULL,
  verdict            TEXT NOT NULL CHECK (verdict IN ('accepted')),
  hunks              INTEGER NOT NULL CHECK (hunks >= 0),
  decisions_json     TEXT NOT NULL,
  test_hint          TEXT NOT NULL,
  tests_acknowledged INTEGER NOT NULL CHECK (tests_acknowledged IN (0, 1)),
  merged_hash        TEXT NOT NULL,
  created_at         INTEGER NOT NULL
);