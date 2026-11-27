-- Versioned project prompt templates and immutable session-turn snapshots.
CREATE TABLE prompt_versions (
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE TABLE prompt_pins (
    turn_id TEXT PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    variables_json TEXT NOT NULL,
    pinned_at INTEGER NOT NULL,
    FOREIGN KEY (name, version) REFERENCES prompt_versions(name, version)
);
