/**
 * The only place the renderer talks to Rust.
 *
 * In a browser (`npm run dev`) there is no Tauri host, so these fall back to empty data and
 * the shell says so instead of pretending. In the app every call goes through `invoke`, and
 * the Rust core remains the single owner of state (handoff §3.2, S2).
 *
 * Field names are snake_case because they are the Rust structs verbatim: no mapping layer to
 * drift out of sync, and the same JSON the CLI prints under `--json`.
 */

import { previewAgents, previewApprovals, previewMemoryFacts } from "./sessionMock";
import { ABOUT_LICENCES } from "./about-licences";

export interface Session {
  id: string;
  project_root: string;
  title: string | null;
  /** Roadmap 56: `manual` names never yield to a later cheap-model title. */
  title_source?: "manual" | "auto" | null;
  status: "active" | "idle" | "interrupted" | "ended";
  budget_cents: number | null;
  spend_cents: number;
  spend_micros: number;
  interventions: number;
  tasks_done: number;
  created_at: number;
  ended_at: number | null;
  /** D148: pinned sessions sort first in the Session pane's list. */
  pinned_at: number | null;
  /** D148: archived sessions are hidden by default, still searchable and still resumable. */
  archived_at: number | null;
  parent_session_id?: string | null;
  parent_turn_id?: string | null;
}

export interface ProjectInfo {
  root: string;
  name: string;
  db: string;
}

/** D147: one palette row per section. The window never re-ranks these; the core already did. */
/** D150: a kept line. `target_id` is an item id or a patch hunk id. */
export interface Bookmark {
  id: string;
  session_id: string;
  kind: "item" | "hunk";
  target_id: string;
  label: string;
  /** Roadmap 103: an optional one-line note the session list and Markdown export carry. */
  note?: string | null;
  created_at: number;
}

export interface PaletteSkill {
  id: string;
  name: string;
  description: string;
  grade: string | null;
  source: string;
}

export interface PaletteSession {
  id: string;
  title: string;
  status: string;
  created_at: number;
}

export interface PaletteTurn {
  session_id: string;
  turn_id: string;
  seq: number;
  role: string;
  harness: string;
  prompt: string;
  snippet: string;
  from_item: boolean;
  from_summary: boolean;
  created_at: number;
}

export interface PalettePatch {
  id: string;
  session_id: string;
  path: string;
  status: string;
  created_at: number;
}

export interface PaletteResults {
  skills: PaletteSkill[];
  sessions: PaletteSession[];
  turns: PaletteTurn[];
  patches: PalettePatch[];
}

export interface Settings {
  theme: string;
  default_budget_cents: number;
  /** One row of the Rust-side adapter table: `omniroute`, `openrouter`, `gemini`, `vertex`, … */
  provider: string;
  model: string;
  /**
   * Free mode: the saved provider/model are the free pair Free mode proved was answering.
   * Picking a model by hand clears it, so the composer never claims a free model it is not on.
   */
  free_mode: boolean;
  /** The GCP project Vertex is billed to, saved by the Providers page. */
  vertex_project: string;
  /** Separate reasoning effort: low, medium or high. */
  effort: string;
  /** D90: per-event desktop notifications; no sound, and a watched turn stays silent. */
  notify_approval: boolean;
  notify_turn_done: boolean;
  notify_turn_failed: boolean;
  /** The Terminal surface: xterm's font size and scrollback, and the shell to open. */
  terminal_font_size: number;
  terminal_scrollback: number;
  /** Empty means PowerShell on Windows and `$SHELL` elsewhere. */
  terminal_shell: string;
  /** Re-sync a skill source whose data is older than the stale window, in the background. */
  skills_auto_update: boolean;
  /** D159: 90, 100, 110 or 125; applied to the shell, never to the terminal's cell size. */
  ui_scale: number;
  /** D159: tighter rows in the transcript, list and diff. */
  compact_density: boolean;
  /** How many model steps one native turn may take. */
  max_steps: number;
  /**
   * Whether the model picker hides rows that cannot answer a chat turn.
   * Saved, so the choice survives a restart; defaults to on.
   */
  chat_models_only: boolean;
  /** Who leads a Swarm: `muse` splits it, `broadcast` sends the same text to every agent. */
  swarm_lead: string;
  /** Refine each draft with the cheap model and send it without the review card. */
  auto_refine: boolean;
  /** Publish every session after its turns finish; off means the button is the only way. */
  cloud_sync_sessions: boolean;
  /** Q290: an explicit high-contrast palette, independent of the OS preference. */
  high_contrast: boolean;
  /** Q283: local-time window ("HH:MM") in which desktop notifications stay silent. */
  quiet_hours_start: string;
  quiet_hours_end: string;
  /** Opt-in Gravity sponsored card on Home. Off by default; off means no request. */
  gravity_ads: boolean;
  /** Test ads: no billing and no metrics; they appear only when Gravity returns one. */
  gravity_test_ads: boolean;
}

/** One Concord delivery receipt. The timestamps are the source of truth for the page's pills. */
export interface ConcordReceipt {
  status?: string;
  ticks?: string;
  sent_at?: string | null;
  delivered_at?: string | null;
  read_at?: string | null;
  replied_at?: string | null;
  expires_at?: string | null;
  failed_at?: string | null;
  error_code?: string | null;
  message_id?: string;
}

/** A message as Concord returns it inside the API's cache. */
export interface ConcordMessage {
  messageId?: string;
  id?: string;
  message_id?: string;
  content?: string;
  senderAgentId?: string;
  recipientAgentId?: string;
  status?: string;
  createdAt?: string;
  deliveredAt?: string | null;
  readAt?: string | null;
  repliedAt?: string | null;
  expiresAt?: string | null;
  failedAt?: string | null;
  replyToMessageId?: string | null;
  deliveryMode?: string;
  errorCode?: string | null;
  errorDetail?: string | null;
  taskId?: string | null;
  receipt?: ConcordReceipt | null;
}

/** One cached row: the API's monotonic cursor and the message itself. */
export interface ConcordInboxMessage {
  seq: number;
  message: ConcordMessage;
}

/** The API poller's cache. `error` is the poller's last failure, not a refused request. */
export interface ConcordInbox {
  ok: boolean;
  agent: string;
  messages: ConcordInboxMessage[];
  retained: number;
  oldest: number;
  revision: number;
  checked_at: number;
  updated_at: number;
  error: string | null;
}

/** What `/v1/concord/reply` returns after it has built and sent the update_work call. */
export interface ConcordReply {
  ok: boolean;
  tool: string;
  message_id: string;
  to_agent_id: string | null;
  text: string;
  result: unknown;
}

/**
 * A sponsored card from Gravity, as the broker returns it. Never contains the API key; the
 * advertiser's artwork is optional and only https URLs are rendered.
 */
export interface GravityAd {
  adText: string;
  title: string;
  brandName: string;
  cta: string;
  url: string;
  /** The advertiser's small mark, preferred over the wider brand image. */
  favicon?: string;
  /** A wider brand image, used only when no favicon is present. */
  brandImage?: string;
  /** The tracked link. `url` is the raw landing page and must not be linked. */
  clickUrl: string;
  impUrl: string;
  placement: string;
  placement_id: string;
  /** A non-billed test fill; clicking opens `url`, not the test host in `clickUrl`. */
  test?: boolean;
}

export interface GravityMessage {
  role: string;
  content: string;
}

/** Forwarded on the ad request for fraud checks and the Device report. */
export interface GravityDevice {
  ua: string;
  ip: string;
  os: string;
}

/** A sample mark for `npm run dev`; real fills carry their own https artwork. */
const PREVIEW_AD_ART = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill="#16130c"/><path d="M16 6l8 10-8 10-8-10z" fill="#e9a23b"/><path d="M16 11l4 5-4 5-4-5z" fill="#fff7ef"/></svg>',
)}`;

/** A sample card for `npm run dev`; the real one comes from `gravity_ad`. */
const PREVIEW_AD: GravityAd = {
  adText: "Serverless Postgres that scales to zero. Start free in a minute.",
  title: "Ship a database this afternoon",
  brandName: "Sample sponsor",
  cta: "Learn more",
  url: "https://example.com",
  favicon: PREVIEW_AD_ART,
  clickUrl: "https://api.trygravity.ai/track/click?p=preview",
  impUrl: "",
  placement: "below_response",
  placement_id: "rigel-sponsor",
  test: true,
};

/** The browser preview's stand-in for `settings.toml`; `null` until the first save. */
let previewSettings: Settings | null = null;

/** The browser preview's saved door; the app replaces it with Rust's route_explain. */
const PREVIEW_ROUTE = { provider: "omniroute", model: "openai/gpt-5.6-sol-medium" };
/** Roadmap 29: the same three preferences the core resolves, so the preview cannot drift. */
const PREVIEW_POLICY: RoutingPolicy = {
  order: ["openai", "anthropic"],
  allow_fallbacks: false,
  quantizations: ["fp16"],
};
let previewCustomDoor: CustomDoorView | null = null;
const PREVIEW_ALIASES: Record<string, { provider: string; model: string }> = {
  fast: { provider: "openrouter", model: "openai/gpt-5-nano" },
  deep: { provider: "openrouter", model: "anthropic/claude-sonnet-4.5" },
  cheap: { provider: "openrouter", model: "google/gemini-2.5-flash-lite" },
};

/**
 * The settings a window starts from before Rust answers — also the shape a first run writes.
 * One copy, so a new field is added here and in Rust's `Settings::default`, not in every form.
 */
export function defaultSettings(): Settings {
  return {
    theme: "dark",
    default_budget_cents: 500,
    provider: "omniroute",
    model: "gpt-5.6-sol-medium",
    free_mode: false,
    vertex_project: "",
    effort: "medium",
    notify_approval: true,
    notify_turn_done: true,
    notify_turn_failed: true,
    terminal_font_size: 13,
    terminal_scrollback: 8_000,
    terminal_shell: "",
    skills_auto_update: true,
    max_steps: 10,
    chat_models_only: true,
    swarm_lead: "muse",
    auto_refine: false,
    cloud_sync_sessions: false,
    ui_scale: 100,
    compact_density: false,
    high_contrast: false,
    quiet_hours_start: "",
    quiet_hours_end: "",
    gravity_ads: false,
    gravity_test_ads: true,
  };
}

/** Roadmap 124: the local facts behind the About dialog. Nothing here identifies the machine. */
export interface AboutInfo {
  version: string;
  commit: string;
  licences: string[];
  components: number;
  update_channel: string;
  privacy: string;
}

/** The browser has no build stamp; it says so instead of inventing one. */
export const PREVIEW_ABOUT: AboutInfo = {
  version: "preview",
  commit: "preview-build",
  licences: [...ABOUT_LICENCES.expressions],
  components: ABOUT_LICENCES.components,
  update_channel: "signed GitHub releases/latest · preview is read-only",
  privacy:
    "No telemetry. Sessions and diagnostics stay on this machine until you copy or export them.",
};

/** The update switch, this build's version and whether a publisher key is present. */
export interface UpdateStatus {
  current: string;
  enabled: boolean;
  auto_install: boolean;
  manifest_url: string;
  key_present: boolean;
}

/** A signed manifest that verified, whether or not it is newer than this build. */
export interface UpdateCheck {
  current: string;
  offered: string;
  available: boolean;
  verified: boolean;
  url: string;
  sha256: string;
  notes: string | null;
  msi_url: string | null;
  msi_sha256: string | null;
}

/** One node in the read-only project graph: a file or a top-level symbol. */
export interface GraphNode {
  id: string;
  label: string;
  kind: "file" | "symbol" | "concept";
  path: string;
  line: number | null;
  language: string;
}

export interface GraphEdge {
  from: string;
  to: string;
  kind: "imports" | "defines" | "references";
}

/** The background indexer's last pass, used by the graph header. */
export interface GraphWatchStatus {
  project: string;
  updatedUnix: number;
  files: number;
  nodes: number;
  edges: number;
  revision: number;
  concepts: number;
  agent: string | null;
  error: string | null;
}

/** The bounded graph the core scanned; `truncated` means a cap was hit. */
export interface CodeGraph {
  root: string;
  nodes: GraphNode[];
  edges: GraphEdge[];
  files: number;
  truncated: boolean;
}

/** The Firebase project and account, as `rigel cloud status` reports them. No token is here. */
export interface CloudStatus {
  configured: boolean;
  project_id: string | null;
  signed_in: boolean;
  email: string | null;
  uid: string | null;
  expires_at: number | null;
  /**
   * Firebase's answer about the address. False when signed out, and for a session stored before
   * the check existed until `cloudCheckVerification` re-reads it.
   */
  email_verified: boolean;
  /**
   * The App Check attestation endpoint, when this machine sends App Check tokens. The client
   * credential itself never reaches the renderer; this only says whether one is stored.
   */
  app_check: { attest_url: string; client_token_present: boolean } | null;
  /** The local manifest's clock: when this machine last pushed and pulled the shelf. */
  last_push_at: number | null;
  last_pull_at: number | null;
  last_sync_at: number | null;
}

/** The AgentMail inbox, as `rigel mail status` reports it. The key itself is never here. */
export interface MailStatus {
  configured: boolean;
  inbox: string | null;
  key_present: boolean;
}

/** One conversation in the inbox. Bodies are fetched only when it is opened. */
export interface MailThreadSummary {
  thread_id: string;
  subject: string;
  timestamp: string | null;
  message_count: number;
  unread: boolean;
}

/** One message inside an opened conversation. */
export interface MailThreadMessage {
  message_id: string;
  from: string;
  to: string[];
  subject: string;
  timestamp: string | null;
  text: string;
  extracted_text: string | null;
}

/** One conversation, oldest message first. */
export interface MailThread {
  thread_id: string;
  subject: string;
  messages: MailThreadMessage[];
}

/** What a send produced. */
export interface SentMail {
  message_id: string;
  thread_id: string | null;
}

/** What a push sent. Bodies never leave the machine. */
export interface CloudPushReport {
  project_id: string;
  uid: string;
  pushed: number;
  batches: number;
  /** Rows in the manifest that are gone locally; each became a tombstone. */
  deleted: number;
  /** The shelf exceeded the mirror cap, so deletions were not propagated. */
  truncated: boolean;
}

/** What a publish sent: the header and one document per turn. */
export interface CloudPublishReport {
  project_id: string;
  uid: string;
  session_id: string;
  title: string;
  turns: number;
  batches: number;
}

/** What publishing every session in a project sent, and what it could not. */
export interface CloudPublishAllReport {
  project_id: string;
  uid: string;
  sessions: number;
  turns: number;
  failed: string[];
}

/** What a pull found. A pulled row never carries a body. */
export interface CloudPullReport {
  project_id: string;
  uid: string;
  received: number;
  written: number;
  skipped: number;
  pages: number;
  /** Tombstone documents seen. */
  tombstones: number;
  /** Local rows a tombstone actually removed. */
  deleted: number;
}

/** D82: an opt-in, local-only diagnostics bundle. Nothing leaves the machine. */
export interface DiagnosticsBundle {
  path: string;
  body: string;
}

/** Roadmap 116: the redacted local crash tail the reporter previews. */
export interface CrashReport {
  path: string;
  text: string;
}

/** Browser preview: a shaped fake key proves the reporter redacts before it shows anything. */
export const PREVIEW_CRASH: CrashReport = {
  path: "crash.log",
  text: [
    "2026-11-25 09:14:02 panic in coordinator: turn 7 died while calling the model",
    "request header Authorization: Bearer sk-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "state: paused turn 7 kept its messages and remaining budget",
  ].join("\n"),
};

/** D97: the exact title and body a pull request would use. */
export interface PullRequestPlan {
  title: string;
  body: string;
  branch: string;
  base: string;
}

/** D91: one project rule file the native preamble reads. */
export interface RuleSource {
  path: string;
  bytes: number;
  kind: "agents" | "claude" | "cursor" | string;
  truncated: boolean;
}

/** D89: a per-turn tree checkpoint. `turn_id` names the turn it followed. */
export interface Checkpoint {
  id: string;
  session_id: string;
  turn_id: string | null;
  tree_hash: string;
  created_at: number;
}

/** D95: one Markdown session bundle, with its JSON beside it. */
export interface SessionExport {
  markdown_path: string;
  json_path: string;
  markdown: string;
}

/** D98/roadmap 41: a session's plan phase and the tools the approved plan covers. */
export interface PlanState {
  session_id: string;
  mode: boolean;
  plan: string | null;
  tools: string[];
  proposed_at: number | null;
  approved_at: number | null;
  updated_at: number;
}

/** Roadmap 42: one node in a parent turn's subagent graph. */
export type SubagentState =
  | "pending"
  | "running"
  | "done"
  | "failed"
  | "blocked"
  | "cancelled";

export interface SubagentNode {
  id: string;
  role: string;
  depends_on: string[];
  state: SubagentState;
  detail?: string | null;
  /** Roadmap 44: 1 for a direct child, deeper for a nested generation. */
  depth?: number;
  /** The inherited shared tree allowance and what is left of it. */
  budget_micros?: number | null;
  spent_micros?: number;
  remaining_micros?: number | null;
}

export interface SubagentGraph {
  parent_turn_id: string;
  nodes: SubagentNode[];
}

/** D94: a saved role prompt, model/effort pair and tool allowance. */
export interface RolePreset {
  id: string;
  title: string;
  brief: string;
  model: string;
  effort: string;
  tools: string[];
}

/** Roadmap 66: the project ceiling and the effective role/agent grants. */
export interface PermissionRole {
  id: string;
  title: string;
  requested: string[];
  effective: string[];
  blocked_by_project: string[];
}

export interface PermissionAgent {
  id: string;
  /** The session that owns this agent, so an override can be saved from any open session. */
  session: string | null;
  role: string | null;
  role_title: string | null;
  /** Explicit narrowing below the role; null means the role alone decides. */
  tool_overrides: string[] | null;
  effective: string[];
  /** Tools the role and project would grant, but this agent's override removes. */
  narrowed_by_override: string[];
  source: string;
}

export interface PermissionMatrix {
  project_grant: string[];
  all_tools: string[];
  roles: PermissionRole[];
  agents: PermissionAgent[];
}

const PREVIEW_TOOLS = [
  "read_file",
  "list_files",
  "edit_file",
  "write_file",
  "skills_search",
  "skill_load",
  "budget",
  "propose_plan",
  "run_command",
  "web_search",
  "web_fetch",
  "fetch_url",
  "browser_open",
  "roblox_studio",
  "roblox_sound",
  "email_list",
  "email_read",
  "email_send",
  "email_reply",
  "spawn_agents",
  "assign_agents",
  "merge_subagent",
  "subagent_results",
];

const PREVIEW_ROLE_TOOLS: Record<string, string[]> = {
  implementer: [
    "read_file",
    "list_files",
    "edit_file",
    "write_file",
    "run_command",
    "skills_search",
    "skill_load",
    "budget",
  ],
  reviewer: ["read_file", "list_files", "run_command", "skills_search", "skill_load", "budget"],
  tester: ["read_file", "list_files", "run_command", "budget"],
  researcher: [
    "read_file",
    "list_files",
    "web_search",
    "web_fetch",
    "fetch_url",
    "browser_open",
    "skills_search",
    "skill_load",
    "budget",
  ],
};

let previewPermissionGrant = [...PREVIEW_TOOLS];
const previewPermissionOverrides: Record<string, string[] | null> = {};

/** The browser preview has no project database or Tauri host; keep one matrix in memory. */
function previewPermissionMatrix(): PermissionMatrix {
  const project = new Set(previewPermissionGrant);
  const roles = [
    ["implementer", "Implementer"],
    ["reviewer", "Reviewer"],
    ["tester", "Tester"],
    ["researcher", "Researcher"],
  ].map(([id, title]) => {
    const requested = PREVIEW_ROLE_TOOLS[id];
    return {
      id,
      title,
      requested,
      effective: requested.filter((tool) => project.has(tool)),
      blocked_by_project: requested.filter((tool) => !project.has(tool)),
    };
  });
  const agents = previewAgents.map((agent) => {
    const role = roles.find((candidate) => candidate.id === agent.role);
    const requested = role ? role.requested : [...PREVIEW_TOOLS];
    const overrides = previewPermissionOverrides[agent.id] ?? null;
    const effective = requested.filter(
      (tool) => project.has(tool) && (overrides === null || overrides.includes(tool)),
    );
    return {
      id: agent.id,
      session: agent.session_id,
      role: agent.role ?? null,
      role_title: role?.title ?? null,
      tool_overrides: overrides,
      effective,
      narrowed_by_override: requested.filter(
        (tool) => project.has(tool) && overrides !== null && !overrides.includes(tool),
      ),
      source: role
        ? overrides
          ? `role preset \`${role.id}\` + per-agent override`
          : `role preset \`${role.id}\``
        : overrides
          ? "per-agent override"
          : "project grant",
    };
  });
  return {
    project_grant: [...previewPermissionGrant],
    all_tools: [...PREVIEW_TOOLS],
    roles,
    agents,
  };
}

