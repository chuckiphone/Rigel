-- Roadmap 57: one compact summary per finished turn, with the item ids it was built from.
-- The summary is derived state: the transcript remains the source of truth, and deleting a
-- turn takes its summary with it. session_id is denormalised so a session's summaries are one
-- indexed read and so the row cannot outlive the session it belongs to.
CREATE TABLE turn_summaries (
    turn_id TEXT PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    summary TEXT NOT NULL,
    item_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL
);

CREATE INDEX turn_summaries_session ON turn_summaries(session_id, created_at);
