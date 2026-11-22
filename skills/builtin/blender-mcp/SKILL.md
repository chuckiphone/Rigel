---
name: blender-mcp
description: Drive a running Blender session through MCP to inspect a scene, add and transform objects, set materials and render a preview.
license: MIT
metadata:
  author: Rigel
  tags:
    - blender-mcp
---

# Blender MCP

## Purpose


Blender MCP connects an agent to a live Blender session. The add-on runs inside Blender;
the MCP server runs as a separate process and forwards calls over a local socket.

## Preconditions

1. Blender is installed and open with the target scene loaded.
2. The Blender MCP add-on is installed and enabled in Blender's preferences.
3. The add-on's server is started in the 3D viewport sidebar.
4. The Python server is available:

```bash
command -v uvx || pip install uv
uvx blender-mcp
```

If any step is missing, say which one. A server that answers without Blender open is not a
working connection.

## Rigel wiring

Add the server to the project's `.mcp.json`:

```json
{
  "mcpServers": {
    "blender": { "command": "uvx", "args": ["blender-mcp"] }
  }
}
```

Rigel will refuse the server until the approval card shows its real `tools/list`, and each
tool asks on first use. Read the tool names before approving: scene-changing tools and
read-only inspection share one server.

## Working order

1. Inspect first: object names, collections, transforms and the current frame.
2. Make one change at a time and read the result back. Do not batch ten transforms and hope.
3. Name what you created with a stable prefix so a later turn can find it.
4. Render a small preview before claiming the result looks right. If you cannot view it,
   say so rather than describing it as correct.
5. Save the `.blend` only when asked; an experimental scene should not overwrite the
   working file.

## Boundaries

- A destructive scene operation (delete all, apply modifier, overwrite) needs the same
  explicit approval as a destructive shell command.
- Do not install add-ons from an unverified path into a Blender that holds the user's work.
- The add-on version and the Python package version must match; report both when debugging.