/** Roadmap 61: the ordered rule and its default. */
export type NetworkAction = "allow" | "deny";
export type NetworkDefault = "ask" | "deny";

export interface NetworkRule {
  action: NetworkAction;
  pattern: string;
  note?: string | null;
}

export interface NetworkLimits {
  window_seconds: number;
  per_origin_requests: number;
  per_agent_requests: number;
  max_response_bytes: number;
  pin_dns: boolean;
  allow_private_addresses: boolean;
}

export interface NetworkPolicy {
  enabled: boolean;
  default: NetworkDefault;
  rules: NetworkRule[];
  limits: NetworkLimits;
  /** Legacy exact-origin list; the core converts it to allow rules on save. */
  allowlist?: string[];
  disabled_sessions: string[];
}

export interface NetworkDecision {
  allowed: boolean;
  code: string;
  source: string;
  reason: string;
}

export interface NetworkCheck {
  origin: string;
  decision: NetworkDecision;
}

/** D92: one row of the cross-session spend report. `micros` is the canonical unit. */
export interface SpendRow {
  key: string;
  label: string;
  micros: number;
  turns: number;
}

/** D92: every recorded turn, grouped by session, day and agent. */
export interface SpendReport {
  total_micros: number;
  turns: number;
  by_session: SpendRow[];
  by_agent: SpendRow[];
  by_day: SpendRow[];
  /** Roadmap 136: recorded cost grouped by the model the agent used. */
  by_model: SpendRow[];
}

/** Live state of the local OmniRoute router. `origin` is where it listens. */
export interface OmnirouteStatus {
  running: boolean;
  origin: string;
  base_url: string;
}

/** The local llama.cpp server behind the `llamacpp` door (`local_model_status`). */
export interface LocalModelStatus {
  /** `ready`, `loading` or `down`. */
  state: string;
  /** One sentence naming what happened and what to do about it. */
  detail: string;
  base_url: string;
  server: string | null;
  model: string | null;
  /** `pinned` when the weights fit in RAM, `mmap` when they are mapped instead. */
  mode: string;
  port: number;
  pid: number | null;
  configured: boolean;
  free_ram_gb: number | null;
  model_gb: number | null;
}

export interface Agent {
  id: string;
  session_id: string;
  harness: string;
  role: string | null;
  /** Roadmap 66: an optional per-agent filter below the role. */
  tool_overrides?: string[] | null;
  model: string | null;
  /** Roadmap 58: the door that serves `model`, when the roster picked the pair. */
  provider?: string | null;
  /** Roadmap 58: the row's own effort; absent falls back to the composer's. */
  effort?: string | null;
  status: "starting" | "running" | "idle" | "exited" | "failed" | string;
  started_at: number;
  spend_cents: number;
  spend_micros: number;
  tokens_in: number;
  tokens_out: number;
  /** Q267: live claims this agent holds, stale ones included. */
  claims_held: number;
  /** Q267: held claims whose heartbeat is older than the stale window. */
  claims_stale: number;
}

/// One file-scoped assignment on the session work board.
export interface TeamAgent {
  agent_key: string;
  role: string | null;
  kind: "native" | "room";
  files: string[];
  task: string;
  status: "assigned" | "working" | "done" | "failed" | string;
}

/** One factor of a memory fact's score (roadmap 55). */
export interface MemoryFactor {
  name: "recency" | "use" | "pin" | string;
  /** 0..1. */
  value: number;
  weight: number;
  /** `value × weight`; the factors sum to the score total. */
  contribution: number;
  reason: string;
}

export interface MemoryScore {
  total: number;
  method: string;
  /** The instant the score was computed, so a decay claim is reproducible. */
  now: number;
  factors: MemoryFactor[];
}

/** One ranked line of committed memory, with the arithmetic that placed it. */
export interface MemoryFact {
  id: string;
  agent_id: string;
  topic: string;
  text: string;
  pinned: boolean;
  uses: number;
  last_used_at: number | null;
  updated_at: number;
  score: MemoryScore;
}

export interface Usage {
  input_tokens: number;
  cached_input_tokens: number;
  /** Tokens the provider wrote into its prompt cache. Anthropic reports this; others send 0. */
  cache_write_tokens?: number;
  output_tokens: number;
  reasoning_tokens: number;
  /** Micro-dollars. This is the bill; cents are only for display. */
  cost_micros: number;
  provider_micros: number | null;
  /** The concrete model a router reports having served, when it differs from the requested id. */
  served_model?: string | null;
  source: "provider" | "estimated";
}

/** One request to the cheap model that tightens a draft before it is sent. */
export interface RefineRequest {
  draft: string;
  target: string;
  channel?: string | null;
  /** Recent room or turn messages, already flattened to `sender: text`. */
  recent: string[];
  /** The user's answers to the previous round's questions. */
  answers: string[];
  round: number;
}

/** Questions plus a proposed brief. Nothing here has been sent to an agent. */
export interface PromptRefinement {
  questions: string[];
  refined: string;
  reads: string[];
  read_notes: string[];
  notes: string[];
  usage: Usage;
  model: string;
}

export interface ModelOption {
  id: string;
  provider: "openrouter" | "omniroute" | "gemini" | string;
  name?: string | null;
  free: boolean;
  /** Whether this row can answer a chat turn; the picker hides the rest behind a toggle. */
  chat?: boolean;
  /** Whether the roster advertises tool calls; Free mode requires this for agent work. */
  tools?: boolean;
  /** A keyless model on this machine (roadmap 27): Ollama or llamafile. */
  local?: boolean;
  /** Completion price in micro-dollars per million tokens, when the provider publishes it. */
  output_micros_per_mtok?: number | null;
  /** The vendor's `input->output` description, when its roster publishes one. */
  modality?: string | null;
}

/** Item 15: one credentialed door, probed once at launch. */
export interface DoorHealth {
  provider: string;
  name: string;
  state: "ready" | "auth" | "rate_limited" | "unreachable";
  models: number | null;
  latency_ms: number | null;
  detail: string | null;
}

/** Roadmap 144: one declared MCP server's on-demand probe result. */
export interface McpServerHealth {
  name: string;
  state: "ready" | "failed" | "unsupported";
  latency_ms: number | null;
  protocol_version: string | null;
  tool_count: number;
  last_error: string | null;
}

/** Item 18: what one picker row can do, with the source and date of the facts. */
export interface ModelCapability {
  provider: string;
  model: string;
  tools: boolean | null;
  vision: boolean | null;
  context_tokens: number | null;
  max_output_tokens: number | null;
  streaming: boolean | null;
  output_micros_per_mtok: number | null;
  source: string;
  as_of: string;
}

/** The catalogue row's public facts, sent back for a capability lookup. */
export interface ModelRef {
  provider: string;
  model: string;
  modality?: string | null;
  output_micros_per_mtok?: number | null;
  chat?: boolean | null;
}

/** One free candidate and what its test message actually returned. */
export interface FreeAttempt {
  provider: string;
  model: string;
  /** ok, rate_limited, unauthorized, not_found, rejected, no_key, timeout, unreachable, … */
  status: string;
  detail?: string | null;
}

/** The free model Free mode settled on, with the evidence behind the choice. */
export interface FreeChoice {
  provider: string;
  model: string;
  note: string;
  /** The checked-in aggregate benchmark retained as supporting evidence. */
  benchmark: number;
  /** Antigravity's model id, or the deterministic benchmark fallback. */
  brain: string;
  attempts: FreeAttempt[];
}

/** Q268: one explicit local test run. `output` is already bounded by Rust. */
export interface TestRun {
  command: string;
  ok: boolean;
  exit_code: number | null;
  passed: number;
  failed: number;
  output: string;
  truncated: boolean;
  timed_out: boolean;
  duration_ms: number;
}

export interface Turn {
  id: string;
  agent_id: string;
  /** Set when a subagent ran this turn on behalf of a parent turn. */
  parent_turn_id: string | null;
  seq: number;
  status: "running" | "completed" | "failed" | "cancelled" | string;
  prompt: string | null;
  /** Image parts persisted with the first user message; omitted on text-only turns. */
  images?: { url: string }[];
  /** Roadmap 82: the mode the turn ran under. Empty on rows recorded before safe mode. */
  mode?: 'normal' | 'safe';
  started_at: number;
  completed_at: number | null;
  usage: Usage | null;
  stop_reason: string | null;
}

export interface Item {
  id: string;
  turn_id: string;
  seq: number;
  kind: "message" | "reasoning" | "command" | "file_change" | "mcp_call" | "web_search" | "plan" | "approval" | "usage" | "error" | "note" | string;
  status: string;
  body: Record<string, unknown>;
  created_at: number;
  raw?: unknown;
}

export interface TurnTranscript {
  turn: Turn;
  items: Item[];
}

/** Roadmap 37: the turns a dead process left running, named individually. */
export interface RecoveryOffer {
  session_id: string;
  turns: Turn[];
}

/** A fresh Rigel b render of a URL an external harness visited. */
export interface BrowserCapture {
  image: string;
  url: string;
  bytes: number;
}

/** The exact frame a harness's Rigel b MCP server wrote after its last page action. */
export interface BrowserHarnessCapture {
  image: string;
  url: string | null;
  tool: string | null;
  captured_at: number;
  bytes: number;
}

/**
 * D113/D116: the safety knobs and today's ledger. The window reads this in Settings and
 * after a raise; the cap itself is enforced in the core, not here.
 */
/** Roadmap 82: one session's safe mode and the phrase required to leave it. */
export interface SafeModeView {
  mode: 'normal' | 'safe';
  leave_phrase: string;
}

export interface SafetyView {
  writable_agents: number;
  daily_cap_micros: number | null;
  day: string;
  today_micros: number;
}

export interface BudgetSnapshot {
  session_id: string;
  agent_id: string;
  session_micros: number;
  agent_micros: number;
  cap_micros: number | null;
  remaining_micros: number | null;
  session_cents: number;
  agent_cents: number;
  cap_cents: number | null;
  remaining_cents: number | null;
}

export interface Patch {
  id: string;
  session_id: string;
  agent_id: string;
  path: string;
  base_hash: string;
  result_hash: string;
  diff: string;
  status: "proposed" | "applied" | "reverted" | "conflicted" | string;
  created_at: number;
  applied_at: number | null;
  reverted_at: number | null;
}

/** One `@@` block and the agent whose patch proposed it (P5.6). */
/** D154: one unresolved row. `kind` names why it is here, never a guess at a cause. */
export interface AttentionItem {
  kind: string;
  /** Empty on an approval from a surface that recorded no session. */
  session_id: string;
  label: string;
  detail: string;
  at: number;
  turn_id: string | null;
  agent_id: string | null;
  patch_id: string | null;
}

export interface AttentionGroup {
  session_id: string;
  title: string;
  status: string;
  archived: boolean;
  items: AttentionItem[];
}

export interface Attention {
  groups: AttentionGroup[];
  total: number;
}

/** D152: one agent a project template spawns, with its own model (D135). */
export interface TemplateAgent {
  harness: string;
  role: string | null;
  model: string | null;
}

/** Q270: one skill fixed to a session at a body hash. */
export interface SkillPin {
  skill_id: string;
  body_sha: string;
  pinned_at: number;
  /** D141: the hash currently on the shelf, when the row still exists. */
  current_body_sha: string | null;
  /** The index moved under this pin; the session still reads `body_sha`. */
  changed: boolean;
  pinned_body_available: boolean;
  current_body_available: boolean;
}

/** D141: the two bodies behind a changed pin, read only when the review card opens. */
export interface SkillPinReview {
  pin: SkillPin;
  pinned_body: string | null;
  current_body: string | null;
}

