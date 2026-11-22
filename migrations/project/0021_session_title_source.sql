-- Roadmap 56: an automatic title must never overwrite a name the user chose. Existing named
-- sessions predate automatic titles, so they are recorded as manual rather than left open to
-- being renamed by the first generation that runs.
ALTER TABLE sessions ADD COLUMN title_source TEXT;

UPDATE sessions SET title_source = 'manual' WHERE title IS NOT NULL;
