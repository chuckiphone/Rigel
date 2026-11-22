# Plugins and the catalogue

A plugin is a directory with a manifest and an entry command. Rigel never imports its code and
never hands it the project: the host starts one short-lived child per tool call, scrubs the
environment, and answers only the capabilities the manifest declares. The catalogue mixes
first-party things Rigel knows how to install — a built-in skill or a pinned plugin tree — with
external marketplace listings that name their own upstream install command.

This is roadmap item 153. It ships the plugin API, the capability broker, the install store and
the marketplace surface. It deliberately does **not** ship WASM plugins, plugin hooks into the
turn loop, a remote registry or an OS-level sandbox for plugins; those limits are named below.

## The manifest

`plugin.json` is JSON, not YAML: one parser, one error shape, and unknown fields are refused
rather than ignored.

```json
{
  "schema": 1,
  "name": "rigel-hello",
  "version": "1.0.0",
  "description": "Two small tools that show the protocol and the capability broker working.",
  "entry": { "command": "python", "args": ["main.py"] },
  "capabilities": {
    "filesystem": { "read": ["."], "write": [] },
    "network": false,
    "process": false,
    "env": []
  },
  "tools": [
    {
      "name": "hello",
      "description": "Return a greeting that proves the plugin process answered.",
      "parameters": { "type": "object", "properties": { "name": { "type": "string" } } }
    }
  ]
}
```

Rules enforced before a byte is copied or a process starts:

- `schema` must be `1`.
- `name` is 1–64 ASCII letters, digits, `-` or `_`, starting with a letter or digit.
- `version` is 1–64 characters from letters, digits, `.`, `-`, `+`, `_`.
- A plugin with tools needs `entry`; the command and args cannot contain NUL.
- Tool names are unique, 1–64 characters, with a non-empty description under 400 characters
  and a JSON-object parameter schema.
- Capability roots cannot contain `..`, be empty, or contain NUL. `env` names are portable
  environment variable names.
- Symlinked files are refused during install, so a receipt covers the bytes that run. A tree is
  capped at 2 000 files and 50 MB.

## The protocol

Newline-delimited JSON on stdin/stdout, protocol version 1:

```text
host → {"type":"hello","protocol":1,"plugin":"rigel-hello"}
plug → {"type":"ready","protocol":1}
host → {"type":"call","id":1,"tool":"hello","arguments":{"name":"Rigel"}}
plug → {"type":"capability","id":2,"capability":"fs.read","path":"plugin.json"}
host → {"type":"capability_result","id":2,"ok":true,"content":"…"}
plug → {"type":"result","id":1,"ok":true,"content":"Hello, Rigel."}
```

A tool result can carry a string or any JSON value. A protocol failure, a missing handshake, a
non-JSON line, more than 256 messages or a call over 30 seconds is an error; the child is killed
and the turn continues with an error result. Stdout is the protocol; stderr is dropped, because
debugging output is not a transcript line and an inherited pipe can block the child.

## Capabilities

The broker is the only path from a plugin to a file through Rigel:

| Request | Grant | Behaviour |
|---|---|---|
| `fs.read` | `capabilities.filesystem.read` | Canonicalise the target; refuse unless it is under a declared root |
| `fs.write` | `capabilities.filesystem.write` | Same, with the parent canonicalised for a new file; never creates directories |
| `env.get` | `capabilities.env` | Return a named variable only when it is declared |
| anything else | — | `CAPABILITY_DENIED` with the unknown name |

Roots are relative to the plugin directory unless absolute. A refusal is an error object and
never carries bytes. The tests in `crates/rigel-core/tests/plugins.rs` and the unit tests in
`plugin::host` prove:

- a read outside the declared root is refused without opening the file;
- a plugin without a filesystem grant cannot read a file even when it asks;
- a real child process that asks for `../secret.txt` receives `CAPABILITY_DENIED` and not the
  secret;
- a declared root permits its own children and nothing else.

**The honest limit:** this boundary covers requests the plugin makes to the host. A subprocess
that calls the operating system directly is outside the broker. `network` and `process` are
declarations shown on the install card, not intercepted calls. Use the existing
[`sandbox`](../crates/rigel-core/src/sandbox/mod.rs) profiles for OS containment where the
platform supports it; Windows Job Objects do not restrict files or network, macOS
`sandbox-exec` is deprecated, and Linux namespaces depend on usable user namespaces. A plugin is
reviewed before it is installed; it is not certified safe.

