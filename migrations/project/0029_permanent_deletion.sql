-- Permanent session deletion keeps an audit marker after the session row is removed.
CREATE TABLE session_tombstones (
  session_id TEXT PRIMARY KEY,
  deleted_at INTEGER NOT NULL
);
