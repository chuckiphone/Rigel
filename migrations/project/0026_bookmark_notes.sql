-- Roadmap 103: one optional note per bookmark. NULL is both an ordinary star with no note and
-- every row written before this migration, so older bookmarks keep their persisted shape and
-- need no backfill. A note is a single trimmed line capped by the core before it is stored.
ALTER TABLE bookmarks ADD COLUMN note TEXT;