## Install, enable, call

```bash
rigel plugins browse                     # every catalogue entry, tiers and shelves
rigel plugins search headroom            # names, descriptions, capabilities
rigel plugins info skillevaluator        # one entry, with its licence and note
rigel plugins install rigel-hello        # bundled tree, no network
rigel plugins install ./my-plugin        # a local directory; validated before copying
rigel plugins validate ./my-plugin       # manifest and entry check, nothing installed
rigel plugins list                       # installed, tools, capabilities, broken trees
rigel plugins disable rigel-hello        # stop offering it; files and receipt stay
rigel plugins enable rigel-hello
rigel plugins run rigel-hello hello --args '{"name":"Rigel"}'
rigel plugins remove rigel-hello         # deletes the installed tree and receipt
```

Installs land in `<index dir>/plugins/<name>/`. A state file `<index dir>/plugins/state.json`
holds the disabled list and one receipt per plugin: version, source, tree SHA-256, the exact
`plugin.json` hash, pinned commit and timestamp. The manifest hash is checked at load: a plugin
that rewrites its own manifest to widen a grant is reported as broken and is not offered, until
a reinstall reviews the new capabilities. A broken tree can be removed through `plugins remove`
or the window even though its manifest no longer loads. Removing the state file never deletes a plugin;
deleting a plugin directory is reported as a broken install instead of disappearing from the list.

### Pinned installs

A catalogue plugin entry can name `repo` and a full 40-character `sha` (plus an optional
`subdir`). Install clones, fetches that exact commit, checks `rev-parse HEAD`, then validates and
copies the manifest's directory. A branch, tag or short SHA is refused
(`PluginError::Unpinned`). This is the same distribution rule as Hermes' catalogue: a listing is
a review of a tree, and "main today" is a different tree tomorrow. Automatic pinned installs need
`git` on PATH; the bundled plugin does not.

## The catalogue

`crates/rigel-core/src/plugin/catalog.json` is compiled in, so browse and search work offline.
A user can put a newer `catalog.json` at `<index dir>/plugins/catalog.json` without a release.
Each entry declares:

| Field | Meaning |
|---|---|
| `name` | The key passed to `plugins install` |
| `title`, `description` | What the card says |
| `kind` | `skill`, `plugin` or `external` (discovery only) |
| `tier` | `official`, `verified` or `community` |
| `category` | The shelf: Quality, Context, Protocol, Tokens, Hardware, Code search, Memory, 3D, Examples, Workflow, plus the upstream marketplace categories |
| `license`, `homepage` | Provenance, shown before install |
| `author`, `version` | Publisher and upstream version for a directory listing |
| `source` | Provenance of an external listing, such as `claude-plugins-official` |
| `repo`, `sha`, `subdir` | A pinned plugin tree |
| `bundled` | A tree compiled into this build |
| `capabilities` | What the entry is known to need, for the card |
| `skills` | Built-in skill ids a `kind: skill` entry installs |
| `install` | The upstream command for an external listing |
| `note` | The honest qualification: review scope, licence, or why it is evaluate-only |

The tiers mean review, not popularity. `official` is first-party. `verified` means the project,
licence and install path were checked and pinned on the catalogue's `updated` date. `community`
means listed for a reason but not endorsed — Cognee is explicitly evaluate-only, and Blender MCP
needs an add-on inside Blender before its server can answer.

The catalogue also carries more than 2,500 external marketplace listings from Anthropic's
official and community Claude Code directories and the MIT-licensed `wshobson/agents`
marketplace. Those entries are `kind: external`, `tier: community`, and discovery-only: each
card names the upstream plugin manager command and says that Rigel does not copy the tree into
its own plugin store. They are searchable offline because the listing metadata is compiled in;
installing one is still a click in the harness that owns it, not an implicit network fetch.

## The marketplace in the window

The rail's Shelf now carries **Skills**, **MCPs** and **Plugins** as three tabs over the same
surface. The Plugins tab is a list-and-detail view: search, kind, tier and category filters, the
full description, author, version, source, licence, capabilities, install command and review
note, plus the installed plugin rows, disable/enable and remove. Settings → Plugins keeps the
same data as a card grid with paged rendering, because the directory is thousands of rows.
Nothing installs on launch and nothing is enabled by default. Broken installs are shown with
their parse or manifest-hash error and a Remove action, so a tampered tree can be cleared
without leaving the page.

