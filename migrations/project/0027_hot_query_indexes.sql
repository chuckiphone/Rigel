-- Item 181: the high-volume session and transcript reads need predictable indexed plans.
CREATE INDEX IF NOT EXISTS turns_agent_latest
  ON turns (agent_id, started_at DESC, seq DESC);

CREATE INDEX IF NOT EXISTS items_turn_kind_seq
  ON items (turn_id, kind, seq);

CREATE INDEX IF NOT EXISTS turns_stalled
  ON turns (completed_at DESC, seq DESC)
  WHERE status = 'failed' OR stop_reason = 'budget_exceeded';

CREATE INDEX IF NOT EXISTS turns_usage_present
  ON turns (agent_id, started_at)
  WHERE usage_json IS NOT NULL;

CREATE INDEX IF NOT EXISTS sessions_spend_title
  ON sessions (id, title);

CREATE INDEX IF NOT EXISTS bookmarks_session_recent
  ON bookmarks (session_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS sessions_list_order
  ON sessions ((pinned_at IS NULL), pinned_at DESC, created_at DESC, id DESC);