/** Q270: the version snapshot a session template carries. */
export interface TemplateSkill {
  skill_id: string;
  body_sha: string;
}

/** D152: the shape of a session before it starts, stored in `.rigel/templates.json`. */
export interface SessionTemplate {
  id: string;
  name: string;
  model: string | null;
  effort: string | null;
  max_steps: number | null;
  agents: TemplateAgent[];
  /** Pinned skill bodies restored when a session starts from this template. */
  skills: TemplateSkill[];
  /** Left in the composer by a create; never sent. */
  prompt: string;
  created_at: number;
}

/** Roadmap 94: persisted template extensions with a reviewable save diff. */
export interface TemplateModel extends SessionTemplate {
  variables: Record<string, string>;
  slots: string[];
  agent_sections: { agent_id: string; section: string }[];
  variable_defs: { name: string; default: string | null }[];
}

export interface TemplateFieldDiff {
  field: string;
  old: string | null;
  new: string | null;
}

/** D156: the recorded chain behind a patch. Empty links are printed as "unknown". */
export interface Provenance {
  patch_id: string;
  path: string;
  status: string;
  created_at: number;
  applied_at: number | null;
  agent_id: string;
  agent_role: string | null;
  harness: string | null;
  model: string | null;
  turn_id: string | null;
  turn_seq: number | null;
  turn_status: string | null;
  item_id: string | null;
  item_kind: string | null;
  tool: string | null;
  complete: boolean;
}

export interface AttributedHunk {
  /** The `patch_hunks` row: what a comment attaches to. */
  id: string;
  patch_id: string;
  path: string;
  ordinal: number;
  agent_id: string;
  header: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  created_at: number;
  status: string;
}

/** A comment on one hunk, addressed to the agent that wrote it (D88). */
export interface HunkComment {
  id: string;
  hunk_id: string;
  patch_id: string;
  path: string;
  header: string;
  /** The hunk's author: who the comment is for. */
  agent_id: string;
  /** `user`, or the id of the agent that wrote it. */
  author: string;
  body: string;
  created_at: number;
  delivered_turn_id: string | null;
  delivered_at: number | null;
}

export interface Recovery {
  patch_id: string;
  path: string;
  state: "applied" | "pending" | "conflicted";
}

export interface ResumeOutcome {
  session: Session;
  clean: boolean;
  tree_hash: string;
  checkpoint: { id: string; tree_hash: string; created_at: number } | null;
  closed_turns: Turn[];
  patches: Recovery[];
  reconcile: {
    session_id: string;
    session_recorded_micros: number;
    session_summed_micros: number;
    ok: boolean;
  };
}

export interface Route {
  provider: string;
  model: string;
}

export type RouteReason = "picker" | "model-only" | "bare-id-repair" | "saved" | "alias";

/** The core resolver's own answer, field for field (`agent/routing.rs`). */
export interface RouteExplanation {
  saved: Route;
  requested: Route | null;
  resolved: Route;
  reason: RouteReason;
  /** One sentence naming what was asked for and what won, when they differ. */
  mismatch: string | null;
}

export interface RoutingPolicy {
  order: string[];
  allow_fallbacks: boolean;
  quantizations: string[];
}

export interface CustomHeader {
  name: string;
  value: string;
}

/** What the renderer may see after a save: no header value ever leaves the keychain. */
export interface CustomDoorView {
  name: string;
  base_url: string;
  header_names: string[];
  models: string[];
}

export interface TurnReply {
  turn: Turn;
  budget_cents: number | null;
  /** The route the turn actually took; the inspector reads the same struct. */
  route: RouteExplanation;
}

export interface E2eConfig {
  project: string;
}

/**
 * A pre-turn estimate from Rust: what the first model call should cost, and the headroom.
 * Advisory — `reserve` and the cap remain the authority.
 */
export interface TurnQuote {
  model: string;
  input_tokens: number;
  output_tokens: number;
  first_step_micros: number;
  spend_micros: number;
  cap_micros: number | null;
  remaining_micros: number | null;
  over_remaining: boolean;
  reserve_micros: number;
  /** When the local price table was read, how old it is, and where it came from. */
  price_fetched_on: string;
  price_age_days: number;
  price_source: string;
  price_stale: boolean;
}

/**
 * Browser-preview prices only. The desktop app asks Rust, where `agent/pricing.rs` owns the
 * table; these numbers exist so the composer can be inspected without a Tauri host.
 */
const PREVIEW_PRICE = { input: 3, output: 15 }; // anthropic/claude-sonnet-4.5
const PREVIEW_CONTEXT_CHARS = 1_800; // system prompt + tool schemas, roughly

function previewQuote(prompt: string, spendMicros: number, capMicros: number | null): TurnQuote {
  const inputTokens = Math.ceil((PREVIEW_CONTEXT_CHARS + prompt.length) / 4);
  const outputTokens = Math.min(2_048, Math.max(256, Math.floor(inputTokens / 2)));
  const firstStep = Math.round(inputTokens * PREVIEW_PRICE.input + outputTokens * PREVIEW_PRICE.output);
  const remaining = capMicros === null ? null : capMicros - spendMicros;
  return {
    model: "anthropic/claude-sonnet-4.5",
    input_tokens: inputTokens,
    output_tokens: outputTokens,
    first_step_micros: firstStep,
    spend_micros: spendMicros,
    cap_micros: capMicros,
    remaining_micros: remaining,
    over_remaining: remaining !== null && firstStep > remaining,
    reserve_micros: 10_000,
    // The preview's price copy is deliberately older than the real table so the stale state is
    // part of what the browser suite can see; the desktop app asks Rust for the real dates.
    price_fetched_on: "2026-06-01",
    price_age_days: Math.max(
      0,
      Math.floor((Date.now() - Date.parse("2026-06-01T00:00:00Z")) / 86_400_000),
    ),
    price_source: "vendor price pages (browser preview copy)",
    price_stale: true,
  };
}

/** Roadmap 144: one answering server and one failed handshake, so both rows can be inspected. */
const PREVIEW_MCP_HEALTH: McpServerHealth[] = [
  {
    name: "filesystem",
    state: "ready",
    latency_ms: 64,
    protocol_version: "2025-06-18",
    tool_count: 11,
    last_error: null,
  },
  {
    name: "broken-server",
    state: "failed",
    latency_ms: 37,
    protocol_version: null,
    tool_count: 0,
    last_error: "broken-server did not answer in time",
  },
];

/** Item 15: the preview's launch pass — one answering door and one refused key. */
const PREVIEW_HEALTH: DoorHealth[] = [
  {
    provider: "openrouter",
    name: "OpenRouter",
    state: "ready",
    models: 627,
    latency_ms: 118,
    detail: null,
  },
  {
    provider: "gemini",
    name: "Google AI Studio",
    state: "auth",
    models: null,
    latency_ms: 42,
    detail: "provider returned 401 Unauthorized",
  },
];

/** Item 18: the preview's capability rows — one known model and unknown facts for the rest. */
function previewModelCapabilities(models: ModelRef[]): ModelCapability[] {
  return models.map((model) => {
    const known = model.model === "anthropic/claude-sonnet-4.5";
    return {
      provider: model.provider,
      model: model.model,
      tools: known ? true : null,
      vision: model.modality
        ? model.modality.toLowerCase().includes("image")
        : null,
      context_tokens: known ? 200_000 : null,
      max_output_tokens: known ? 64_000 : null,
      streaming: model.chat === false ? null : true,
      output_micros_per_mtok: model.output_micros_per_mtok ?? null,
      source: known
        ? "provider model pages"
        : "no published capability row in this build",
      as_of: "2026-11-05",
    };
  });
}

/** Browser-preview attention: one session group plus a waiting approval. */
function previewAttention(): Attention {
  const now = Date.now();
  return {
    total: 3,
    groups: [
      {
        session_id: "",
        title: "Waiting on you",
        status: "active",
        archived: false,
        items: [
          {
            kind: "approval",
            session_id: "7c1e4a3d-0000-4000-8000-000000000000",
            label: "cargo test --workspace",
            detail: "command · waiting for an answer",
            at: now - 60_000,
            turn_id: "turn-preview-2",
            agent_id: "agent-preview-worker",
            patch_id: null,
          },
        ],
      },
      {
        session_id: "7c1e4a3d-0000-4000-8000-000000000000",
        title: "Fix the flaky resume test",
        status: "active",
        archived: false,
        items: [
          {
            kind: "blocked_merge",
            session_id: "7c1e4a3d-0000-4000-8000-000000000000",
            label: "Blocked merge from writer",
            detail: "blocked · 1 file · the caller asked for exactly this comment",
            at: now - 240_000,
            turn_id: null,
            agent_id: "c4d18e77-2b90-4a51-8f0e-6d3a9b5c7e21",
            patch_id: null,
          },
          {
            kind: "failed_turn",
            session_id: "7c1e4a3d-0000-4000-8000-000000000000",
            label: "Turn 2 failed",
            detail: "writer · no later turn on this agent completed",
            at: now - 300_000,
            turn_id: "turn-preview-2",
            agent_id: "agent-preview-worker",
            patch_id: null,
          },
        ],
      },
    ],
  };
}

/** Browser-preview templates: one sample, kept in memory for the session. */
let previewTemplates: SessionTemplate[] | null = null;

const PREVIEW_SESSION_ID = "7c1e4a3d-0000-4000-8000-000000000000";
const PREVIEW_PDF_ID = "github:anthropics/skills:skills/pdf/SKILL.md";

/** A stable stand-in hash for the browser preview; real pins carry the index's SHA-256. */
function previewSha(value: string): string {
  let hash = 0;
  for (const char of value) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash.toString(16).padStart(8, "0").repeat(8).slice(0, 64);
}

function previewBodySha(id: string): string {
  return previewSha(id);
}

const PREVIEW_PDF_OLD_BODY =
  "---\nname: pdf-reader\ndescription: Read scanned PDFs and receipts.\nlicense: MIT\n---\n# PDF reader\n\nAsk the agent for the file, then summarise it.\n";
const PREVIEW_PDF_CURRENT_BODY =
  "---\nname: pdf-reader\ndescription: Read scanned PDFs and receipts.\nlicense: MIT\n---\n# PDF reader\n\nAsk the agent for the file, wait for the scan, then summarise it.\n";

/** Browser-preview pins: one sample, kept in memory for the session. */
let previewPins: Record<string, SkillPin[]> = {
  [PREVIEW_SESSION_ID]: [
    {
      skill_id: PREVIEW_PDF_ID,
      body_sha: previewSha(PREVIEW_PDF_OLD_BODY),
      pinned_at: Date.now() - 240_000,
      current_body_sha: previewSha(PREVIEW_PDF_CURRENT_BODY),
      changed: true,
      pinned_body_available: true,
      current_body_available: true,
    },
  ],
};

function previewTemplateList(): SessionTemplate[] {
  previewTemplates ??= [
    {
      id: "template-preview-1",
      name: "Fix the flaky test",
      model: "anthropic/claude-sonnet-4.5",
      effort: "medium",
      max_steps: 10,
      agents: [{ harness: "native", role: "writer", model: "anthropic/claude-sonnet-4.5" }],
      skills: [{ skill_id: PREVIEW_PDF_ID, body_sha: previewSha(PREVIEW_PDF_OLD_BODY) }],
      prompt: "The resume test fails on Windows. Find the cause before patching it.",
      created_at: Date.now() - 600_000,
    },
  ];
  return previewTemplates;
}

function previewTemplateSave(template: SessionTemplate): SessionTemplate {
  const list = previewTemplateList();
  const saved: SessionTemplate = {
    ...template,
    id: template.id || `template-preview-${Date.now()}`,
    name: template.name.split("\n")[0].trim().slice(0, 60) || "Session",
    prompt: template.prompt.trimEnd(),
    created_at: template.created_at || Date.now(),
  };
  const index = list.findIndex((row) => row.id === saved.id);
  previewTemplates = index >= 0 ? list.map((row) => (row.id === saved.id ? saved : row)) : [...list, saved];
  return saved;
}

/**
 * Browser-preview provenance only: the chain the sample diff and transcript share. The desktop
 * app reads the same shape from `provenance.rs`.
 */
function previewProvenance(patchId: string): Provenance {
  if (patchId !== "patch-preview-1") {
    throw new Error(`patch ${patchId} is not in the journal`);
  }
  return {
    patch_id: patchId,
    path: "ui/src/session.tsx",
    status: "applied",
    created_at: Date.now() - 120_000,
    applied_at: Date.now() - 120_000,
    agent_id: "agent-worker",
    agent_role: "worker",
    harness: "native",
    model: "anthropic/claude-sonnet-4.5",
    turn_id: "turn-preview-2",
    turn_seq: 2,
    turn_status: "completed",
    item_id: "item-preview-5",
    item_kind: "file_change",
    tool: "edit_file",
    complete: true,
  };
}

export interface KeyStatus {
  /** `true` per credential name: one entry per provider and one per harness key. */
  providers: Record<string, boolean>;
  /** Whether any chat provider has a credential — the answer the launch banner asks for. */
  native_ready: boolean;
}

/** Q244: what a candidate provider key proved before it was stored. */
export interface KeyValidation {
  /** Whether the provider answered a request carrying the candidate key. */
  checked: boolean;
  /** Models the roster returned; zero when no check was possible. */
  models: number;
  /** One sentence for the Settings notice. */
  note: string;
}

export interface KeySetResult {
  status: KeyStatus;
  /** `null` for a harness credential; a chat provider always carries a result. */
  check: KeyValidation | null;
}

/** One chat adapter, from the Rust-side table (`providers_list`). Never a key, only its absence. */
export interface ProviderRow {
  id: string;
  name: string;
  hint: string;
  /** The variable a headless run reads instead of the keychain. */
  env: string | null;
  /** A short-lived token rather than a key: the field's label and copy follow this. */
  access_token: boolean;
  default_model: string | null;
  /** Where a key is created, when the vendor publishes a page for it. */
  key_url: string | null;
  key_present: boolean;
  /** A local door: no credential exists, so "no key" would be the wrong sentence. */
  keyless: boolean;
}

/** Health of the vendor chat server (agentchattr) the Chat tab fronts. */
export interface AgentChatStatus {
  running: boolean;
  /** True when Rust holds a session token for it — everything past the landing page needs one. */
  authed: boolean;
  origin: string;
  /** The checkout on this machine, when one was found. */
  dir: string | null;
  /** Install instructions, when no checkout exists. The core owns the copy. */
  setup: string | null;
}

/** One file a room message carries. `url` is a room-relative `/uploads/…` path. */
export interface AgentChatAttachment {
  name: string;
  url: string;
}

/** The vendor store's message shape, narrowed to what the stream renders. */
export interface AgentChatMessage {
  id: number;
  sender: string;
  text: string;
  time: string;
  kind: string;
  channel: string;
  attachments?: AgentChatAttachment[];
}

/** One roster pill: agentchattr presence plus its own label/colour/role fields. */
export interface AgentChatAgent {
  available: boolean;
  busy: boolean;
  label: string;
  color: string;
  role: string | null;
}

/**
 * The API placement the `rigel` model door and the relay bridge use. This is separate from
 * Firebase Cloud sync: it is the Rigel API itself, hosted or self-hosted.
 */
export interface ApiBackendStatus {
  mode: "hosted" | "self_hosted";
  api_url: string;
  effective_url: string;
  hosted_url: string;
  /** Set when `RIGEL_API_URL` (or the legacy variable) is overriding the saved choice. */
  override_env: string | null;
  relay_token_present: boolean;
}

/** One harness process Rigel started for the room, so the roster can offer a Stop. */
export interface AgentChatProcess {
  name: string;
  label: string;
  pid: number;
}

/** One agent a swarm can be sent to, as offered in the preflight. */
export interface SwarmTarget {
  name: string;
  label: string;
  role: string | null;
}

/**
 * Who leads a Swarm. `rigel` is the Muse coordinator; `model` asks that same coordinator to
 * think with a particular model; `broadcast` has no boss and sends the task to every agent.
 */
export type SwarmLead =
  | { kind: "rigel" }
  | { kind: "broadcast" }
  | { kind: "model"; provider: string; model: string };

