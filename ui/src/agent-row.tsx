/**
 * Roadmap 58: one roster row with its own model pair.
 *
 * The resolved pair is the one the next turn will use. A role preset owns the row outright
 * (D94): the picker shows the preset's pair and is disabled, because a click there could not
 * change what the turn sends. Choosing `custom` in the preset menu leaves the preset and keeps
 * the pair it resolved to, so the row does not jump when it becomes the person's.
 */

import { ModelControls } from "./modelpicker";
import type { Agent, RolePreset } from "./api";

export type AgentRouteSource = "preset" | "agent" | "request" | "saved";

export interface AgentRoute {
  model: string;
  provider?: string;
  effort: string;
  source: AgentRouteSource;
}

export interface AgentFallback {
  model: string;
  provider?: string;
  effort: string;
}

/** The same ladder as `rigel_core::agent::routing::for_agent`, for the label and the picker. */
export function resolveAgentRoute(
  agent: Agent | null | undefined,
  presets: RolePreset[],
  fallback: AgentFallback,
): AgentRoute {
  const role = agent?.role?.trim().toLowerCase();
  const preset = role ? presets.find((row) => row.id.toLowerCase() === role) : undefined;
  if (preset) {
    return { model: preset.model, effort: preset.effort, source: "preset" };
  }
  if (agent?.model?.trim()) {
    return {
      model: agent.model,
      provider: agent.provider ?? undefined,
      effort: agent.effort ?? fallback.effort,
      source: "agent",
    };
  }
  if (fallback.model.trim()) {
    return {
      model: fallback.model,
      provider: fallback.provider,
      effort: fallback.effort,
      source: "request",
    };
  }
  return {
    model: fallback.model,
    provider: fallback.provider,
    effort: fallback.effort,
    source: "saved",
  };
}

const SOURCE_LABEL: Record<AgentRouteSource, string> = {
  preset: "preset",
  agent: "this row",
  request: "composer",
  saved: "settings",
};

export function AgentRow({
  agent,
  nested,
  selected,
  presets,
  chatOnly,
  busy = false,
  observed = false,
  working,
  fallback,
  onToggle,
  onConfigure,
  onRole,
  onChatOnly,
}: {
  agent: Agent;
  nested: boolean;
  selected: boolean;
  presets: RolePreset[];
  chatOnly: boolean;
  busy?: boolean;
  observed?: boolean;
  working?: string;
  fallback: AgentFallback;
  onToggle: () => void;
  onConfigure: (model: string, provider: string, effort: string) => void;
  onRole: (role: string) => void;
  onChatOnly: (chatOnly: boolean) => void;
}) {
  const route = resolveAgentRoute(agent, presets, fallback);
  const presetOwnsRow = route.source === "preset";
  const name = agent.role ?? "agent";

  return (
    <div
      className={`row agent-row${nested ? " nested" : ""}${selected ? " on" : ""}`}
      data-agent-id={agent.id}
      data-route-source={route.source}
    >
      <button
        type="button"
        className="agent-row-main"
        onClick={onToggle}
        aria-pressed={selected}
        title={
          nested
            ? "A subagent: click to watch its transcript in the main pane; up to two can be open."
            : "Click to watch this agent's transcript; up to two can be open."
        }
      >
        <span className={`status-dot ${agent.status}`} title={agent.status} />
        <span className="row-text">
          <span className="row-name">
            {nested ? (
              <span className="nest-mark" aria-hidden="true">
                ↳
              </span>
            ) : null}
            {name}
            <span className="tag">{agent.harness}</span>
            {observed ? <span className="tag observed">observed only</span> : null}
            {selected ? <span className="tag watching">watching</span> : null}
          </span>
          <span className="row-desc" title={working}>
            {working ?? `${route.model || "no model"} · ${route.effort}`}
          </span>
        </span>
        <span className="row-meta">
          {agent.claims_held > 0 ? (
            <span
              className={`claim-count${agent.claims_stale > 0 ? " stale" : ""}`}
              title={`${agent.claims_held} file claim(s) held${
                agent.claims_stale > 0 ? `, ${agent.claims_stale} stale` : ""
              } — see Diff`}
            >
              {agent.claims_held} claimed
              {agent.claims_stale > 0 ? ` · ${agent.claims_stale} stale` : ""}
            </span>
          ) : null}
          ${(agent.spend_micros / 1_000_000).toFixed(6)}
        </span>
      </button>

      <div className="agent-row-config">
        <ModelControls
          model={route.model}
          provider={route.provider}
          effort={route.effort}
          chatOnly={chatOnly}
          disabled={busy || presetOwnsRow}
          label={presetOwnsRow ? "preset" : "model"}
          heading={`Model for ${name}`}
          title={
            presetOwnsRow
              ? "The role preset owns this pair. Choose custom in the role menu to pick one."
              : "This row's model and effort; overrides the composer for its turns"
          }
          onModel={(model, provider) => onConfigure(model, provider, route.effort)}
          onEffort={(effort) => onConfigure(route.model, route.provider ?? "", effort)}
          onChatOnly={onChatOnly}
        />
        <select
          className="agent-role-select"
          aria-label={`Role preset for ${name}`}
          value={agent.role ?? ""}
          disabled={busy}
          onChange={(event) => onRole(event.target.value)}
          title="A role preset overrides this row's model and effort; custom keeps the current pair"
        >
          <option value="">custom</option>
          {agent.role && !presets.some((preset) => preset.id === agent.role) ? (
            <option value={agent.role}>{agent.role} (custom)</option>
          ) : null}
          {presets.map((preset) => (
            <option key={preset.id} value={preset.id}>
              {preset.title} · {preset.effort}
            </option>
          ))}
        </select>
        <span className="tag agent-route-source" title="Where this row's model pair comes from">
          {SOURCE_LABEL[route.source]}
        </span>
      </div>
    </div>
  );
}
