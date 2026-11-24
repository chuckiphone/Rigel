# The code graph

The Graph surface (rail: **Graph**) is a local, read-only map of the open project: files, their
top-level symbols, and the imports and name references that connect them. Click a node to see
what links to it; focus a file or symbol to keep its neighbourhood.

## What is a node

| Node | Source |
|---|---|
| `file` | every source file `tree::project_files` returns for the project |
| `symbol` | top-level function, class, struct, enum or similar declaration the scanner can see |
| `concept` | an architecture label named by a live free OpenRouter row, linked to the files it covers |

Edges are `imports` (a resolved relative import), `defines` (file → symbol) and `references`
(a symbol body or import names another symbol). The scanner is deliberately not a compiler: it
uses bounded text rules per language, so a reference is a name match, not a type-checked call.

## The background indexer

Opening a project starts one watcher thread. Every 20 seconds it fingerprints the project's
source metadata (path, size, modification time). When the fingerprint moves it:

1. rebuilds the bounded graph;
2. keeps the previous concept layer only where its files still exist;
3. writes `.rigel/codegraph.json` and `.rigel/codegraph-status.json`;
4. at most once every ten minutes, asks a live free OpenRouter chat model for up to 8
   architecture concepts and merges the ones that point at real files.

The Graph header shows the last pass, its age, the concept count and the model that named them.
A failed pass writes the reason into the status and leaves the previous index in place. The
watcher switches with the open project; it never runs one scanner per project.

## Bounds

| Bound | Value |
|---|---|
| Source files per scan | 320 |
| Bytes read per file | 512 KiB |
| Nodes / edges | 900 / 2 400 |
| Files named to the free agent | 120 |
| Concepts kept | 12 |
| Concept request timeout | 45 s |

`truncated` in the graph response means a bound was hit; the surface says so rather than
pretending the list is complete.

## Privacy and cost

The deterministic scan is local and offline. The concept pass is the only network step: it sends
project-relative paths and at most the symbol names already in the graph, never file bodies,
credentials or absolute paths. It selects a row the live roster marks zero-priced, so the pass
does not spend paid credits. With no OpenRouter key, or no live free row, the pass records the
reason and the local graph keeps working.

The index lives under `.rigel/`, which is already ignored by the project. It is not a second
source of truth: deleting it only costs the next scan and the current concept labels.