/**
 * One harness the chat checkout configures. `ready` is true only after a real prompt probe
 * succeeds; `status` distinguishes a missing CLI, a missing API key and a CLI that needs login.
 */
export interface AgentChatHarness {
  name: string;
  label: string;
  api: boolean;
  ready: boolean;
  status:
    | "unknown"
    | "usable"
    | "missing"
    | "missing_key"
    | "unauthenticated"
    | "probe_timeout"
    | "provider_blocked"
    | "probe_failed";
  /** The exact CLI line or probe outcome behind the status. */
  evidence: string;
  remediation: string | null;
  credential: string | null;
}

/** Browser preview only: the shape of a live room, from this project's recorded run. */
const PREVIEW_CHAT_MESSAGES: AgentChatMessage[] = [
  { id: 1, sender: "user", text: "@claude @codex Review crates/rigel-cli/src/phase3.rs — one improvement each, then debate.", time: "21:18:02", kind: "chat", channel: "general" },
  {
    id: 2,
    sender: "claude",
    text: "PatchCommand variants carry no help text while AgentCommand's do — add doc comments.",
    time: "21:18:41",
    kind: "chat",
    channel: "general",
    attachments: [{ name: "phase3-diff.png", url: "/uploads/phase3-diff.png" }],
  },
  { id: 3, sender: "codex", text: "--max-steps 0 is accepted, then the coordinator fails on the step bound at once — validate in Clap.", time: "21:19:03", kind: "chat", channel: "general" },
  { id: 4, sender: "claude", text: "@codex Agreed — your fix lands first; mine is docs-only follow-up. This enum has acp, not register.", time: "21:19:30", kind: "chat", channel: "general" },
];

/** The adapters a browser preview shows; the real list comes from the Rust-side table. */
const PREVIEW_PROVIDERS: ProviderRow[] = [
  {
    id: "omniroute",
    name: "OmniRoute",
    hint: "Local router · one key, any model",
    env: "OMNIROUTE_API_KEY",
    access_token: false,
    default_model: "gpt-5.6-sol-medium",
    key_url: null,
    key_present: false,
    keyless: false,
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    hint: "Hosted gateway · the widest roster",
    env: "OPENROUTER_API_KEY",
    access_token: false,
    default_model: "openai/gpt-4.1-mini",
    key_url: "https://openrouter.ai/keys",
    key_present: false,
    keyless: false,
  },
  {
    id: "llamacpp",
    name: "llama.cpp (local)",
    hint: "This machine · no key · llama-server on 8080",
    env: "RIGEL_LLAMACPP_BASE_URL",
    access_token: false,
    default_model: null,
    key_url: null,
    key_present: false,
    keyless: true,
  },
  {
    id: "vertex",
    name: "Google Vertex AI",
    hint: "Your GCP project · a short-lived access token",
    env: "VERTEX_ACCESS_TOKEN",
    access_token: true,
    default_model: "google/gemini-4-argon",
    key_url: "https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/start/openai",
    key_present: false,
    keyless: false,
  },
  {
    id: "anthropic",
    name: "Anthropic",
    hint: "Claude's own endpoint · the Messages API",
    env: "ANTHROPIC_API_KEY",
    access_token: false,
    default_model: "claude-opus-5-5",
    key_url: "https://platform.claude.com/settings/keys",
    key_present: false,
    keyless: false,
  },
];

/** One row of `adapters.json` (P4.1). */
export interface HarnessInfo {
  harness: "native" | "codex" | "claude" | "muse" | "cursor" | "cline" | "gemini";
  installed: boolean;
  version: string | null;
  path: string | null;
  auth_mode: "api_key" | "subscription" | "none";
  capabilities: {
    acp: boolean;
    json_stream: boolean;
    programmatic_approval: boolean;
    worktree: boolean;
    nested_agents: boolean;
    usage: boolean;
    resume: "wired" | "vendor-only" | "unsupported";
    tools: boolean;
  };
  provider: string | null;
  install_hint: string | null;
}

/** Browser preview only: the shape of a real probe, with this project's recorded versions. */
const PREVIEW_HARNESSES: HarnessInfo[] = [
  {
    harness: "codex",
    installed: true,
    version: "0.162.1",
    path: "C:\\Program Files\\Codex\\codex.exe",
    auth_mode: "api_key",
    capabilities: { acp: false, json_stream: true, programmatic_approval: true, worktree: true, nested_agents: false, usage: true, resume: "vendor-only", tools: true },
    provider: "omniroute",
    install_hint: null,
  },
  {
    harness: "claude",
    installed: true,
    version: "2.1.251",
    path: "C:\\Users\\you\\.local\\bin\\claude.exe",
    auth_mode: "api_key",
    capabilities: { acp: false, json_stream: true, programmatic_approval: true, worktree: false, nested_agents: true, usage: true, resume: "vendor-only", tools: true },
    provider: "anthropic",
    install_hint: null,
  },
  {
    harness: "muse",
    installed: false,
    version: null,
    path: null,
    auth_mode: "none",
    capabilities: { acp: false, json_stream: true, programmatic_approval: false, worktree: false, nested_agents: false, usage: false, resume: "unsupported", tools: true },
    provider: "meta",
    install_hint: "powershell -NoProfile -Command \"irm https://dev.meta.ai/install.ps1 | iex\"",
  },
  {
    harness: "cursor",
    installed: false,
    version: null,
    path: null,
    auth_mode: "none",
    capabilities: { acp: true, json_stream: true, programmatic_approval: true, worktree: true, nested_agents: false, usage: true, resume: "unsupported", tools: true },
    provider: "cursor",
    install_hint: "powershell -NoProfile -Command \"irm 'https://cursor.com/install?win32=true' | iex\"",
  },
  {
    harness: "cline",
    installed: false,
    version: null,
    path: null,
    auth_mode: "none",
    capabilities: { acp: true, json_stream: true, programmatic_approval: true, worktree: true, nested_agents: false, usage: true, resume: "unsupported", tools: true },
    provider: null,
    install_hint: "npm install -g cline",
  },
  {
    harness: "gemini",
    installed: false,
    version: null,
    path: null,
    auth_mode: "none",
    capabilities: { acp: true, json_stream: true, programmatic_approval: true, worktree: false, nested_agents: false, usage: true, resume: "unsupported", tools: true },
    provider: "gemini",
    install_hint: "npm install -g @google/gemini-cli",
  },
];

/** The serialised `AgentEvent`; `type` is the discriminator. */
export type AgentEvent =
  | {
      type: "turn_started";
      session_id: string;
      agent_id: string;
      turn_id: string;
      parent_turn_id: string | null;
      seq: number;
      prompt: string | null;
    }
  | { type: "delta"; turn_id: string; kind: string; text: string }
  | { type: "item"; item: Item }
  | { type: "turn_finished"; turn_id: string; status: string; stop: string; usage: Usage; session_spend_micros: number; cap_micros: number | null }
  | { type: "budget_warning"; session_id: string; spend_micros: number; cap_micros: number; remaining_micros: number }
  | { type: "budget_exceeded"; session_id: string; spend_micros: number; cap_micros: number }
  | { type: "task_budget_exceeded"; session_id: string; agent_id: string; spend_micros: number; cap_micros: number }
  | { type: "daily_budget_warning"; day: string; spend_micros: number; cap_micros: number }
  | { type: "daily_budget_exceeded"; day: string; spend_micros: number; cap_micros: number }
  | { type: "approval_requested"; approval_id: string; kind: string; detail: unknown }
  | { type: "approval_resolved"; approval_id: string; decision: string }
  | { type: "session_updated"; session: Session }
  | { type: "error"; turn_id: string | null; message: string };

export type Grade = "A" | "B" | "C" | "D" | "F";

export interface Capability {
  label: string;
  severity: "critical" | "high" | "low";
  evidence: { file: string; line: number; code: string }[];
}

export interface SkillRow {
  id: string;
  name: string;
  description: string;
  source: string;
  repo: string | null;
  path: string | null;
  url: string | null;
  /** `null` means not scanned. The shelf prints that, never a guessed letter. */
  grade: Grade | null;
  grade_scope: string | null;
  stars: number | null;
  installs: number | null;
  tokens: number | null;
  cluster_id: string | null;
  /** Rows in this row's cluster, including itself. Never used to hide a row. */
  variants: number;
  last_seen_at: number | null;
  /** The description advertises a number nobody has measured (P2.7). */
  unmeasured: boolean;
  has_body: boolean;
}

export interface EvidenceRow {
  metric: string;
  measured: number | null;
  advertised: number | null;
  source: string;
  source_url: string | null;
  observed_at: number;
  /** "measured" or "unmeasured", derived in Rust from the row itself. */
  label: "measured" | "unmeasured";
}

export interface VariantRow {
  id: string;
  name: string;
  repo: string | null;
  grade: Grade | null;
  license: string | null;
  last_seen_at: number | null;
  delta: string;
}

export interface SkillDetail {
  skill: SkillRow;
  capabilities: Capability[];
  evidence: EvidenceRow[];
  variants: VariantRow[];
  first_seen_at: number | null;
}

export interface SkillInstallReport {
  name: string;
  directory: string;
  paths: string[];
  replaced: boolean;
  scan: SkillScan;
}

/** SkillSpector's verdict, carried by every download and install receipt. */
export interface SkillScan {
  score: number;
  severity: string;
  recommendation: string;
  findings: number;
  scanner: string;
}

/** One row from the embedded MCP registry catalogue. */
export interface McpServer {
  id: string;
  name: string;
  qualified: string;
  description: string;
  repo: string;
  homepage: string;
  source: string;
  sources: string[];
  transport: string;
  install: string;
  version: string;
  /** False when the catalogue has no launch command the app can write without guessing. */
  installable: boolean;
}

export interface McpInstallReport {
  id: string;
  name: string;
  entry: string;
  path: string;
  created: boolean;
}

export interface TrayAgent {
  id: string;
  name: string;
  harness: string;
  model: string | null;
  session: string;
  status: string;
  spend_micros: number;
  source: "session" | "chat" | string;
}

export interface TraySnapshot {
  project: string | null;
  project_name: string | null;
  sessions: number;
  agents_running: number;
  agents_hidden: number;
  spend_micros: number;
  cpu_percent: number;
  memory_bytes: number;
  power: string;
  power_note: string;
  agents: TrayAgent[];
  updated_at: number;
}

export interface SourceStatus {
  id: string;
  base_url: string | null;
  state: "ok" | "stale" | "broken";
  last_sync_at: number | null;
  /** When the background checker last attempted this source, successful or not. */
  last_attempt_at: number | null;
  /** Consecutive failures; the core uses it to space out retries. */
  failures: number;
  error: string | null;
}

/** One curated marketplace listing: a built-in skill or an installable plugin. */
export interface PluginCatalogEntry {
  name: string;
  title: string;
  kind: "skill" | "plugin" | "external";
  tier: "official" | "verified" | "community";
  category: string;
  description: string;
  license: string | null;
  homepage: string | null;
  /** Publisher named by an external marketplace listing. */
  author: string | null;
  /** Upstream plugin version, when the listing names one. */
  version: string | null;
  /** Where an external listing came from, e.g. `claude-plugins-official`. */
  source: string | null;
  repo: string | null;
  sha: string | null;
  subdir: string | null;
  bundled: string | null;
  capabilities: string[];
  skills: string[];
  install: string | null;
  note: string | null;
}

export interface PluginCatalog {
  entries: PluginCatalogEntry[];
  categories: string[];
  updated: string;
}

export interface PluginRow {
  name: string;
  version: string;
  description: string;
  enabled: boolean;
  tools: string[];
  capabilities: string[];
  source: string | null;
  license: string | null;
  installed_at: number | null;
}

export interface PluginsState {
  plugins: PluginRow[];
  broken: { name: string; error: string }[];
  enabled_tools: string[];
}

export interface IngestReport {
  source: string;
  repo: string | null;
  branch: string | null;
  found: number;
  added: number;
  updated: number;
  unchanged: number;
  with_body: number;
  http_requests: number;
  truncated: boolean;
  clusters: number;
  scan: SkillScan | null;
  blocked: number;
}

/** What the shelf needs from a source. `mock.ts` implements the same shape for previews. */
export interface SkillSource {
  readonly label: string;
  /** One page of the current query. `offset` pages through the whole catalogue. */
  search(query: string, limit: number, offset?: number): Promise<SkillRow[]>;
  /** How many rows the query matches — the whole shelf when it is empty. */
  count(query: string): Promise<number>;
  load(id: string): Promise<SkillDetail>;
  status(): Promise<SourceStatus[]>;
  indexRepo?(repo: string): Promise<IngestReport>;
  /** Re-sync every source that is not fresh; what the auto-update switch runs. */
  refresh?(): Promise<RefreshReport>;
  /** Copy this row's indexed body into the user-level harness stores. */
  install?(id: string): Promise<SkillInstallReport>;
  /** Whether any harness store already contains this row. */
  installed?(id: string): Promise<boolean>;
}

/** One source after an auto-update pass. */
export interface SourceRefresh {
  id: string;
  /** `ok` | `stale` | `broken` — the state the source is left in. */
  state: string;
  /** One line: what was fetched, or why the source was left alone. */
  detail: string;
}

/** What one auto-update pass did. */
export interface RefreshReport {
  checked: number;
  refreshed: number;
  sources: SourceRefresh[];
}


/** One question waiting for a human: a command, or a harness tool call (N2). */
export interface PendingApproval {
  id: string;
  /** The exact text a grant is keyed by, so a near-miss asks again. */
  key: string;
  kind: string;
  title: string;
  detail: Record<string, unknown> | null;
  requested_at: number;
  /** Q246: when silence refuses the question, in milliseconds since the epoch. */
  expires_at?: number;
  /** D154: the project and origin the asking surface recorded; absent on older files. */
  project_root?: string;
  session_id?: string;
  agent_id?: string;
  turn_id?: string;
}

export type ApprovalAnswer = "once" | "session" | "project" | "always" | "deny";

/** Roadmap 83: the Audit log page's filter. Empty fields mean "all". */
export interface AuditFilter {
  project: string;
  q: string;
  session: string;
  agent: string;
  turn: string;
  decision: string;
}

export interface AuditRecord {
  schema: string;
  seq: number;
  approval_id: string;
  session_id: string;
  agent_id: string;
  turn_id: string | null;
  kind: string;
  request: unknown;
  decision: string;
  scope: string | null;
  requested_at: number;
  decided_at: number | null;
  prev_hash: string;
  hash: string;
}

export interface AuditView {
  project: string;
  total: number;
  matched: number;
  valid: boolean;
  first_bad: { line: number; seq: number | null; reason: string } | null;
  records: AuditRecord[];
}

export interface AuditExport {
  csv: string;
  rows: number;
  total: number;
  sha256: string;
  chain_tip: string;
}


/** One claim in the ledger, as the Rust struct serialises. */
export interface Claim {
  id: string;
  path: string;
  agent_id: string;
  /** The agent's role, else its harness. */
  agent: string;
  harness: string;
  base_hash: string | null;
  acquired_at: number;
  heartbeat_at: number;
  released_at: number | null;
  release_reason: string | null;
  /** No heartbeat for 90 s: reported, never auto-released. */
  stale: boolean;
  held: boolean;
}

export interface ClaimConflict {
  id: string;
  path: string;
  claimant: string | null;
  writer: string | null;
  base_hash: string | null;
  observed_hash: string | null;
  detected_at: number;
  resolved_at: number | null;
  resolution: string | null;
}

export interface UnverifiedChange {
  path: string;
  agent: string | null;
  claimant: string | null;
  base_hash: string | null;
  observed_hash: string | null;
  ambiguous: boolean;
}

export interface CoordinationView {
  claims: Claim[];
  unverified_changes: UnverifiedChange[];
  conflicts: ClaimConflict[];
}

/** One stored verdict from the clean-context reviewer (D115). */
export interface MergeReviewView {
  id: string;
  session_id: string;
  agent_id: string;
  reviewer_model: string;
  verdict: "approved" | "blocked" | "unavailable";
  reason: string | null;
  /** The hash of the exact diff the verdict was about. */
  diff_sha: string;
  files: string[];
  /** The reason the user typed to merge over a block, if they did. */
  override_note: string | null;
  /** False means the reviewer answered but the merge did not happen (or has not yet). */
  merged: boolean;
  created_at: number;
}

