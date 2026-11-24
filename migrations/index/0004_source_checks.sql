-- 0004: the clock and failure count a repeated source check needs.
--
-- `last_sync_at` answers "how old is the data?", but not "when did we last try?".
-- A background checker needs the latter: a source whose last success is two days old must not be
-- fetched on every tick, and one that failed right after a successful sync must not wait a full
-- day. `failures` drives a bounded retry backoff and resets on the next success.

ALTER TABLE sources ADD COLUMN last_attempt_at INTEGER;
ALTER TABLE sources ADD COLUMN failures INTEGER NOT NULL DEFAULT 0;

-- The last successful sync is the best evidence an existing row has of its last attempt. Without
-- this, every upgraded source would say "checked never" until the next pass.
UPDATE sources SET last_attempt_at = last_sync_at WHERE last_sync_at IS NOT NULL;
