-- Roadmap 58: a roster row remembers its own model pair. `model` already existed for spawn and
-- role presets; the provider travels with it so a pick cannot be sent through the previous door,
-- and the effort is the row's own reasoning level. Both are nullable: an unconfigured row falls
-- back to the composer/settings pair.
ALTER TABLE agents ADD COLUMN provider TEXT;
ALTER TABLE agents ADD COLUMN effort TEXT;