/**
 * A writable subagent as the Diff view lists it (D80). The pending diff is not here: this
 * rides the two-second poll, and a worktree diff can be a whole file.
 */
export interface WritableAgentView {
  agent_id: string;
  role: string | null;
  model: string | null;
  status: string;
  worktree: string | null;
  plan: string | null;
  plan_note: string | null;
  review: MergeReviewView | null;
}

/** A worktree's pending changes as one unified diff, read on demand. */
export interface WritableDiffView {
  agent_id: string;
  files: string[];
  diff: string;
}

/** One live per-path write lane (roadmap 46). */
export interface MergeQueueEntry {
  path: string;
  writer_id: string;
  /** The writer holding this path while this ticket waits, or the holder itself. */
  holder_id: string;
  state: "writing" | "waiting";
  /** One-based for a waiter; zero for the holder. */
  position: number;
  since: number;
}

export interface MergeQueueView {
  entries: MergeQueueEntry[];
}

/** One file's outcome from a merge, as the CAS pipeline reported it. */
export interface MergeLineView {
  path: string;
  status: "applied" | "conflicted" | "deleted" | "unsupported" | "unchanged" | string;
  patch_id: string | null;
  detail: string | null;
}

export interface MergeReportView {
  agent_id: string;
  base: string;
  lines: MergeLineView[];
  applied: number;
  conflicted: number;
  skipped: number;
}

/** The result of a merge the user decided: the review it happened under, and what it did. */
export interface AppliedMerge {
  review: MergeReviewView;
  report: MergeReportView;
  worktree_removed: boolean;
}

/** One side of a conflict: its text and the hash those bytes must have (D79). */
export interface ConflictText {
  hash: string;
  text: string;
}

/** The three rendered versions. A side is null when Rigel could not rebuild it. */
export interface ConflictSides {
  id: string;
  path: string;
  base: ConflictText | null;
  ours: ConflictText | null;
  theirs: ConflictText | null;
  /** Why a side is missing, when one is. */
  note?: string;
  merged_patch_id?: string;
  merged_hash?: string;
  /** The stored guided-merge verdict, once one has been written. */
  verdict?: ConflictVerdict | null;
}

/** One conflict block as the three versions a reviewer decides between. */
export interface ConflictHunk {
  base: string;
  ours: string;
  theirs: string;
}

export interface ConflictHunkDecision {
  index: number;
  choice: "ours" | "theirs" | "edited";
}

/** The reviewer's answer, sent with the merged text. */
export interface ConflictVerdictInput {
  reviewer: string;
  decisions: ConflictHunkDecision[];
  test_hint: string;
  tests_acknowledged: boolean;
}

/** The verdict stored beside the conflict after the write. */
export interface ConflictVerdict extends ConflictVerdictInput {
  verdict: "accepted";
  hunks: number;
  merged_hash: string;
  created_at: number;
}

/** The merge text, before anything is written. */
export interface MergedConflict {
  id: string;
  path: string;
  merged: string;
  conflicts: number;
  clean: boolean;
  /** The base, ours and theirs text for each conflict block, in order. */
  hunks: ConflictHunk[];
}

export interface MergeResolution {
  path: string;
  patch: Patch | null;
  unchanged: boolean;
  verdict: ConflictVerdict;
}

export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;


/** The browser preview's queue. Answering removes the row; nothing is sent anywhere. */
let previewQueue: PendingApproval[] = previewApprovals.map((pending) => ({ ...pending }));

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

/** Roadmap 82: the preview starts safe when `?safe=1` so every turn row carries the mode. */
let previewSafeMode: SafeModeView = {
  mode: typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('safe') === '1'
    ? 'safe'
    : 'normal',
  leave_phrase: 'leave safe mode',
}

