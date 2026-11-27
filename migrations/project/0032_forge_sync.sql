-- Idempotent per-session imports from a forge provider.
CREATE TABLE forge_pull_requests (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    state TEXT NOT NULL,
    body TEXT NOT NULL,
    head_ref TEXT,
    head_sha TEXT,
    updated_at TEXT,
    merged_at TEXT,
    source_turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL,
    synced_at INTEGER NOT NULL,
    PRIMARY KEY (session_id, number)
);

CREATE TABLE forge_checks (
    session_id TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    conclusion TEXT,
    details_url TEXT,
    started_at TEXT,
    completed_at TEXT,
    PRIMARY KEY (session_id, pr_number, external_id),
    FOREIGN KEY (session_id, pr_number) REFERENCES forge_pull_requests(session_id, number) ON DELETE CASCADE
);

CREATE TABLE forge_review_comments (
    session_id TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    external_id TEXT NOT NULL,
    body TEXT NOT NULL,
    path TEXT,
    line INTEGER,
    author TEXT,
    created_at TEXT,
    updated_at TEXT,
    PRIMARY KEY (session_id, pr_number, external_id),
    FOREIGN KEY (session_id, pr_number) REFERENCES forge_pull_requests(session_id, number) ON DELETE CASCADE
);
