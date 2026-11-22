-- Roadmap 66: an individual agent may be narrowed below its role. NULL means the role alone
-- decides; a JSON array of canonical tool names is an additional filter, never a grant of its
-- own. The project ceiling still applies on every dispatch.
ALTER TABLE agents ADD COLUMN tool_overrides TEXT;
