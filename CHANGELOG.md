# Changelog

Every release, newest first. The heading is the version its `v*` tag publishes.

## 0.3.0 — 2026-11-22

The 0.3 release. It keeps the 0.2.0 feature set and ships the work completed for the 0.3 tree,
including the plugin marketplace, the grouped Settings window and the polished Rigel Paper.

- feat(ui): give the composer one amber active state instead of a green Build button, and move the changed-skill badge into the same accent family
- feat(ui): fold the Settings notes rail with a remembered preference and group the twenty pages under Agent, Guardrails, Workspace, Connections, Window and System
- feat(ui): refine the Rigel Paper with a warmer stock, a flagline, an amber masthead rule, a pull-quote prompt, section ornaments and a front-page archive
- refactor(ui): remove the Terminal surface, its rail tab, palette action, Settings page, xterm dependency and dead styles
- feat(plugins): add a subprocess plugin API with a validated manifest, a capability broker, a per-install tree receipt and an agent `plugin_call` that the permission matrix governs
- feat(plugins): add a curated offline catalogue and a Settings → Plugins marketplace with tiers, capability chips, pinned installs, enable/disable and broken-install reporting
- fix(plugins): pin the manifest hash in the receipt, keep a tampered install removable, and prove the marketplace in the real window with `e2e/specs/plugins.spec.ts`
- feat(skills): ship eight native skills (SkillEvaluator, Headroom, AG-UI, mcptoon, llmfit, Semble, Cognee evaluate-only, Blender MCP), indexed at launch and installable into the harness stores; all eight pass SkillEvaluator tier 1 with grade A
- feat(swarm): add Rigel, the Muse big boss, and let Swarm choose any chat model for the coordinator
- feat(concord): read and reply to Concord work from Settings, with awaiting/all filters and receipts
- test(window): drive the built app through open, turn, approval and diff with the native window runner
- test(core): rehearse every migration prefix and fuzz the patch, MCP, settings and skill parsers
- feat(diagnostics): keep the last thousand events in a bounded, redacted ring
- feat(agent): plan mode with one plan approval, steering, pause across restart and crash recovery
- feat(agent): a subagent DAG and write-mode children that merge only through a reviewed worktree
- feat(agent): route inspection, launch provider health, local and custom doors, aliases and a project routing policy
- feat(agent): context compaction, token estimation, tool-call repair, a prompt-section budget and per-step cost
- feat(agent): give each roster row its own model, provider and effort, with a role preset overriding it and the next turn's pair shown before send
- feat(agent): edit ordered network allow/deny rules, a default decision and disabled sessions in Settings → Network, enforced by every egress tool and `rigel network`
- feat(agent): cap egress per origin and per agent, pin DNS to the validated address, refuse private targets by default and record the contacted address on every request
- feat(core): add opt-in sandbox profiles for approved commands: Windows Job Objects, macOS sandbox-exec and Linux namespaces, each with a two-sided containment statement
- feat(agent): add a project tool permission matrix; roles narrow the project grant, can never widen it, and Settings → Permissions shows the effective grants for every role and agent
- feat(agent): let each agent narrow its role with a per-agent tool override, stored in the session and shown as checkboxes in the permission matrix; no override can widen the role or project grant
- fix(agent): clamp a spawned child's tools to its parent's effective grant at spawn, so a custom or writable parent cannot delegate a capability it lacks
- feat(mcp): pin each project server's version and tool schemas, and block added or changed tools until the schema diff is accepted
- feat(snapshot): record per-file hashes, verify them before restore, and prune only old orphan snapshots with `rigel snapshot prune`
- feat(agent): store one compact, provenance-carrying summary per turn and read it from compaction and palette search
- feat(agent): send Anthropic prompt-cache breakpoints and show provider-reported cache reads, writes and savings
- feat(memory): rank committed facts by recency, use and pin, with a non-decaying pin and the three factors in the inspector
- feat(sessions): name an untitled session with the cheapest working model, without delaying the turn and without replacing a manual rename
- feat(cli): package an unfinished session as a rules-preserving handoff prompt
- feat(adapters): generate and test a dated harness parity matrix
- feat(adapters): unify Claude, Codex, Muse and ACP events behind one tested renderer
- feat(security): pre-write secret scanning, an untrusted-content firewall, MCP transport hardening and a file jail
- feat(api): `rigel serve` exposes the coordinator over loopback HTTP with OpenAPI 3.1 and SSE
- feat(ui): the Rigel Paper home surface, recovery and subagent views, design tokens and accessibility audits
- feat(server,cms): add the Fastify API, Payload CMS and the Caddy/Postgres deployment runbook
- feat(web): refresh the public site, add the FAQ and features pages, robots.txt and sitemap.xml

