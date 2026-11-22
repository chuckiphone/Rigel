-- Roadmap 55: facts carved from the committed memory files, with the state that outlives a
-- rewrite. The file is still the source of the text; this table only remembers which fact the
-- user pinned and how often a recall returned it.
CREATE TABLE memory_facts (
  id           TEXT PRIMARY KEY,
  agent_id     TEXT NOT NULL,
  topic        TEXT NOT NULL,
  fact_hash    TEXT NOT NULL,
  text         TEXT NOT NULL,
  ordinal      INTEGER NOT NULL,
  pinned       INTEGER NOT NULL DEFAULT 0,
  uses         INTEGER NOT NULL DEFAULT 0,
  last_used_at INTEGER,
  updated_at   INTEGER NOT NULL
);

CREATE INDEX memory_facts_agent ON memory_facts (agent_id, topic);

CREATE UNIQUE INDEX memory_facts_identity ON memory_facts (agent_id, topic, fact_hash);