export const api = {
  isTauri,

  /** The approval card: every open question for this project, oldest first (P4.10e, D154). */
  approvals: {
    async pending(project: string): Promise<PendingApproval[]> {
      if (!isTauri()) return previewQueue;
      return call<PendingApproval[]>("approval_pending", { project });
    },
    /** Answer one question. A refusal is `null` on the wire, never an unknown string. */
    async answer(
      id: string,
      scope: ApprovalAnswer,
      options?: { skip?: boolean; note?: string },
    ): Promise<void> {
      if (!isTauri()) {
        previewQueue = previewQueue.filter((pending) => pending.id !== id);
        return;
      }
      return call<void>("approval_answer", {
        id,
        scope: scope === "deny" || options?.skip ? null : scope,
        skip: options?.skip ?? false,
        note: options?.note ?? null,
      });
    },
  },

  /** Roadmap 83: the approval ledger, filtered by the same fields the page shows. */
  audit: {
    async query(project: string, filter: AuditFilter): Promise<AuditView> {
      if (!isTauri()) {
        return { project, total: 0, matched: 0, valid: true, first_bad: null, records: [] };
      }
      return call<AuditView>("audit_query", { project, filter });
    },
    async exportCsv(project: string, filter: AuditFilter): Promise<AuditExport> {
      if (!isTauri()) throw new Error("Export the audit log in the desktop app.");
      return call<AuditExport>("audit_export", { project, filter });
    },
  },

  async openProject(path: string): Promise<ProjectInfo> {
    if (!isTauri()) {
      return { root: path, name: path.split(/[\\/]/).pop() || path, db: "(browser preview)" };
    }
    return call<ProjectInfo>("project_open", { path });
  },

  /** The window driver's first project. `null` in a shipped build. */
  async e2eConfig(): Promise<E2eConfig | null> {
    if (!isTauri()) return null;
    try {
      return await call<E2eConfig | null>("e2e_config");
    } catch {
      return null;
    }
  },

  /** The same resolver the turn uses, so the inspector cannot drift from the door. */
  async routeExplain(provider?: string | null, model?: string | null): Promise<RouteExplanation> {
    if (isTauri()) {
      return call<RouteExplanation>("route_explain", {
        provider: provider ?? null,
        model: model ?? null,
      });
    }
    const saved = previewSettings
      ? { provider: previewSettings.provider, model: previewSettings.model }
      : PREVIEW_ROUTE;
    const alias = model
      ? PREVIEW_ALIASES[model.trim().toLowerCase()]
      : undefined;
    const requested =
      provider || model
        ? {
            provider: provider ?? alias?.provider ?? saved.provider,
            model: model ?? saved.model,
          }
        : null;
    const resolved = alias ?? requested ?? saved;
    const reason: RouteReason = alias ? "alias" : requested ? "picker" : "saved";
    return {
      saved,
      requested,
      resolved,
      reason,
      mismatch:
        resolved.provider !== saved.provider || resolved.model !== saved.model
          ? `asked for ${requested?.provider ?? "nothing"} / ${requested?.model ?? "nothing"}; resolved ${resolved.provider} / ${resolved.model} (${reason}), while Settings holds ${saved.provider} / ${saved.model}`
          : null,
    };
  },

  /** The native folder picker. Returns null when the user cancels or in the browser preview. */
  async pickProjectFolder(): Promise<string | null> {
    if (!isTauri()) return null;
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, title: "Open a project folder" });
    return typeof picked === "string" ? picked : null;
  },

  /** Copy files chosen in the native picker into `.rigel/attachments`, returning relative paths. */
  async importAttachments(project: string, paths: string[]): Promise<string[]> {
    if (!isTauri()) return [];
    return call<string[]>("attachment_import", { project, paths });
  },

  /** Create a folder under the workspace root and open it. */
  async createProject(name: string): Promise<ProjectInfo> {
    if (!isTauri()) throw new Error("Create a workspace from the desktop app, not the browser preview.");
    return call<ProjectInfo>("project_create", { name });
  },

  /** Where "New workspace" puts folders: `%USERPROFILE%\\Rigel` on Windows. */
  async workspaceRoot(): Promise<string> {
    if (!isTauri()) return "~/Rigel";
    return call<string>("project_workspace_root");
  },

  async listSessions(project: string): Promise<Session[]> {
    if (!isTauri()) return [];
    return call<Session[]>("session_list", { project });
  },

  async forkSession(project: string, session: string, turn: string): Promise<Session> {
    if (!isTauri()) throw new Error("Fork a session in the desktop app.");
    return call<Session>("session_fork", { project, session, turn });
  },

  /** D148: nil clears the name, so the automatic title is used again. */
  async renameSession(project: string, id: string, title: string | null): Promise<Session> {
    if (!isTauri()) throw new Error("Rename a session in the desktop app.");
    return call<Session>("session_rename", { project, id, title });
  },

  async pinSession(project: string, id: string, pinned: boolean): Promise<Session> {
    if (!isTauri()) throw new Error("Pin a session in the desktop app.");
    return call<Session>("session_pin", { project, id, pinned });
  },

  async archiveSession(project: string, id: string, archived: boolean): Promise<Session> {
    if (!isTauri()) throw new Error("Archive a session in the desktop app.");
    return call<Session>("session_archive", { project, id, archived });
  },

  /** Q274: permanent deletion of an archived session; the core refuses a live one. */
  async deleteSession(project: string, id: string, confirmation: string): Promise<void> {
    if (!isTauri()) throw new Error("Delete a session in the desktop app.");
    await call<void>("session_delete", { project, id, confirmation });
  },

  async listBookmarks(project: string, session: string): Promise<Bookmark[]> {
    if (!isTauri()) return [];
    return call<Bookmark[]>("bookmark_list", { project, session });
  },

  /** D154: unresolved work, grouped by session, with open approvals leading the list. */
  async attention(project: string): Promise<Attention> {
    if (!isTauri()) return previewAttention();
    return call<Attention>("attention_list", { project });
  },

  /** D152: the project's own templates. Nothing here is committed to the repository. */
  templates: {
    async list(project: string): Promise<TemplateModel[]> {
      if (!isTauri())
        return previewTemplateList().map((template) => ({
          ...template,
          variables: {},
          slots: [],
          agent_sections: [],
          variable_defs: [],
        }));
      return call<TemplateModel[]>("template_list_v2", { project });
    },

    async preview(project: string, template: TemplateModel): Promise<TemplateFieldDiff[]> {
      if (!isTauri()) {
        const existing = previewTemplateList().find((row) => row.id === template.id);
        if (!existing) return [];
        return ["name", "model", "effort", "max_steps", "prompt"]
          .filter((field) => existing[field as keyof SessionTemplate] !== template[field as keyof SessionTemplate])
          .map((field) => ({
            field,
            old: String(existing[field as keyof SessionTemplate] ?? ""),
            new: String(template[field as keyof SessionTemplate] ?? ""),
          }));
      }
      return call<TemplateFieldDiff[]>("template_preview_save", { project, template });
    },

    async save(project: string, template: TemplateModel): Promise<TemplateModel> {
      if (!isTauri()) return { ...previewTemplateSave(template), ...template };
      return call<TemplateModel>("template_save_v2", { project, template });
    },

    async delete(project: string, id: string): Promise<void> {
      if (!isTauri()) {
        previewTemplates = previewTemplateList().filter((row) => row.id !== id);
        return;
      }
      return call<void>("template_delete", { project, id });
    },
  },

  /** Q270: skill pins belong to the session, not the window; a template copies them. */
  pins: {
    async list(project: string, session: string): Promise<SkillPin[]> {
      if (!isTauri()) return previewPins[session] ?? [];
      return call<SkillPin[]>("session_skill_pins", { project, session });
    },

    async pin(
      project: string,
      session: string,
      skillId: string,
      bodySha?: string,
    ): Promise<SkillPin> {
      if (!isTauri()) {
        const list = previewPins[session] ?? [];
        const existing = list.find((pin) => pin.skill_id === skillId);
        if (existing) return existing;
        const pinned: SkillPin = {
          skill_id: skillId,
          body_sha: bodySha ?? previewBodySha(skillId),
          pinned_at: Date.now(),
          current_body_sha: bodySha ?? previewBodySha(skillId),
          changed: false,
          pinned_body_available: true,
          current_body_available: true,
        };
        previewPins = {
          ...previewPins,
          [session]: [...list, pinned].sort((a, b) => a.pinned_at - b.pinned_at),
        };
        return pinned;
      }
      return call<SkillPin>("session_skill_pin", { project, session, skillId, bodySha });
    },

    async review(project: string, session: string, skillId: string): Promise<SkillPinReview> {
      if (!isTauri()) {
        const pin = (previewPins[session] ?? []).find((row) => row.skill_id === skillId);
        if (!pin) throw new Error(`${skillId} is not pinned to this session`);
        return {
          pin,
          pinned_body: pin.skill_id === PREVIEW_PDF_ID ? PREVIEW_PDF_OLD_BODY : null,
          current_body: pin.skill_id === PREVIEW_PDF_ID ? PREVIEW_PDF_CURRENT_BODY : null,
        };
      }
      return call<SkillPinReview>("session_skill_review", { project, session, skillId });
    },

    async keep(
      project: string,
      session: string,
      skillId: string,
      reason: string,
    ): Promise<SkillPin> {
      if (!isTauri()) {
        const pin = (previewPins[session] ?? []).find((row) => row.skill_id === skillId);
        if (!pin) throw new Error(`${skillId} is not pinned to this session`);
        return pin;
      }
      return call<SkillPin>("session_skill_keep", { project, session, skillId, reason });
    },

    async keepAll(project: string, session: string, reason: string): Promise<SkillPin[]> {
      if (!isTauri()) return previewPins[session] ?? [];
      return call<SkillPin[]>("session_skill_keep_all", { project, session, reason });
    },

    async update(
      project: string,
      session: string,
      skillId: string,
      reason: string,
    ): Promise<SkillPin> {
      if (!isTauri()) {
        const list = previewPins[session] ?? [];
        const pin = list.find((row) => row.skill_id === skillId);
        if (!pin) throw new Error(`${skillId} is not pinned to this session`);
        const next: SkillPin = {
          ...pin,
          body_sha: pin.current_body_sha ?? pin.body_sha,
          changed: false,
          pinned_body_available: pin.current_body_available,
          pinned_at: Date.now(),
        };
        previewPins = {
          ...previewPins,
          [session]: list.map((row) => (row.skill_id === skillId ? next : row)),
        };
        return next;
      }
      return call<SkillPin>("session_skill_update", { project, session, skillId, reason });
    },

    async updateAll(project: string, session: string, reason: string): Promise<SkillPin[]> {
      if (!isTauri()) {
        const list = previewPins[session] ?? [];
        const next = list.map((pin) => {
          if (!pin.changed) return pin;
          return {
            ...pin,
            body_sha: pin.current_body_sha ?? pin.body_sha,
            changed: false,
            pinned_body_available: pin.current_body_available,
            pinned_at: Date.now(),
          };
        });
        previewPins = { ...previewPins, [session]: next };
        return next;
      }
      return call<SkillPin[]>("session_skill_update_all", { project, session, reason });
    },

    async unpin(project: string, session: string, skillId: string): Promise<boolean> {
      if (!isTauri()) {
        const list = previewPins[session] ?? [];
        const next = list.filter((pin) => pin.skill_id !== skillId);
        previewPins = { ...previewPins, [session]: next };
        return next.length !== list.length;
      }
      return call<boolean>("session_skill_unpin", { project, session, skillId });
    },
  },

  /** D156: read the chain for one patch. Nothing here is written or re-run. */
  async provenance(project: string, patchId: string): Promise<Provenance> {
    if (!isTauri()) return previewProvenance(patchId);
    return call<Provenance>("patch_provenance", { project, patchId });
  },

  /** `null` means the star was cleared. */
  async toggleBookmark(
    project: string,
    session: string,
    kind: "item" | "hunk",
    targetId: string,
    label: string,
  ): Promise<Bookmark | null> {
    if (!isTauri()) throw new Error("Bookmark a line in the desktop app.");
    return call<Bookmark | null>("bookmark_toggle", { project, session, kind, targetId, label });
  },

  /** Roadmap 103: write one bookmark's note, or clear it with `null`. */
  async setBookmarkNote(
    project: string,
    session: string,
    id: string,
    note: string | null,
  ): Promise<boolean> {
    if (!isTauri()) return false;
    return call<boolean>("bookmark_note", { project, session, id, note });
  },

  /** D147: skills, sessions, turns and patches in one read. Capped per section in the core. */
  async searchAll(
    project: string | null,
    query: string,
    currentSession: string | null,
  ): Promise<PaletteResults> {
    if (!isTauri()) {
      return { skills: [], sessions: [], turns: [], patches: [] };
    }
    return call<PaletteResults>("search_all", { project, query, currentSession });
  },

  async startSession(project: string, title?: string, budgetCents?: number | null): Promise<Session> {
    if (!isTauri()) throw new Error("Start a session from the desktop app, not the browser preview.");
    return call<Session>("session_start", {
      project,
      title: title ?? null,
      budgetCents: budgetCents ?? null,
    });
  },

  async getSession(project: string, id: string): Promise<Session> {
    return call<Session>("session_get", { project, id });
  },

  async transcript(project: string, id: string): Promise<TurnTranscript[]> {
    return call<TurnTranscript[]>("session_transcript", { project, id });
  },

  /** Render the URL a harness browser call visited when no exact capture exists. */
  async browserCapture(url: string): Promise<BrowserCapture> {
    if (!isTauri()) throw new Error("Browser captures are available in the desktop app.");
    return call<BrowserCapture>("browser_capture", { url });
  },

  /** Read the exact post-action frame written by the harness's Rigel b server. */
  async browserHarnessCapture(session: string, agent: string): Promise<BrowserHarnessCapture> {
    if (!isTauri()) throw new Error("Browser captures are available in the desktop app.");
    return call<BrowserHarnessCapture>("browser_harness_capture", { session, agent });
  },

  /** Read a screenshot `browser_open` saved, as a data URL the CSP already permits. */
  async browserImage(path: string): Promise<string> {
    if (!isTauri()) throw new Error("Browser captures are available in the desktop app.");
    return call<string>("browser_image", { path });
  },

  async resumeSession(project: string, id: string, force: boolean): Promise<ResumeOutcome> {
    return call<ResumeOutcome>("session_resume", { project, id, force });
  },

  async recoveryOffer(project: string, id: string): Promise<RecoveryOffer | null> {
    if (!isTauri()) return null;
    return call<RecoveryOffer | null>("session_recovery", { project, id });
  },

  async discardTurn(project: string, turn: string): Promise<Turn> {
    if (!isTauri()) throw new Error("Discarding a turn is available in the desktop app.");
    return call<Turn>("turn_discard", { project, turn });
  },

  async listAgents(project: string, session: string): Promise<Agent[]> {
    return call<Agent[]>("agent_list", { project, session });
  },

  async team(project: string, session: string): Promise<TeamAgent[]> {
    return call<TeamAgent[]>("agent_team", { project, session });
  },

  async spawnAgent(
    project: string,
    session: string,
    harness: "native" | "claude" = "native",
    role?: string,
    model?: string,
  ): Promise<Agent> {
    return call<Agent>("agent_spawn", {
      project,
      session,
      harness,
      role: role ?? null,
      model: model ?? null,
    });
  },

  async stopAgent(project: string, agent: string): Promise<void> {
    return call<void>("agent_stop", { project, agent });
  },

  async sendTurn(
    project: string,
    session: string,
    agent: string,
    prompt: string,
    model?: string,
    provider?: string,
    effort?: string,
    images?: string[],
    maxSteps?: number,
  ): Promise<TurnReply> {
    // The provider travels with the model: a settings save that has not landed yet must not send
    // the pick through the previous door (which is how a Gemini pick became a Vertex 401).
    return call<TurnReply>("turn_send", {
      project,
      session,
      agent,
      prompt,
      model: model ?? null,
      provider: provider ?? null,
      effort: effort ?? null,
      images: images ?? null,
      maxSteps: maxSteps ?? null,
    });
  },

  async cancelTurn(turn: string): Promise<void> {
    return call<void>("turn_cancel", { turn });
  },

  /** Q254: stop at the next step boundary; `resumeTurn` continues the same turn. */
  async pauseTurn(turn: string): Promise<void> {
    return call<void>("turn_pause", { turn });
  },

  async resumeTurn(turn: string): Promise<void> {
    return call<void>("turn_resume", { turn });
  },

  /** D98: read the session's plan phase. */
  async planGet(project: string, session: string): Promise<PlanState> {
    return call<PlanState>("plan_get", { project, session });
  },

  /** D98: plan on; approving sets `enabled: false` and records the hand-off. */
  async planSet(project: string, session: string, enabled: boolean): Promise<PlanState> {
    return call<PlanState>("plan_set", { project, session, enabled });
  },

  /** Roadmap 42: the parent turn's subagent graph with its edges and states. */
  async subagentGraph(project: string, session: string): Promise<SubagentGraph[]> {
    return call<SubagentGraph[]>("subagent_graph", { project, session });
  },

  /** Roadmap 66: the project ceiling and the effective role/agent grants. */
  async permissionsGet(project: string, session: string | null): Promise<PermissionMatrix> {
    if (!isTauri()) return previewPermissionMatrix();
    return call<PermissionMatrix>("permissions_get", { project, session });
  },

  /** Roadmap 66: save the project grant; a role never gains a tool back. */
  async permissionsSet(
    project: string,
    session: string | null,
    tools: string[],
  ): Promise<PermissionMatrix> {
    if (!isTauri()) {
      previewPermissionGrant = [...tools];
      return previewPermissionMatrix();
    }
    return call<PermissionMatrix>("permissions_set", { project, session, tools });
  },

  /** Roadmap 66: narrow one agent below its role; `null` clears the override. */
  async permissionsSetAgent(
    project: string,
    session: string,
    agent: string,
    tools: string[] | null,
  ): Promise<PermissionMatrix> {
    if (!isTauri()) {
      if (tools === null) delete previewPermissionOverrides[agent];
      else previewPermissionOverrides[agent] = [...tools];
      return previewPermissionMatrix();
    }
    return call<PermissionMatrix>("permissions_set_agent", {
      project,
      session,
      agent,
      tools,
    });
  },

  /** D94: the four built-in role presets. */
  async rolePresets(): Promise<RolePreset[]> {
    if (!isTauri()) return [];
    return call<RolePreset[]>("role_presets");
  },

  /** D94: apply a preset from the roster; the next turn reads the new role. */
  async roleApply(
    project: string,
    session: string,
    agent: string,
    preset: string,
  ): Promise<Agent> {
    return call<Agent>("role_apply", { project, session, agent, preset });
  },

  /** Roadmap 58: leave a preset while keeping the pair it resolved to. */
  async roleClear(project: string, session: string, agent: string): Promise<Agent> {
    return call<Agent>("role_clear", { project, session, agent });
  },

  /** Roadmap 58: a roster row's own model, provider and effort. */
  async agentConfigure(
    project: string,
    session: string,
    agent: string,
    model: string,
    provider: string,
    effort: string,
  ): Promise<Agent> {
    return call<Agent>("agent_configure", {
      project,
      session,
      agent,
      model,
      provider,
      effort,
    });
  },

  /** Roadmap 61: read the project's network policy. The preview starts from the default. */
  async networkGet(project: string): Promise<NetworkPolicy> {
    if (!isTauri()) {
      return {
        enabled: true,
        default: "ask",
        rules: [],
        limits: {
          window_seconds: 60,
          per_origin_requests: 30,
          per_agent_requests: 120,
          max_response_bytes: 262144,
          pin_dns: true,
          allow_private_addresses: false,
        },
        disabled_sessions: [],
      };
    }
    return call<NetworkPolicy>("network_get", { project });
  },

  /** Roadmap 61: validate and save the policy the core will enforce. */
  async networkSet(project: string, policy: NetworkPolicy): Promise<NetworkPolicy> {
    if (!isTauri()) return policy;
    return call<NetworkPolicy>("network_set", { project, policy });
  },

  /** Roadmap 61: explain one URL before a fetch. The preview has no core to ask. */
  async networkCheck(
    project: string,
    url: string,
    session?: string | null,
  ): Promise<NetworkCheck | null> {
    if (!isTauri()) return null;
    return call<NetworkCheck>("network_check", {
      project,
      url,
      session: session ?? null,
    });
  },

  /**
   * Price the first step of a turn before it is sent. In the browser this is a local
   * heuristic over sample prices; in the app the quote comes from Rust, priced with the
   * same table that bills the step once it lands.
   */
  async quoteTurn(
    project: string,
    session: string,
    prompt: string,
    spendMicros: number,
    capMicros: number | null,
    model?: string,
    provider?: string,
    effort?: string,
  ): Promise<TurnQuote> {
    if (!isTauri()) return previewQuote(prompt, spendMicros, capMicros);
    return call<TurnQuote>("turn_estimate", {
      project,
      session,
      prompt,
      model: model ?? null,
      provider: provider ?? null,
      effort: effort ?? null,
    });
  },

  async getBudget(project: string, agent: string): Promise<BudgetSnapshot> {
    return call<BudgetSnapshot>("budget_get", { project, agent });
  },

  async setBudget(project: string, session: string, capCents: number | null): Promise<Session> {
    return call<Session>("budget_set", { project, session, capCents });
  },

  async raiseBudget(project: string, session: string, capCents: number): Promise<Session> {
    return call<Session>("budget_raise", { project, session, capCents });
  },

  /** Roadmap 82: read the session's safe mode. The preview keeps it in memory. */
  async safeModeGet(project: string, session: string): Promise<SafeModeView> {
    if (!isTauri()) return { ...previewSafeMode };
    return call<SafeModeView>("safe_mode_get", { project, session });
  },

  /** Entering is explicit; leaving requires the exact phrase the core enforces. */
  async safeModeSet(
    project: string,
    session: string,
    enter: boolean,
    confirmation = '',
  ): Promise<SafeModeView> {
    if (!isTauri()) {
      if (!enter && confirmation !== previewSafeMode.leave_phrase) {
        throw new Error('confirmation phrase does not match');
      }
      previewSafeMode = {
        ...previewSafeMode,
        mode: enter ? 'safe' : 'normal',
      };
      return { ...previewSafeMode };
    }
    return call<SafeModeView>("safe_mode_set", { project, session, enter, confirmation });
  },

  /** D92: spend across every session in the project. */
  /** The writable-agent cap and the day's spend. */
  async safetyGet(project: string): Promise<SafetyView> {
    return call<SafetyView>("safety_get", { project });
  },
  /**
   * Change one knob. `dailyCapCents: null` leaves it alone; pass `clearDailyCap` to turn it
   * off. A one-click raise is exactly this call with a larger number.
   */
  async safetySet(
    project: string,
    session: string,
    input: {
      writableAgents?: number;
      dailyCapCents?: number;
      clearDailyCap?: boolean;
    },
  ): Promise<SafetyView> {
    return call<SafetyView>("safety_set", {
      project,
      session,
      writableAgents: input.writableAgents ?? null,
      dailyCapCents: input.dailyCapCents ?? null,
      clearDailyCap: input.clearDailyCap ?? false,
    });
  },

  async spendReport(project: string): Promise<SpendReport> {
    return call<SpendReport>("spend_report", { project });
  },

  /** D92: cap one task (agent) below the session cap. `null` removes it. */
  async taskCapSet(
    project: string,
    session: string,
    agent: string,
    capCents: number | null,
  ): Promise<BudgetSnapshot> {
    return call<BudgetSnapshot>("task_cap_set", { project, session, agent, capCents });
  },


  /** The merged diff: claims, unverified changes and open conflicts (P5.6). */
  async claimList(project: string): Promise<CoordinationView> {
    if (!isTauri()) {
      return { claims: [], unverified_changes: [], conflicts: [] };
    }
    return call<CoordinationView>("claim_list", { project });
  },

  /**
   * Roadmap 95: release one agent's claim on the named paths. The UI only calls this after an
   * explicit confirmation with a reason; the core keeps the released row as a tombstone.
   */
  async claimRelease(
    project: string,
    agent: string,
    paths: string[],
    reason: string,
  ): Promise<string[]> {
    if (!isTauri()) return [];
    return call<string[]>("claim_release", { project, agent, paths, reason });
  },

  /** Live write lanes, holder and waiters; polled with the claims and reviews. */
  async mergeQueue(project: string): Promise<MergeQueueView> {
    if (!isTauri()) return { entries: [] };
    return call<MergeQueueView>("merge_queue", { project });
  },

  /** Writable subagents with their plans and latest review; polled with the claims. */
  async writableList(project: string, session: string): Promise<WritableAgentView[]> {
    if (!isTauri()) return [];
    return call<WritableAgentView[]>("writable_list", { project, session });
  },

  /** The worktree's pending changes as one diff, read once when a row is expanded. */
  async writableDiff(project: string, agent: string): Promise<WritableDiffView> {
    return call<WritableDiffView>("writable_diff", { project, agent });
  },

  /**
   * Merge a recorded review on the user's behalf. An approved review needs no reason; a
   * blocked or unavailable one needs the typed reason that will be stored beside it.
   */
  async writableMerge(
    project: string,
    session: string,
    agent: string,
    reason: string | null,
  ): Promise<AppliedMerge> {
    return call<AppliedMerge>("writable_merge", { project, session, agent, reason });
  },

  async resolveConflict(project: string, id: string, resolution: string): Promise<void> {
    if (!isTauri()) return;
    return call<void>("conflict_resolve", { project, id, resolution });
  },

  /** Read the base/ours/theirs text once, when a conflict row is expanded (D79). */
  async conflictSides(project: string, id: string): Promise<ConflictSides> {
    return call<ConflictSides>("conflict_sides", { project, id });
  },

  /** Compute the merge; nothing is written until `resolveMergedConflict`. */
  async mergeConflict(project: string, id: string): Promise<MergedConflict> {
    return call<MergedConflict>("conflict_merge", { project, id });
  },

  /** Write the merged text through the CAS patch pipeline and resolve the conflict (D79). */
  async resolveMergedConflict(
    project: string,
    session: string,
    id: string,
    merged: string,
    verdict: ConflictVerdictInput,
  ): Promise<MergeResolution> {
    return call<MergeResolution>("conflict_resolve_merged", {
      project,
      session,
      id,
      merged,
      verdict,
    });
  },

  /** Tell the desktop watcher which project to follow. */
  async watchProject(project: string): Promise<void> {
    if (!isTauri()) return;
    return call<void>("watch_project", { project });
  },

  /** Queue a Swarm task for Rigel, the Muse coordinator. The API keeps it until Muse claims it. */
  async swarmEnqueue(
    project: string,
    task: string,
    targets: string[],
    lead: SwarmLead = { kind: "rigel" },
  ): Promise<string> {
    if (!isTauri()) throw new Error("Muse swarm is only available in the desktop app.");
    return call<string>("swarm_enqueue", { project, task, targets, lead });
  },

  async listPatches(project: string, session: string): Promise<Patch[]> {
    return call<Patch[]>("patch_list", { project, session });
  },

  async patchHunks(project: string, session: string): Promise<AttributedHunk[]> {
    return call<AttributedHunk[]>("patch_hunks", { project, session });
  },

  async hunkComments(project: string, session: string): Promise<HunkComment[]> {
    return call<HunkComment[]>("hunk_comments", { project, session });
  },

  /** Attach a comment to one hunk. Nothing is sent until `deliverHunkComment`. */
  async addHunkComment(project: string, hunk: string, body: string): Promise<HunkComment> {
    return call<HunkComment>("hunk_comment_add", { project, hunk, body });
  },

  /** Send the comment to the hunk's author as a turn. */
  async deliverHunkComment(
    project: string,
    session: string,
    id: string,
  ): Promise<TurnReply> {
    return call<TurnReply>("hunk_comment_deliver", { project, session, id });
  },

  async revertPatch(id: string): Promise<Patch> {
    return call<Patch>("patch_revert", { id });
  },

  /** Cached probe rows; `force` re-runs the binaries instead of trusting a fresh cache. */
  async harnessProbe(force = false): Promise<HarnessInfo[]> {
    if (!isTauri()) return PREVIEW_HARNESSES;
    return call<HarnessInfo[]>("harness_probe", { force });
  },

  async keyStatus(): Promise<KeyStatus> {
    if (!isTauri()) return { providers: {}, native_ready: false };
    return call<KeyStatus>("key_status");
  },

  async setKey(provider: string, value: string): Promise<KeySetResult> {
    if (!isTauri()) {
      return { status: { providers: { [provider]: true }, native_ready: true }, check: null };
    }
    return call<KeySetResult>("key_set", { provider, value });
  },

  async deleteKey(provider: string): Promise<KeyStatus> {
    if (!isTauri()) return { providers: { [provider]: false }, native_ready: false };
    return call<KeyStatus>("key_delete", { provider });
  },

  /** The chat adapters, in the order the app lists them. A preview answers the same shape. */
  async providersList(): Promise<ProviderRow[]> {
    if (!isTauri()) return PREVIEW_PROVIDERS;
    return call<ProviderRow[]>("providers_list");
  },

  /** Roadmap 26: the project's effective OpenRouter policy, before a turn. */
  async routingPolicy(project: string): Promise<RoutingPolicy> {
    if (!isTauri()) return PREVIEW_POLICY;
    return call<RoutingPolicy>("routing_policy", { project });
  },

  /** Roadmap 28: the saved custom door, masked; null until one is saved. */
  async customProviderGet(): Promise<CustomDoorView | null> {
    if (!isTauri()) return previewCustomDoor;
    return call<CustomDoorView | null>("custom_provider_get");
  },

  /** Roadmap 28: probe a candidate door without saving its headers. */
  async customProviderProbe(baseUrl: string, headers: CustomHeader[]): Promise<string[]> {
    if (!isTauri()) {
      if (!/^https?:\/\//i.test(baseUrl.trim())) {
        throw new Error("the base URL must start with http:// or https://");
      }
      return ["llama-3.2-8b", "qwen2.5-coder-7b"];
    }
    return call<string[]>("custom_provider_probe", {
      door: { name: "", base_url: baseUrl, headers, models: [] },
    });
  },

  /** Roadmap 28: write the complete record to the keychain and return its masked view. */
  async customProviderSave(door: {
    name: string;
    base_url: string;
    headers: CustomHeader[];
    models: string[];
  }): Promise<CustomDoorView> {
    if (!isTauri()) {
      previewCustomDoor = {
        name: door.name,
        base_url: door.base_url,
        header_names: door.headers.map((header) => header.name),
        models: door.models,
      };
      return previewCustomDoor;
    }
    return call<CustomDoorView>("custom_provider_save", { door });
  },

  async harnesses(): Promise<HarnessInfo[]> {
    // A browser preview has no Rust host, so there is nothing to probe.
    if (!isTauri()) return [];
    return call<HarnessInfo[]>("harnesses_probe");
  },

  async omnirouteStatus(): Promise<OmnirouteStatus> {
    if (!isTauri()) {
      return { running: true, origin: "http://127.0.0.1:20128", base_url: "(browser preview)" };
    }
    return call<OmnirouteStatus>("omniroute_status");
  },

  async omnirouteStart(): Promise<OmnirouteStatus> {
    if (!isTauri()) throw new Error("Start OmniRoute from the desktop app, not the browser preview.");
    return call<OmnirouteStatus>("omniroute_start");
  },

  async localModelStatus(): Promise<LocalModelStatus> {
    if (!isTauri()) {
      return {
        state: "down",
        detail: "The browser preview has no local llama.cpp server.",
        base_url: "http://127.0.0.1:8080/v1",
        server: null,
        model: null,
        mode: "mmap",
        port: 8080,
        pid: null,
        configured: false,
        free_ram_gb: null,
        model_gb: null,
      };
    }
    return call<LocalModelStatus>("local_model_status");
  },

  async localModelStart(): Promise<LocalModelStatus> {
    if (!isTauri()) {
      throw new Error("Start the local model from the desktop app, not the browser preview.");
    }
    return call<LocalModelStatus>("local_model_start");
  },

  async localModelStop(): Promise<LocalModelStatus> {
    if (!isTauri()) {
      throw new Error("Stop the local model from the desktop app, not the browser preview.");
    }
    return call<LocalModelStatus>("local_model_stop");
  },

  /**
   * Boot every local service a turn may need — the router, and the chat server once the
   * project's checkout is known. Fire-and-forget: each service reports itself in its tab.
   */
  async startServers(project: string | null): Promise<void> {
    if (!isTauri()) return;
    await call<void>("servers_start", { project });
  },

  async usage(project: string, session: string): Promise<Usage> {
    return call<Usage>("usage_summary", { project, session });
  },

  async getSettings(): Promise<Settings> {
    if (!isTauri()) {
      return previewSettings ?? defaultSettings();
    }
    return call<Settings>("settings_get");
  },

  /** Whether the opt-in card is on and which broker serves it. Never a publisher key. */
  async gravitySettings(): Promise<{
    enabled: boolean;
    test_ads: boolean;
    placement_id: string;
    endpoint: string;
  }> {
    if (!isTauri()) {
      return {
        enabled: previewSettings?.gravity_ads ?? false,
        test_ads: previewSettings?.gravity_test_ads ?? true,
        placement_id: "rigel-sponsor",
        endpoint: "https://rigel-25128.web.app/api/gravity-ad",
      };
    }
    return call<{
      enabled: boolean;
      test_ads: boolean;
      placement_id: string;
      endpoint: string;
    }>("gravity_settings");
  },

  /** Request one ad from the shared broker. Off means no request at all. */
  async gravityAd(
    session: string,
    messages: GravityMessage[],
    device: GravityDevice,
  ): Promise<GravityAd[]> {
    if (!isTauri()) {
      return previewSettings?.gravity_ads && (previewSettings.gravity_test_ads ?? true)
        ? [PREVIEW_AD]
        : [];
    }
    return call<GravityAd[]>("gravity_ad", { session, messages, device });
  },

  /** Load the impression URL once. Tracking never breaks the page. */
  async gravityImpression(url: string): Promise<void> {
    if (!isTauri() || !url) return;
    try {
      await call<void>("gravity_impression", { url });
    } catch {
      // An impression that cannot be recorded is not an error the reader should see.
    }
  },

  /** Record the click and open the landing page. The app never navigates itself. */
  async gravityClick(url: string, landing?: string, test?: boolean): Promise<string> {
    if (!isTauri()) return test && landing ? landing : url;
    return call<string>("gravity_click", { url, landing: landing ?? null, test: test ?? false });
  },

  async saveSettings(settings: Settings): Promise<Settings> {
    // Every surface saves through here, so this is the one place that can tell an already-open
    // terminal (or another mounted surface) that its copy is stale. The event carries the saved
    // value, not a request to re-read: the preview has no settings file to read back.
    const announce = (saved: Settings): Settings => {
      if (typeof window !== "undefined") {
        window.dispatchEvent(new CustomEvent("rigel://settings", { detail: saved }));
      }
      return saved;
    };
    if (!isTauri()) {
      // The browser preview has no settings.toml. Keep one in memory for the session so a saved
      // choice reads back the way it will in the window, instead of resetting on the next mount.
      previewSettings = settings;
      return announce(settings);
    }
    return announce(await call<Settings>("settings_set", { settings }));
  },

  /** The API's cached Concord inbox, including the delivery receipt on every message. */
  async concordInbox(): Promise<ConcordInbox> {
    if (!isTauri()) {
      return {
        ok: true,
        agent: "muse",
        messages: [],
        retained: 0,
        oldest: 0,
        revision: 0,
        checked_at: 0,
        updated_at: 0,
        error: "The browser preview has no Concord bridge; open the desktop window.",
      };
    }
    return call<ConcordInbox>("concord_inbox");
  },

  /** Reply to one cached message; the API wraps update_work and its idempotency key. */
  async concordReply(messageId: string, content: string): Promise<ConcordReply> {
    if (!isTauri()) throw new Error("Concord replies go through the desktop app.");
    return call<ConcordReply>("concord_reply", { message_id: messageId, content });
  },

  /** Roadmap 116: read the already-redacted crash tail from app data; no bundle is written. */
  async crashReport(): Promise<CrashReport> {
    if (!isTauri()) return PREVIEW_CRASH;
    return call<CrashReport>("crash_report");
  },

  /** Roadmap 124: build stamp, version, SBOM licences and the update/privacy summary. */
  async aboutInfo(): Promise<AboutInfo> {
    if (!isTauri()) return PREVIEW_ABOUT;
    return call<AboutInfo>("about_info");
  },

  async updateStatus(): Promise<UpdateStatus> {
    if (!isTauri()) {
      return {
        current: "0.1.1",
        enabled: false,
        auto_install: false,
        manifest_url: "https://github.com/chuckiphone/Rigel/releases/latest/download/latest.json",
        key_present: false,
      };
    }
    return call<UpdateStatus>("update_status");
  },

  async updateSet(
    enabled: boolean,
    autoInstall: boolean,
    manifestUrl: string | null = null,
  ): Promise<UpdateStatus> {
    if (!isTauri()) {
      return {
        current: "0.1.1",
        enabled,
        auto_install: autoInstall,
        manifest_url:
          manifestUrl ?? "https://github.com/chuckiphone/Rigel/releases/latest/download/latest.json",
        key_present: false,
      };
    }
    return call<UpdateStatus>("update_set", {
      enabled,
      autoInstall,
      manifestUrl,
    });
  },

  /** The project's files, symbols and edges, read locally and bounded by the core. */
  async codeGraph(
    project: string,
    focus: string | null = null,
    depth = 2,
    limit = 700,
  ): Promise<CodeGraph> {
    if (!isTauri()) throw new Error("The code graph is read from the desktop app.");
    return call<CodeGraph>("code_graph", { project, focus, depth, limit });
  },

  /** The background indexer's last pass; `null` before the first scan. */
  async codeGraphWatchStatus(project: string): Promise<GraphWatchStatus | null> {
    if (!isTauri()) return null;
    return call<GraphWatchStatus | null>("code_graph_watch_status", { project });
  },

  /** The live zero-priced chat rows, ranked, with no probe requests. */
  async freeAgents(): Promise<ModelOption[]> {
    if (!isTauri()) return [];
    return call<ModelOption[]>("free_agents");
  },

  async updateCheck(): Promise<UpdateCheck> {
    if (!isTauri()) throw new Error("Update checks run in the desktop app.");
    return call<UpdateCheck>("update_check");
  },

  async updateInstall(): Promise<string> {
    if (!isTauri()) throw new Error("Updates install from the desktop app.");
    return call<string>("update_install");
  },

  /** The hosted/self-hosted Rigel API choice, read from `backend.json`. */
  async backendStatus(): Promise<ApiBackendStatus> {
    if (!isTauri()) {
      return {
        mode: "hosted",
        api_url: "",
        effective_url: "https://api.askrigel.com",
        hosted_url: "https://api.askrigel.com",
        override_env: null,
        relay_token_present: false,
      };
    }
    return call<ApiBackendStatus>("backend_status");
  },

  /** Validate and save the placement. The URL field is ignored in hosted mode. */
  async backendSet(mode: "hosted" | "self_hosted", apiUrl: string): Promise<ApiBackendStatus> {
    if (!isTauri()) throw new Error("Configure the Rigel API in the desktop app.");
    return call<ApiBackendStatus>("backend_set", { mode, apiUrl });
  },

  /** Store the relay bearer credential in the OS keychain; it is never returned. */
  async backendSetToken(token: string): Promise<ApiBackendStatus> {
    if (!isTauri()) throw new Error("The relay credential is saved in the desktop app.");
    return call<ApiBackendStatus>("backend_set_token", { token });
  },

  async backendClearToken(): Promise<ApiBackendStatus> {
    if (!isTauri()) throw new Error("The relay credential is saved in the desktop app.");
    return call<ApiBackendStatus>("backend_clear_token");
  },

  /** The Firebase project and account. No network call; `cloud.json` and the keychain answer. */
  async cloudStatus(): Promise<CloudStatus> {
    if (!isTauri()) {
      return {
        configured: false,
        project_id: null,
        signed_in: false,
        email: null,
        uid: null,
        expires_at: null,
        email_verified: false,
        app_check: null,
        last_push_at: null,
        last_pull_at: null,
        last_sync_at: null,
      };
    }
    return call<CloudStatus>("cloud_status");
  },

  async cloudConfigure(project: string, apiKey: string): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Configure the Firebase project in the desktop app.");
    return call<CloudStatus>("cloud_configure", { project, apiKey });
  },

  /** One sign-in. The password is not stored; only the refresh token reaches the keychain. */
  async cloudLogin(email: string, password: string): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud signs in from the desktop app.");
    return call<CloudStatus>("cloud_login", { email, password });
  },

  /** Create the account; Firebase returns the session, so the page is signed in afterwards. */
  async cloudSignup(email: string, password: string): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud creates accounts from the desktop app.");
    return call<CloudStatus>("cloud_signup", { email, password });
  },

  /** Ask Firebase for a reset link. The answer is the address, not whether it has an account. */
  async cloudResetPassword(email: string): Promise<string> {
    if (!isTauri()) throw new Error("Rigel Cloud sends reset links from the desktop app.");
    return call<string>("cloud_reset_password", { email });
  },

  /**
   * Ask Firebase for a verification link to the signed-in account. Sending again is the resend
   * control; the answer is the address the link went to, never a token.
   */
  async cloudSendVerification(): Promise<string> {
    if (!isTauri()) throw new Error("Rigel Cloud sends verification links from the desktop app.");
    return call<string>("cloud_send_verification");
  },

  /** Re-read the verification state from Firebase; the stored session keeps the answer. */
  async cloudCheckVerification(): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud checks verification from the desktop app.");
    return call<CloudStatus>("cloud_check_verification");
  },

  /**
   * Point App Check at an attestation endpoint, or turn it off with `null`. The endpoint answers
   * with short-lived tokens; none of them ever reaches this process.
   */
  async cloudAppCheck(url: string | null): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud configures App Check in the desktop app.");
    return call<CloudStatus>("cloud_app_check", { url });
  },

  /**
   * Store this installation's App Check client credential in the OS keychain. The endpoint
   * verifies it before minting a token; the value is never returned to this process.
   */
  async cloudAppCheckClientToken(token: string): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud stores App Check credentials in the desktop app.");
    return call<CloudStatus>("cloud_app_check_client_token", { token });
  },

  async cloudClearAppCheckClientToken(): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud stores App Check credentials in the desktop app.");
    return call<CloudStatus>("cloud_app_check_clear_client_token");
  },

  async cloudLogout(): Promise<CloudStatus> {
    if (!isTauri()) throw new Error("Rigel Cloud signs out from the desktop app.");
    return call<CloudStatus>("cloud_logout");
  },

  async cloudPush(): Promise<CloudPushReport> {
    if (!isTauri()) throw new Error("The shelf mirror runs in the desktop app.");
    return call<CloudPushReport>("cloud_push");
  },

  async cloudPull(): Promise<CloudPullReport> {
    if (!isTauri()) throw new Error("The shelf mirror runs in the desktop app.");
    return call<CloudPullReport>("cloud_pull");
  },

  /** The AgentMail inbox and whether a key is stored. No network call. */
  async mailStatus(): Promise<MailStatus> {
    if (!isTauri()) {
      return { configured: false, inbox: null, key_present: false };
    }
    return call<MailStatus>("mail_status");
  },

  async mailConfigure(inbox: string): Promise<MailStatus> {
    if (!isTauri()) throw new Error("The AgentMail inbox is configured in the desktop app.");
    return call<MailStatus>("mail_configure", { inbox });
  },

  /** The key goes to the OS keychain; this window never reads it back. */
  async mailSetKey(apiKey: string): Promise<MailStatus> {
    if (!isTauri()) throw new Error("The AgentMail API key is saved in the desktop app.");
    return call<MailStatus>("mail_set_key", { apiKey });
  },

  /** Conversations in the inbox, newest first. No body is fetched here. */
  async mailThreads(limit = 10): Promise<MailThreadSummary[]> {
    if (!isTauri()) return [];
    return call<MailThreadSummary[]>("mail_threads", { limit });
  },

  /** One conversation, oldest message first. */
  async mailThread(threadId: string): Promise<MailThread> {
    if (!isTauri()) throw new Error("Mail is read in the desktop app.");
    return call<MailThread>("mail_thread", { threadId });
  },

  /** Send from the page. An agent's send goes through the approval card instead. */
  async mailSend(to: string, subject: string, text: string): Promise<SentMail> {
    if (!isTauri()) throw new Error("Mail is sent from the desktop app.");
    return call<SentMail>("mail_send", { to, subject, text });
  },

  /** Publish one session to the phone: prompts and assistant text only. */
  async cloudPublishSession(project: string, session: string): Promise<CloudPublishReport> {
    if (!isTauri()) throw new Error("Sessions publish from the desktop app.");
    return call<CloudPublishReport>("cloud_publish_session", { project, session });
  },

  /** Publish every session in the project; one failure is reported, not fatal. */
  async cloudPublishAll(project: string): Promise<CloudPublishAllReport> {
    if (!isTauri()) throw new Error("Sessions publish from the desktop app.");
    return call<CloudPublishAllReport>("cloud_publish_all", { project });
  },

  /** Compose the PR title/body locally, for a review before creating it. */
  async pullRequestPlan(project: string, session: string): Promise<PullRequestPlan> {
    return call<PullRequestPlan>("pull_request_plan", { project, session });
  },

  /** Run gh pr create. Only an explicit click calls this. */
  async createPullRequest(project: string, session: string): Promise<string> {
    return call<string>("pull_request_create", { project, session });
  },

  /** Project rule files a native turn would read (D91). */
  async rulesSources(project: string): Promise<RuleSource[]> {
    if (!isTauri()) return [];
    return call<RuleSource[]>("rules_sources", { project });
  },

  /** Committed memory as one copyable AGENTS.md block. */
  async exportMemory(project: string): Promise<string> {
    if (!isTauri()) return "";
    return call<string>("rules_export_memory", { project });
  },

  /** Ranked memory facts with recency, use and pin explained (roadmap 55). */
  async memoryFacts(project: string, agent?: string): Promise<MemoryFact[]> {
    if (!isTauri()) return previewMemoryFacts;
    return call<MemoryFact[]>("memory_facts", { project, agent: agent ?? null });
  },

  /** Filter the ranking and count the returned facts as recalled. */
  async recallMemory(
    project: string,
    query: string,
    agent?: string,
    limit = 10,
  ): Promise<MemoryFact[]> {
    if (!isTauri()) {
      const needle = query.trim().toLowerCase();
      return previewMemoryFacts.filter(
        (fact) => !needle || fact.text.toLowerCase().includes(needle),
      );
    }
    return call<MemoryFact[]>("memory_recall", {
      project,
      agent: agent ?? null,
      query,
      limit,
    });
  },

  /** Pin or unpin one fact. A pinned fact's recency value no longer decays. */
  async pinMemoryFact(project: string, id: string, pinned: boolean): Promise<MemoryFact> {
    if (!isTauri()) {
      const found = previewMemoryFacts.find((fact) => fact.id === id);
      if (found) found.pinned = pinned;
      if (found) return found;
      throw new Error(`memory fact ${id} does not exist`);
    }
    return call<MemoryFact>("memory_pin", { project, id, pinned });
  },

  /** Per-turn checkpoints, newest last. */
  async listCheckpoints(project: string, session: string): Promise<Checkpoint[]> {
    if (!isTauri()) return [];
    return call<Checkpoint[]>("checkpoint_list", { project, session });
  },

  /** Restore the snapshot a turn recorded. Explicit; the journal is not rolled back. */
  async restoreCheckpoint(project: string, id: string): Promise<{ files_restored: number }> {
    return call<{ files_restored: number }>("checkpoint_restore", { project, id });
  },

  /** Write a session's Markdown and JSON under the app data folder. */
  async exportSession(project: string, id: string): Promise<SessionExport> {
    if (!isTauri()) throw new Error("Export a session from the desktop app, not the browser preview.");
    return call<SessionExport>("session_export", { project, id });
  },

  /** Assemble the diagnostics bundle on demand; it is written locally and returned for copying. */
  async exportDiagnostics(project: string, session: string | null): Promise<DiagnosticsBundle> {
    if (!isTauri()) {
      const body = "# Rigel diagnostics\n\nThe browser preview has no database or logs.";
      return { path: "(browser preview)", body };
    }
    return call<DiagnosticsBundle>("diagnostics_export", { project, session });
  },

  /**
   * The merged model catalogue for the picker: every provider that holds a credential, the
   * current one first. Rows that cannot answer a chat turn come back marked, not removed — the
   * picker's toggle decides whether they are shown.
   */
  async listModels(): Promise<ModelOption[]> {
    if (!isTauri()) {
      return [
        { id: "gpt-5.6-sol-medium", provider: "omniroute", free: false, chat: true },
        { id: "openai/gpt-4.1-mini", provider: "openrouter", free: false, chat: true },
        { id: "baseten/mammoth-1:free", provider: "openrouter", free: true, chat: true },
        {
          id: "anthropic/claude-sonnet-4.5",
          provider: "openrouter",
          free: false,
          chat: true,
          modality: "text+image->text+image",
          output_micros_per_mtok: 15_000_000,
        },
        { id: "llama3.2", provider: "local", free: true, chat: true, local: true },
        { id: "aihorde/AbyssOrangeMix", provider: "omniroute", free: false, chat: false },
      ];
    }
    return call<ModelOption[]>("models_list");
  },

  /** Item 15: the launch health pass, cached in Rust after the first call. */
  async providerHealth(): Promise<DoorHealth[]> {
    if (!isTauri()) return PREVIEW_HEALTH;
    return call<DoorHealth[]>("provider_health");
  },

  /** Item 18: one capability row per picker row, unknown facts included as null. */
  async modelCapabilities(models: ModelRef[]): Promise<ModelCapability[]> {
    if (!isTauri()) return previewModelCapabilities(models);
    return call<ModelCapability[]>("model_capabilities", { models });
  },

  /** Read the rosters again, ignoring the cache: the launch pass and Free mode do the same. */
  async refreshModels(): Promise<ModelOption[]> {
    if (!isTauri()) return this.listModels();
    return call<ModelOption[]>("models_refresh");
  },

  /**
   * Free mode: refresh every configured provider, let Antigravity rank the live zero-price
   * tool-capable chat rows, then switch to the best one that answers a real probe. The provider
   * and model are persisted through the same path a manual pick uses.
   */
  async freeMode(): Promise<FreeChoice> {
    if (!isTauri()) {
      throw new Error("Free mode tests real models in the desktop app, not the browser preview.");
    }
    return call<FreeChoice>("free_mode");
  },

  /** The vendor chat server the Chat tab fronts. Same boundary as the router above. */
  agentChat: {
    async status(project: string | null): Promise<AgentChatStatus> {
      if (!isTauri()) {
        return { running: true, authed: true, origin: "(browser preview)", dir: null, setup: null };
      }
      return call<AgentChatStatus>("chat_status", { project });
    },

    async start(project: string | null): Promise<AgentChatStatus> {
      if (!isTauri()) throw new Error("Start the chat server from the desktop app, not the browser preview.");
      return call<AgentChatStatus>("chat_start", { project });
    },

    async channels(): Promise<string[]> {
      if (!isTauri()) return ["general", "review", "debate"];
      return call<string[]>("chat_channels");
    },

    async agents(): Promise<Record<string, AgentChatAgent>> {
      if (!isTauri()) {
        return {
          claude: { available: true, busy: false, label: "Claude", color: "#da7756", role: "reviewer" },
          codex: { available: true, busy: false, label: "Codex", color: "#10a37f", role: "reviewer" },
        };
      }
      return call<Record<string, AgentChatAgent>>("chat_agents");
    },

    async messages(channel: string, sinceId = 0): Promise<AgentChatMessage[]> {
      if (!isTauri()) return sinceId > 0 ? [] : PREVIEW_CHAT_MESSAGES;
      return call<AgentChatMessage[]>("chat_messages", { channel, since_id: sinceId, limit: 100 });
    },

    /** One room upload as a data URL. Images only; the window never learns the origin. */
    async attachment(url: string): Promise<string> {
      if (!isTauri()) return "";
      return call<string>("chat_attachment", { url });
    },

    async send(channel: string, text: string): Promise<void> {
      if (!isTauri()) {
        PREVIEW_CHAT_MESSAGES.push({
          id: Date.now(),
          sender: "user",
          text,
          time: new Date().toTimeString().slice(0, 8),
          kind: "chat",
          channel,
        });
        return;
      }
      await call<void>("chat_send", { channel, text });
    },

    /** Start the room's agents on a hidden console; returns what was launched. */
    async startAgents(project: string | null): Promise<string[]> {
      if (!isTauri()) throw new Error("Start agents from the desktop app, not the browser preview.");
      return call<string[]>("chat_start_agents", { project });
    },

    /** Free mode: add the managed free-model agent to a room that is already awake. */
    async startFree(project: string | null): Promise<string | null> {
      if (!isTauri()) return null;
      return call<string | null>("chat_start_free", { project });
    },

    /** The agents Rigel owns. Anything else in the room was started by hand. */
    async launched(): Promise<AgentChatProcess[]> {
      if (!isTauri()) return [];
      return call<AgentChatProcess[]>("chat_launched");
    },

    async harnesses(project: string | null, force = false): Promise<AgentChatHarness[]> {
      if (!isTauri()) {
        return [
          {
            name: "claude",
            label: "Claude",
            api: false,
            ready: true,
            status: "usable",
            evidence: "Browser preview fixture",
            remediation: null,
            credential: null,
          },
          {
            name: "codex",
            label: "Codex",
            api: false,
            ready: true,
            status: "usable",
            evidence: "Browser preview fixture",
            remediation: null,
            credential: null,
          },
          {
            name: "phi4",
            label: "Phi4 (local)",
            api: true,
            ready: true,
            status: "usable",
            evidence: "Browser preview fixture",
            remediation: null,
            credential: null,
          },
        ];
      }
      return call<AgentChatHarness[]>("chat_harnesses", { project, force });
    },

    async startAgent(project: string | null, name: string): Promise<string> {
      if (!isTauri()) throw new Error("Start agents from the desktop app, not the browser preview.");
      return call<string>("chat_start_agent", { project, name });
    },

    async stopAgent(name: string): Promise<string> {
      if (!isTauri()) throw new Error("Stop agents from the desktop app, not the browser preview.");
      return call<string>("chat_stop_agent", { name });
    },
  },

  /** Q268: run the project's detected test command and return its parsed result. */
  tests: {
    async run(project: string): Promise<TestRun> {
      if (!isTauri()) return previewTestRun();
      return call<TestRun>("tests_run", { project });
    },
  },

  /**
   * A cheap model reads the draft and returns questions and a tighter brief. It never sends
   * anything: the card shows the result and the send button stays the user's.
   */
  refine: {
    async run(project: string | null, request: RefineRequest): Promise<PromptRefinement> {
      if (!isTauri()) return previewRefinement(request);
      return call<PromptRefinement>("prompt_refine", { project, request });
    },
  },

  /** The themed tray panel: live agents, spend and Rigel's process-tree load. */
  tray: {
    async snapshot(): Promise<TraySnapshot> {
      if (!isTauri()) {
        return {
          project: null,
          project_name: null,
          sessions: 0,
          agents_running: 0,
          agents_hidden: 0,
          spend_micros: 0,
          cpu_percent: 0,
          memory_bytes: 0,
          power: "idle",
          power_note: "The tray runs in the desktop app.",
          agents: [],
          updated_at: Date.now(),
        };
      }
      return call<TraySnapshot>("tray_snapshot");
    },

    async open(): Promise<void> {
      if (!isTauri()) return;
      return call<void>("tray_open");
    },

    async hide(): Promise<void> {
      if (!isTauri()) return;
      return call<void>("tray_hide");
    },

    async quit(): Promise<void> {
      if (!isTauri()) return;
      return call<void>("tray_quit");
    },
  },

  /** The curated plugin catalogue, installed trees and the harness skill copies. */
  plugins: {
    async catalog(
      query = "",
      category: string | null = null,
      tier: string | null = null,
    ): Promise<PluginCatalog> {
      if (!isTauri()) return { entries: [], categories: [], updated: "preview" };
      return call<PluginCatalog>("plugins_catalog", { query, category, tier });
    },

    async list(): Promise<PluginsState> {
      if (!isTauri()) return { plugins: [], broken: [], enabled_tools: [] };
      return call<PluginsState>("plugins_list");
    },

    async install(name: string): Promise<unknown> {
      if (!isTauri()) throw new Error("Install plugins in the desktop app.");
      return call<unknown>("plugins_install", { name });
    },

    async remove(name: string): Promise<boolean> {
      if (!isTauri()) return false;
      return call<boolean>("plugins_remove", { name });
    },

    async setEnabled(name: string, enabled: boolean): Promise<PluginRow> {
      if (!isTauri()) throw new Error("Enable plugins in the desktop app.");
      return call<PluginRow>("plugins_set_enabled", { name, enabled });
    },
  },

  /** Roadmap 144: probe the project's declared MCP servers on demand. */
  async mcpHealth(project: string): Promise<McpServerHealth[]> {
    if (!isTauri()) return PREVIEW_MCP_HEALTH;
    return call<McpServerHealth[]>("mcp_health", { project });
  },

  /** The embedded MCP catalogue and its project-local installer. */
  mcpCatalog: {
    async search(query: string, limit = 250, offset = 0): Promise<McpServer[]> {
      if (!isTauri()) throw new Error("The MCP catalogue runs in the desktop app.");
      return call<McpServer[]>("mcp_catalog_search", { query, limit, offset });
    },

    async count(query: string): Promise<number> {
      if (!isTauri()) throw new Error("The MCP catalogue runs in the desktop app.");
      return call<number>("mcp_catalog_count", { query });
    },

    async install(project: string, id: string): Promise<McpInstallReport> {
      if (!isTauri()) throw new Error("Install MCP servers in the desktop app.");
      return call<McpInstallReport>("mcp_install", { project, id });
    },

    async installed(project: string, id: string): Promise<boolean> {
      if (!isTauri()) return false;
      return call<boolean>("mcp_installed", { project, id });
    },
  },
};

