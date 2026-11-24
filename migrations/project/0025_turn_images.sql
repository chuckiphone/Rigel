-- Image-bearing turns keep the exact ImagePart URLs that were sent: remote HTTPS URLs or
-- local images already inlined as data: URLs. NULL is the ordinary text-only turn and is also
-- every row written before this migration, so no backfill is needed and their persisted shape
-- is unchanged. The per-image cap is enforced when the reference is resolved before this insert.
ALTER TABLE turns ADD COLUMN images_json TEXT;