The production window is driven by `e2e/specs/plugins.spec.ts` through Tauri's real IPC bridge and
the bundled `rigel-hello` tree: it renders the first page of the expanded catalogue and the
first-party chips, narrows by search, tier and category, installs and checks the receipt and
tool list, disables and re-enables the plugin, tampers with the installed manifest to prove the
broken state and backend error reach the page, removes the broken tree, reinstalls, and removes
the healthy tree. `PluginStore` and
`Tools::specs` are also tested in `crates/rigel-core/tests/plugins.rs`, including a manifest
changed after install and `plugin_call` leaving the next turn's tool set when disabled.

## Agent access

`plugin_call` is one tool in the normal registry, so the project permission matrix, role presets,
per-agent overrides and read-only child rules all apply to it like any other tool. The tool
description carries the enabled tool names; a call names `plugin.tool`, runs through the broker,
and returns the plugin's content plus a text form. A spawned read-only child cannot call it.
Plugin output passes the same redaction filter as every other tool result.

## Built-in skills

Nine `SKILL.md` bodies ship in `skills/builtin/` and are compiled into the binary. They are
indexed at window launch and by `rigel skills builtin`, as `builtin:<slug>` rows, so the shelf,
palette, `skills_search`, `skill_load` and a session pin work with no network:

| Skill | Tool | Licence |
|---|---|---|
| `skillevaluator` | NVIDIA SkillEvaluator | Apache-2.0 |
| `headroom` | Headroom context compression | Apache-2.0 |
| `ag-ui` | AG-UI event vocabulary | MIT |
| `mcptoon` | names-only MCP manifest | Apache-2.0 |
| `llmfit` | GPU/VRAM model fit | MIT |
| `semble` | CPU semantic code search | MIT |
| `cognee-evaluate` | Cognee, evaluate only | Apache-2.0 |
| `blender-mcp` | Blender MCP | MIT |
| `superpowers` | Cross-agent brainstorming, planning, TDD, debugging and review workflow | MIT |

```bash
rigel skills builtin                 # seed the index and list the ids
rigel skills builtin --install       # also copy into ~/.agents/skills and ~/.claude/skills
rigel skills builtin --install --to ./skills-store
```

`--install` is explicit; launching the window never writes into a harness store. The copy is the
same reviewed body that was indexed, and it replaces a file only when the bytes differ.

### Quality gate

NVIDIA SkillEvaluator (Apache-2.0) was installed and run on the skills folder on 2026-11-22:

```bash
uv tool install --python 3.13 "skillevaluator @ git+https://github.com/NVIDIA/SkillEvaluator.git"
for dir in skills/builtin/*/; do skillevaluator quality-check "$dir"; done
```

All eight original skills pass the required checks with grade A: 90.0–96.2/100. `superpowers`
was added after that sweep and follows the same frontmatter and body rules. The remaining findings are
advisory rather than correctness problems — missing negative triggers, no troubleshooting
section, and no examples on some entries. They are listed in the turn that added the skills; this
file does not claim the findings are zero.

The scan is static. It does not prove an installed third-party command is harmless, and it does
not replace the command approval that runs when a skill tells the agent to execute something.

## Adding a plugin

1. Create a directory with `plugin.json` and the entry script.
2. Run `rigel plugins validate .`; fix every manifest error.
3. Install it with `rigel plugins install .` and call each tool with `rigel plugins run`.
4. Read the tree the receipt covers, then enable it.
5. For the catalogue, add one JSON entry with a real 40-character SHA and a licence. A listing
   without a pinned commit is not installable.

## Not in this version

- WASM plugins. The roadmap allows subprocess or WASM; only the subprocess path is implemented.
- Plugin hooks into the turn loop (`post_tool_call`, prompt sections). The manifest does not
  declare hooks, so nothing is silently ignored.
- A hosted registry, ratings, or automatic catalogue refresh. The catalogue is reviewed and
  compiled in; a local file can override it.
- OS-enforced plugin sandboxing. See the boundary above.
