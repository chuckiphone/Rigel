---
name: mcptoon
description: Serve a names-only MCP tool manifest and resolve a schema only when a tool is called, cutting the prompt cost of a large catalogue.
license: Apache-2.0
metadata:
  author: Rigel
  tags:
    - mcptoon
---

# mcptoon

## Purpose


A names-only MCP manifest keeps every tool discoverable while leaving the full JSON schemas
out of the prompt until a call needs one. The project reports 255 tools at 581 tokens
instead of 71,929 — treat that as their measurement, not a guarantee for your servers.

## Preconditions

```bash
command -v mcptoon || pip install mcptoon
mcptoon --help
mcptoon discover
```

`discover` only reports what it finds. Use `--write` when you intend to keep the config.

## Workflow

1. List the servers and count the tools before changing anything.
2. Read the compact manifest and confirm every server you expect is present:
   `mcptoon manifest`
3. For one real task, resolve the schema on demand:
   `mcptoon inspect <server> <tool>`
4. Call through the gateway and compare the result with a direct MCP call:
   `mcptoon call <server> <tool> '{"…":"…"}'`

Never enable a write-capable server through a gateway you have not inspected. The gateway
does not make a dangerous tool safe; it makes its schema cheaper to hold.

## In Rigel

- Rigel's own `.mcp.json` approval still governs the server and each tool. A gateway is
  another process asking for the same grant.
- Keep the names-only manifest as a discovery aid, not as the approval record. Rigel's
  schema pin (`MCP_SCHEMA_DENIED`) is the authority on a changed tool.
- If a server is removed from the gateway config, remove its pin too rather than leaving a
  grant for a name nothing serves.

## Boundaries

- Do not send credentials through the gateway's config file; it is plain text.
- A names-only list can hide a destructive tool behind an innocuous name. Inspect before
  calling, and let the approval card show the real schema.
- Report token counts with the server and tool count beside them; an unqualified percentage
  is not evidence.
