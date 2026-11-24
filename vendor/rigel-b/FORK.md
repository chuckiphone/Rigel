# Rigel b

Rigel b is Rigel's fork of [Obscura](https://github.com/h4ckf0r0day/obscura), the headless browser
used by every agent in the workspace. The name is the planet in Rigel's system: one engine, one
identity, shipped beside the app instead of installed separately.

## What this fork changes

- The shipped CLI and worker are `rigel-b` and `rigel-b-worker`.
- `rigel-b mcp` accepts `--capture <FILE>`. After every successful page action it writes the
  active tab's exact PNG and a small JSON metadata file before replying, so Rigel's inspector can
  show the frame the harness actually produced rather than re-rendering the URL.
- `rigel-b mcp --space <NAME>` gives each agent an isolated Space. Tabs, cookies and web storage
  are scoped to that process and name; another Space cannot list, switch or close its tabs.
  `browser_space` reports the boundary and its background-only focus policy.
- `browser_run` executes up to 50 navigate/click/fill/type/wait/evaluate/tab actions in one MCP
  call. It is the compact code-style surface, without a round trip per action.
- `browser_snapshot` includes text from open shadow roots and live iframe documents. Shadow
  controls are reachable through `browser_evaluate` or `browser_run`; element refs remain scoped
  to the top-level document.
- MCP input is synthetic and page-scoped. The fork creates no native browser window, never moves
  the OS pointer, never injects system keyboard input, and cannot focus or switch a user tab.
- The release binary is bundled as a Tauri sidecar and resolved next to the app executable.
- The MCP server keeps the `obscura` config name so existing agent configs and tool names keep
  working while the binary and product are Rigel b. Rigel passes a session/agent-derived
  `--space` when it launches the server.

## Upstream

Forked from <https://github.com/h4ckf0r0day/obscura>. The upstream `LICENSE` is retained
verbatim. Obscura's documentation under `docs/` describes the engine and remains applicable;
references to `obscura` binaries in upstream docs map to `rigel-b` here.

The upstream repository is Apache-2.0. The Rigel application bundles a copy of the license in
`resources/rigel-b/LICENSE`.

## Rebuild the sidecar

From the Rigel repository root:

```bash
node scripts/build-rigel-b.mjs
```

The script defaults to the host target from `rustc -vV`, builds the release binary, and writes
`crates/rigel-app/binaries/rigel-b-<target-triple>[.exe]`, which is the name Tauri's
`externalBin` entry resolves on that platform. CI passes `--target <triple>` on a runner that
matches the target, because Tauri does not cross-compile installers; a macOS universal build
needs a separate `lipo` step. Use `node scripts/build-rigel-b.mjs --dry-run` to print the
command and destination without building.