/** The real shelf: SQLite FTS5 and the blob store, through the Rust core. */
export class TauriSkillSource implements SkillSource {
  readonly label = "index.db";

  async search(query: string, limit = 100, offset = 0): Promise<SkillRow[]> {
    return call<SkillRow[]>("skills_search", { query, limit, offset });
  }

  async count(query: string): Promise<number> {
    return call<number>("skills_count", { query });
  }

  async load(id: string): Promise<SkillDetail> {
    return call<SkillDetail>("skills_load", { id });
  }

  async status(): Promise<SourceStatus[]> {
    return call<SourceStatus[]>("skills_status");
  }

  async indexRepo(repo: string): Promise<IngestReport> {
    return call<IngestReport>("skills_index", { repo, force: false });
  }

  async refresh(): Promise<RefreshReport> {
    return call<RefreshReport>("skills_refresh");
  }

  async install(id: string): Promise<SkillInstallReport> {
    return call<SkillInstallReport>("skills_install", { id });
  }

  async installed(id: string): Promise<boolean> {
    return call<boolean>("skills_installed", { id });
  }
}

/** Browser preview only: a canned refinement, labelled as such so no one mistakes it for a call. */
function previewRefinement(request: RefineRequest): PromptRefinement {
  const draft = request.draft.trim();
  const first = request.round === 0;
  return {
    questions: first
      ? ["Which file or crate should this change land in?", "What must stay unchanged?"]
      : [],
    refined: first
      ? draft
      : `${draft}\n\nName the files you change, keep the existing tests passing, and run the relevant test before reporting back.`,
    reads: [],
    read_notes: [],
    notes: ["Browser preview: no model was called and nothing was billed."],
    model: "preview",
    usage: {
      input_tokens: 0,
      cached_input_tokens: 0,
      output_tokens: 0,
      reasoning_tokens: 0,
      cost_micros: 0,
      provider_micros: null,
      source: "estimated",
    },
  };
}

/** Browser preview only: one canned run so the counts and output link are inspectable. */
function previewTestRun(): TestRun {
  return {
    command: "cargo test --workspace",
    ok: false,
    exit_code: 101,
    passed: 418,
    failed: 1,
    truncated: false,
    timed_out: false,
    duration_ms: 42_700,
    output: [
      "running 419 tests",
      "test result: FAILED. 418 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
      "",
      "failures:",
      "    resume::writes_are_durable_before_the_tree_hash",
      "",
      "error: test failed, to rerun pass `--test resume`",
    ].join("\n"),
  };
}

/** Timestamps are epoch milliseconds everywhere in Rigel; render them as UTC minutes. */
export function when(ms: number | null | undefined): string {
  if (!ms) return "never";
  return new Date(ms).toISOString().slice(0, 16).replace("T", " ");
}

export function compact(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(value);
}
