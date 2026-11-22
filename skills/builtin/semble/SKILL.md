---
name: semble
description: Search a codebase by meaning on the CPU and return only relevant snippets instead of grepping and reading whole files.
license: MIT
metadata:
  author: Rigel
  tags:
    - semble
---

# Semble

## Purpose


Semble indexes a repository locally and answers natural-language queries with the code
chunks that match. It runs on CPU, needs no key, and is fast enough to use inside a turn.

## Preconditions

```bash
command -v semble || pip install semble
semble --help
```

Read the installed help before choosing a subcommand; the CLI and the MCP server are both
supported entry points and the flags move between releases.

## Use it well

1. Ask a behavioural question, not a keyword: "where is the permission matrix enforced on
   dispatch", not "permissions".
2. Read the returned snippets before editing. A snippet is a pointer, not proof that the
   path is reached.
3. Follow the symbol into its callers with Rigel's own `code_search` when the answer must be
   exhaustive. Semantic search ranks; it does not guarantee completeness.
4. For a wide refactor, run one query per behaviour and collect the file list, then read
   the files normally.

## MCP integration

Semble ships an MCP server. If you add it to `.mcp.json`, Rigel will show an approval card
with the server's real `tools/list` before any call, and a schema pin will refuse a changed
tool until it is reviewed. Do not bypass that card because the tool is read-only.

## Boundaries

- The index is a local artifact: adding it to the repository is a choice, not a default.
- A local model index can be stale after a large edit. Re-index before trusting a result on
  changed files, or verify with a direct read.
- Do not send private source to a remote embedding service; this tool's value here is that
  it stays on the CPU.
