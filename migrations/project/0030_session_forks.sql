ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions(id);
ALTER TABLE sessions ADD COLUMN parent_turn_id TEXT;
