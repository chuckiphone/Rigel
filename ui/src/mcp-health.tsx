/**
 * Roadmap 144: on-demand health for the project's declared MCP servers.
 *
 * The rows come from the same core probe the window's tools use. A failed handshake is one row
 * with its error, not a hidden server, and refreshing re-runs every probe without a reload.
 */

import { useCallback, useEffect, useState } from "react";

import { api, type McpServerHealth } from "./api";

export function healthSummary(rows: McpServerHealth[]) {
  return {
    ready: rows.filter((row) => row.state === "ready").length,
    failed: rows.filter((row) => row.state === "failed").length,
    unsupported: rows.filter((row) => row.state === "unsupported").length,
  };
}

/** One server's row, exported so the renderer can be checked without a live project. */
export function HealthRow({ row }: { row: McpServerHealth }) {
  return (
    <li className={`mcp-health-row ${row.state}`} data-state={row.state}>
      <div className="mcp-health-row-head">
        <strong className="mcp-health-name">{row.name}</strong>
        <span className={`pill ${row.state}`}>{row.state}</span>
      </div>
      <p className="mcp-health-facts fine">
        {row.latency_ms === null ? "no probe" : `${row.latency_ms} ms`}
        {" · "}
        {row.protocol_version ? `protocol ${row.protocol_version}` : "no protocol"}
        {" · "}
        {row.tool_count} tool{row.tool_count === 1 ? "" : "s"}
      </p>
      {row.last_error ? (
        <p className="mcp-health-error" role="status">
          {row.last_error}
        </p>
      ) : null}
    </li>
  );
}

export default function McpHealthPanel({ project }: { project: string | null }) {
  const [rows, setRows] = useState<McpServerHealth[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      // The browser preview has no project root; the API serves a fixture there.
      setRows(await api.mcpHealth(project ?? ""));
      setLoaded(true);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }, [project]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const summary = healthSummary(rows);
  return (
    <section className="mcp-health" role="region" aria-label="MCP health">
      <div className="mcp-health-head">
        <div>
          <p className="eyebrow">Project servers</p>
          <h3>MCP health</h3>
        </div>
        <button
          type="button"
          className="chip"
          onClick={() => void refresh()}
          disabled={busy}
          aria-label="Refresh MCP health"
        >
          {busy ? "Probing…" : "Refresh health"}
        </button>
      </div>
      {error ? (
        <p className="mcp-health-error" role="alert">
          {error}
        </p>
      ) : null}
      <p className="mcp-health-summary fine" data-testid="mcp-health-summary">
        {loaded
          ? `${summary.ready} ready · ${summary.failed} failed · ${summary.unsupported} unsupported`
          : "Not probed yet"}
      </p>
      {rows.length > 0 ? (
        <ul className="mcp-health-list">
          {rows.map((row, index) => (
            <HealthRow key={`${row.name}-${index}`} row={row} />
          ))}
        </ul>
      ) : loaded ? (
        <p className="fine">No MCP servers are declared in this project.</p>
      ) : null}
    </section>
  );
}
