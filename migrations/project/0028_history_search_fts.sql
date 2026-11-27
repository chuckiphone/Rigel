-- Local FTS5 index for searchable history. The three tables stay separate so each source can
-- retain its own rowid while exposing the same metadata columns to the query layer.
CREATE VIRTUAL TABLE turns_fts USING fts5(
  session_id UNINDEXED,
  turn_id UNINDEXED,
  agent_id UNINDEXED,
  model UNINDEXED,
  project_root UNINDEXED,
  created_at UNINDEXED,
  kind UNINDEXED,
  path UNINDEXED,
  content
);

CREATE VIRTUAL TABLE items_fts USING fts5(
  session_id UNINDEXED,
  turn_id UNINDEXED,
  agent_id UNINDEXED,
  model UNINDEXED,
  project_root UNINDEXED,
  created_at UNINDEXED,
  kind UNINDEXED,
  path UNINDEXED,
  content
);

CREATE VIRTUAL TABLE patches_fts USING fts5(
  session_id UNINDEXED,
  turn_id UNINDEXED,
  agent_id UNINDEXED,
  model UNINDEXED,
  project_root UNINDEXED,
  created_at UNINDEXED,
  kind UNINDEXED,
  path UNINDEXED,
  content
);

INSERT INTO turns_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
SELECT t.rowid, a.session_id, t.id, t.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
       t.started_at, 'prompt', '', COALESCE(t.prompt, '')
FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id;

INSERT INTO items_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
SELECT i.rowid, a.session_id, t.id, t.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
       i.created_at, i.kind, '', i.body_json
FROM items i JOIN turns t ON t.id = i.turn_id JOIN agents a ON a.id = t.agent_id
JOIN sessions s ON s.id = a.session_id;

INSERT INTO patches_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
SELECT p.rowid, p.session_id, '', p.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
       p.created_at, 'patch', p.path, p.path || char(10) || p.diff
FROM patches p LEFT JOIN agents a ON a.id = p.agent_id LEFT JOIN sessions s ON s.id = p.session_id;

CREATE TRIGGER turns_fts_ai AFTER INSERT ON turns BEGIN
  INSERT INTO turns_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, a.session_id, new.id, new.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.started_at, 'prompt', '', COALESCE(new.prompt, '')
  FROM agents a JOIN sessions s ON s.id = a.session_id WHERE a.id = new.agent_id;
END;

CREATE TRIGGER turns_fts_ad AFTER DELETE ON turns BEGIN
  DELETE FROM turns_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER turns_fts_au AFTER UPDATE ON turns BEGIN
  DELETE FROM turns_fts WHERE rowid = old.rowid;
  INSERT INTO turns_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, a.session_id, new.id, new.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.started_at, 'prompt', '', COALESCE(new.prompt, '')
  FROM agents a JOIN sessions s ON s.id = a.session_id WHERE a.id = new.agent_id;
END;

CREATE TRIGGER items_fts_ai AFTER INSERT ON items BEGIN
  INSERT INTO items_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, a.session_id, t.id, t.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.created_at, new.kind, '', new.body_json
  FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id
  WHERE t.id = new.turn_id;
END;

CREATE TRIGGER items_fts_ad AFTER DELETE ON items BEGIN
  DELETE FROM items_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER items_fts_au AFTER UPDATE ON items BEGIN
  DELETE FROM items_fts WHERE rowid = old.rowid;
  INSERT INTO items_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, a.session_id, t.id, t.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.created_at, new.kind, '', new.body_json
  FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id
  WHERE t.id = new.turn_id;
END;

CREATE TRIGGER patches_fts_ai AFTER INSERT ON patches BEGIN
  INSERT INTO patches_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, new.session_id, '', new.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.created_at, 'patch', new.path, new.path || char(10) || new.diff
  FROM sessions s LEFT JOIN agents a ON a.id = new.agent_id WHERE s.id = new.session_id;
END;

CREATE TRIGGER patches_fts_ad AFTER DELETE ON patches BEGIN
  DELETE FROM patches_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER patches_fts_au AFTER UPDATE ON patches BEGIN
  DELETE FROM patches_fts WHERE rowid = old.rowid;
  INSERT INTO patches_fts(rowid, session_id, turn_id, agent_id, model, project_root, created_at, kind, path, content)
  SELECT new.rowid, new.session_id, '', new.agent_id, COALESCE(a.model, ''), COALESCE(s.project_root, ''),
         new.created_at, 'patch', new.path, new.path || char(10) || new.diff
  FROM sessions s LEFT JOIN agents a ON a.id = new.agent_id WHERE s.id = new.session_id;
END;