## 0.2.0 — 2026-11-06

- fix(agent): make the picked provider the door the turn uses, via one core route resolver
- fix(agent): refuse API keys, service-account JSON and ID tokens as Vertex credentials before sending
- feat(agent): Free mode probes free chat rows and saves the winning pair; launch re-syncs rosters and skills
- feat(agent): per-project MCP server approval with a real `tools/list`, and per-tool grants (Q242)
- feat(agent): skill scripts are approved separately from the command grant (Q243)
- feat(agent): per-subagent step caps, pause/resume at a step boundary (Q245, Q254)
- feat(agent): blast-radius preview for destructive commands, and a project network policy (Q239, Q240)
- feat(agent): validate a provider key before writing it to the keychain (Q244)
- feat(memory): memory writes need an approver and show the exact path, mode and content (Q251)

## 0.1.7 — 2026-11-03

- test(agentchat): say why a launched wrapper wrote no report
- feat: finish the cloud, mail and window workflow
- feat: publish the catalogue, gate the contact form, add an OWASP review skill
- docs(skill): stop the OWASP review skill tripping the scanner
- feat: seed from the catalogue, pin raw fetches, and cut 0.1.5
- docs: what cloud agent runs cost, and who could pay for them
- feat(cloud): queue a run as a Firestore document
- feat(cloud): list and inspect queued runs
- feat(cloud): cancel and requeue a run, without claiming work
- fix(agent): make Vertex usable from the window, and send the model it shows
- test(agent): pin how the Vertex project is read off the cloud CLI
- feat(agent): add Meta's Muse provider and a headless Muse Code adapter
- chore(data): track the MCP catalogue the app embeds
- ci(release): gate both Windows bundles on the Rigel branding
- feat: complete the 0.1.6 window, room, browser and cloud surfaces
- vendor: add the Rigel b browser fork
- build: ship the Rigel b sidecar with the app
- feat: repair settings and harden the 0.1.7 release gate

## 0.1.6 — 2026-11-03

- feat(chat): rebuild the agents screen as a bots list and message threads
- feat(chat): draw room attachments in a bubble, through the core rather than the server origin
- feat(agent): add Meta Model API (Muse Spark) as a provider
- feat(chat): add Meta's Muse Code as a room harness when its CLI is installed
- feat(agent): run one headless Muse Code turn through muse exec --json, with items and no invented usage
- feat(agent): give the room Concord and AgentCAD MCP servers alongside obscura
- feat(browser): give each agent a named Space, batched actions and shadow/iframe snapshots
- feat(ui): add themed context menus across every surface
- feat(ui): put a themed turn-target picker beside the model selector
- feat(cloud): queue, list, inspect, cancel and retry agent runs
- fix(agent): read the Vertex project from the window and send the model it shows
- docs(cloud): record sponsored-run economics and the queue design

## 0.1.4 — 2026-10-27

- ci(release): refuse to publish until CI passes for the tag
- fix(release): hold the gate until every CI run on the commit finishes
- feat(bundle): brand the MSI and NSIS installers
- docs(platforms): say where the installer artwork comes from

## 0.1.3 — 2026-10-26

- feat(release): cut a release in one command
- fix(release): roll back a failed version bump
