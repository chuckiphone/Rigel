/**
 * Roadmap 66: one table for the project ceiling and every role/agent grant.
 *
 * The project grant is stored in `.rigel/permissions.json`; roles are the four built-in presets
 * and each agent may add its own narrowing override. Every cell is an intersection: project,
 * then role, then override. An agent checkbox can only remove a tool from that agent; the role
 * and project cells decide what it could ever have.
 */

import { useEffect, useState } from "react";

import { api, type PermissionAgent, type PermissionMatrix } from "../api";

export default function PermissionsEditor({
  project,
  session,
  onNotice,
}: {
  project: string | null;
  session: string | null;
  onNotice: (notice: string) => void;
}) {
  const [matrix, setMatrix] = useState<PermissionMatrix | null>(null);
  const [draft, setDraft] = useState<string[]>([]);
  const [overrides, setOverrides] = useState<Record<string, string[] | null>>({});
  const [savedOverrides, setSavedOverrides] = useState<Record<string, string[] | null>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setMatrix(null);
    setError(null);
    setDraft([]);
    setOverrides({});
    setSavedOverrides({});
    if (!project) return;
    api
      .permissionsGet(project, session)
      .then((next) => {
        if (!live) return;
        setMatrix(next);
        setDraft(next.project_grant);
        const loaded: Record<string, string[] | null> = {};
        for (const agent of next.agents) loaded[agent.id] = agent.tool_overrides;
        setOverrides(loaded);
        setSavedOverrides(loaded);
      })
      .catch((failure: unknown) => {
        if (!live) return;
        setError(failure instanceof Error ? failure.message : String(failure));
      });
    return () => {
      live = false;
    };
  }, [project, session]);

  const selected = new Set(draft);
  const sameTools = (left: string[] | null | undefined, right: string[] | null | undefined) => {
    if (left == null || right == null) return left == null && right == null;
    return left.length === right.length && left.every((tool) => right.includes(tool));
  };
  const projectDirty =
    matrix !== null &&
    (matrix.project_grant.length !== draft.length ||
      matrix.project_grant.some((tool) => !selected.has(tool)));
  const agentsDirty =
    matrix?.agents.some(
      (agent) => !sameTools(overrides[agent.id], savedOverrides[agent.id]),
    ) ?? false;
  const dirty = projectDirty || agentsDirty;

  const roleFor = (role: string | null) =>
    role === null ? undefined : matrix?.roles.find((candidate) => candidate.id === role);
  /** The role's own allowance. A custom role (no preset) starts from every tool. */
  const roleTools = (role: string | null) => roleFor(role)?.requested ?? matrix?.all_tools ?? [];
  const overrideFor = (agentId: string) => overrides[agentId] ?? null;
  const roleAllows = (role: string | null, tool: string) =>
    selected.has(tool) && roleTools(role).includes(tool);
  const allows = (role: string | null, override: string[] | null, tool: string) =>
    roleAllows(role, tool) && (override === null || override.includes(tool));

  const grantAll = () => {
    if (!matrix) return;
    setDraft(matrix.all_tools);
  };

  /** Unchecking narrows this agent; checking back to its full role clears the override. */
  const toggleAgent = (agent: PermissionAgent, tool: string, checked: boolean) => {
    setOverrides((current) => {
      const stored = current[agent.id] ?? null;
      const defaults = roleTools(agent.role).filter((candidate) => selected.has(candidate));
      const next = new Set(stored ?? defaults);
      if (checked) next.add(tool);
      else next.delete(tool);
      const sameAsRole =
        next.size === defaults.length && defaults.every((candidate) => next.has(candidate));
      return { ...current, [agent.id]: sameAsRole ? null : [...next] };
    });
  };

  const save = async () => {
    if (!project || !matrix) return;
    const changedAgents = matrix.agents.filter(
      (agent) => !sameTools(overrides[agent.id], savedOverrides[agent.id]),
    );
    setBusy(true);
    setError(null);
    try {
      let next = await api.permissionsSet(project, session, draft);
      for (const agent of changedAgents) {
        next = await api.permissionsSetAgent(
          project,
          session ?? "",
          agent.id,
          overrideFor(agent.id),
        );
      }
      setMatrix(next);
      setDraft(next.project_grant);
      const loaded: Record<string, string[] | null> = {};
      for (const agent of next.agents) loaded[agent.id] = agent.tool_overrides;
      setOverrides(loaded);
      setSavedOverrides(loaded);
      onNotice(
        `Permission matrix saved: ${next.project_grant.length} of ${next.all_tools.length} tools granted for this project${
          changedAgents.length ? `; ${changedAgents.length} agent override(s)` : ""
        }.`,
      );
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  if (!project) {
    return (
      <section className="settings-section">
        <h3>Permission matrix</h3>
        <p className="fine">Open a project to see and edit its tool grants.</p>
      </section>
    );
  }

  if (!matrix) {
    return (
      <section className="settings-section">
        <h3>Permission matrix</h3>
        <p className="fine">
          {error ? `Could not read the project matrix: ${error}` : "Reading the project matrix…"}
        </p>
      </section>
    );
  }

  return (
    <>
      <section className="settings-section">
        <div className="settings-head">
          <h3>Project grant and role ceiling</h3>
          <span className="pill">
            {draft.length} of {matrix.all_tools.length} granted
          </span>
        </div>
        <p className="fine">
          The project grant is the ceiling for every agent. A role preset can narrow it by
          offering fewer tools, and an agent checkbox can narrow that agent further; neither can
          add a tool the project or the role withholds. Changes apply to the next turn; the
          current turn keeps the grant it started with.
        </p>
        <div className="permission-summary">
          {matrix.roles.map((role) => {
            const blocked = role.requested.filter((tool) => !selected.has(tool)).length;
            return (
              <span className="fine" key={role.id}>
                <strong>{role.title}</strong> {role.requested.length - blocked}/
                {role.requested.length}
                {blocked ? ` · ${blocked} narrowed by the project` : ""}
              </span>
            );
          })}
        </div>
        <div className="permission-matrix-wrap">
          <table className="permission-matrix" aria-label="Permission matrix">
            <thead>
              <tr>
                <th scope="col">Tool</th>
                <th scope="col">Project grant</th>
                {matrix.roles.map((role) => (
                  <th scope="col" key={role.id} title={`${role.title} preset`}>
                    {role.title}
                  </th>
                ))}
                {matrix.agents.map((agent) => (
                  <th scope="col" key={agent.id} title={`${agent.id} · ${agent.source}`}>
                    <span>{agent.role_title ?? "Custom"}</span>
                    <small>
                      {agent.id.slice(0, 8)}
                      {overrideFor(agent.id) ? " · narrowed" : ""}
                    </small>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {matrix.all_tools.map((tool) => (
                <tr key={tool}>
                  <th scope="row">
                    <code>{tool}</code>
                  </th>
                  <td>
                    <input
                      type="checkbox"
                      aria-label={`Grant ${tool} to the project`}
                      checked={selected.has(tool)}
                      disabled={busy}
                      onChange={(event) =>
                        setDraft((current) =>
                          event.target.checked
                            ? [...current, tool]
                            : current.filter((candidate) => candidate !== tool),
                        )
                      }
                    />
                  </td>
                  {matrix.roles.map((role) => {
                    const allowed = roleAllows(role.id, tool);
                    return (
                      <td key={role.id} data-allowed={allowed}>
                        <span
                          aria-label={`${role.title}: ${allowed ? "granted" : "not granted"}`}
                          title={
                            allowed
                              ? "Granted by the project and the role"
                              : role.requested.includes(tool)
                                ? "Narrowed by the project grant"
                                : "Not part of this role preset"
                          }
                        >
                          {allowed ? "✓" : "—"}
                        </span>
                      </td>
                    );
                  })}
                  {matrix.agents.map((agent) => {
                    const override = overrideFor(agent.id);
                    const roleAllowed = roleAllows(agent.role, tool);
                    const allowed = allows(agent.role, override, tool);
                    return (
                      <td key={agent.id} data-allowed={allowed}>
                        <input
                          type="checkbox"
                          aria-label={`Grant ${tool} to ${agent.id}`}
                          checked={allowed}
                          disabled={busy || !roleAllowed}
                          title={
                            allowed
                              ? `Granted through ${agent.source}`
                              : !selected.has(tool)
                                ? "Narrowed by the project grant"
                                : !roleTools(agent.role).includes(tool)
                                  ? "Not part of this agent's role"
                                  : "Narrowed by this agent's override"
                          }
                          onChange={(event) =>
                            toggleAgent(agent, tool, event.target.checked)
                          }
                        />
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="cap-form">
          <button className="primary" type="button" onClick={() => void save()} disabled={busy || !dirty}>
            {busy ? "Saving…" : "Save permission matrix"}
          </button>
          <button
            className="chip"
            type="button"
            onClick={grantAll}
            disabled={busy || draft.length === matrix.all_tools.length}
          >
            Grant every tool to the project
          </button>
          {dirty ? <span className="fine">unsaved changes</span> : null}
        </div>
        {error ? <p className="fine danger-text">{error}</p> : null}
        <p className="fine">
          Stored in <code>.rigel/permissions.json</code> and the agent's session row. An
          unreadable or unknown policy is refused rather than treated as “grant everything”; a
          malformed file during a turn denies every tool until it is fixed.
        </p>
      </section>
    </>
  );
}
