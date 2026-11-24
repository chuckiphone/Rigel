import { useEffect, useMemo, useState } from "react";

import {
  api,
  type CodeGraph,
  type GraphEdge,
  type GraphNode,
  type GraphWatchStatus,
  type ProjectInfo,
} from "./api";

type Point = { x: number; y: number };

const WIDTH = 1000;
const HEIGHT = 720;

function hash(text: string): number {
  let value = 2166136261;
  for (let index = 0; index < text.length; index += 1) {
    value ^= text.charCodeAt(index);
    value = Math.imul(value, 16777619);
  }
  return Math.abs(value);
}

function duration(unixSeconds: number): string {
  const seconds = Math.max(0, Math.round(Date.now() / 1000 - unixSeconds));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m ago`;
  return `${Math.round(seconds / 3600)}h ago`;
}

function neighbours(edges: GraphEdge[], id: string): { id: string; kind: string }[] {
  const out: { id: string; kind: string }[] = [];
  for (const edge of edges) {
    if (edge.from === id) out.push({ id: edge.to, kind: edge.kind });
    else if (edge.to === id) out.push({ id: edge.from, kind: edge.kind });
  }
  const seen = new Set<string>();
  return out.filter((row) => {
    const key = `${row.id}:${row.kind}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** A deterministic radial layout: files on the outer ring, their symbols around them. With a
 * focus, the focus sits at the centre and each BFS hop gets its own ring. */
function layout(
  nodes: GraphNode[],
  edges: GraphEdge[],
  focusId: string | null,
): Map<string, Point> {
  const positions = new Map<string, Point>();

  if (focusId) {
    const adjacency = new Map<string, string[]>();
    for (const edge of edges) {
      adjacency.set(edge.from, [...(adjacency.get(edge.from) ?? []), edge.to]);
      adjacency.set(edge.to, [...(adjacency.get(edge.to) ?? []), edge.from]);
    }
    const hop = new Map<string, number>([[focusId, 0]]);
    const queue = [focusId];
    while (queue.length > 0) {
      const current = queue.shift() as string;
      const distance = hop.get(current) ?? 0;
      if (distance >= 3) continue;
      for (const next of adjacency.get(current) ?? []) {
        if (hop.has(next)) continue;
        hop.set(next, distance + 1);
        queue.push(next);
      }
    }
    positions.set(focusId, { x: WIDTH / 2, y: HEIGHT / 2 });
    for (const ring of [1, 2, 3]) {
      const members = nodes
        .filter((node) => node.id !== focusId && (hop.get(node.id) ?? ring) === ring)
        .sort((a, b) => a.label.localeCompare(b.label));
      const radius = 95 + ring * 95;
      members.forEach((node, index) => {
        const angle = (index / Math.max(1, members.length)) * Math.PI * 2 - Math.PI / 2;
        positions.set(node.id, {
          x: WIDTH / 2 + Math.cos(angle) * radius,
          y: HEIGHT / 2 + Math.sin(angle) * radius,
        });
      });
    }
    for (const node of nodes) {
      if (!positions.has(node.id)) {
        const angle = (hash(node.id) % 360) * (Math.PI / 180);
        positions.set(node.id, {
          x: WIDTH / 2 + Math.cos(angle) * 430,
          y: HEIGHT / 2 + Math.sin(angle) * 320,
        });
      }
    }
    return positions;
  }

  const files = nodes.filter((node) => node.kind === "file").sort((a, b) => a.path.localeCompare(b.path));
  const symbols = nodes.filter((node) => node.kind === "symbol");
  const fileAngle = new Map<string, number>();
  files.forEach((file, index) => {
    const angle = (index / Math.max(1, files.length)) * Math.PI * 2 - Math.PI / 2;
    fileAngle.set(file.path, angle);
    positions.set(file.id, {
      x: WIDTH / 2 + Math.cos(angle) * 330,
      y: HEIGHT / 2 + Math.sin(angle) * 255,
    });
  });

  const perFile = new Map<string, GraphNode[]>();
  for (const symbol of symbols) {
    perFile.set(symbol.path, [...(perFile.get(symbol.path) ?? []), symbol]);
  }
  for (const [path, members] of perFile) {
    const angle = fileAngle.get(path) ?? (hash(path) % 360) * (Math.PI / 180);
    members.forEach((symbol, index) => {
      const spread = (index / Math.max(1, members.length)) * Math.PI * 0.9 - Math.PI * 0.45;
      const radius = 34 + (index % 4) * 13;
      positions.set(symbol.id, {
        x: WIDTH / 2 + Math.cos(angle + spread) * (330 - radius),
        y: HEIGHT / 2 + Math.sin(angle + spread) * (255 - radius * 0.8),
      });
    });
  }
  for (const node of nodes) {
    if (!positions.has(node.id)) {
      positions.set(node.id, { x: WIDTH / 2, y: HEIGHT / 2 });
    }
  }
  return positions;
}

function colour(node: GraphNode, selected: boolean): string {
  if (selected) return "var(--accent)";
  if (node.kind === "concept") return "var(--text)";
  return node.kind === "file" ? "var(--accent-text)" : "var(--muted)";
}

export default function CodeGraphView({ project }: { project: ProjectInfo | null }) {
  const [graph, setGraph] = useState<CodeGraph | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [focus, setFocus] = useState<string | null>(null);
  const [depth, setDepth] = useState(2);
  const [selected, setSelected] = useState<string | null>(null);
  const [watch, setWatch] = useState<GraphWatchStatus | null>(null);

  useEffect(() => {
    if (!project) {
      setGraph(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    void api
      .codeGraph(project.root, focus, depth)
      .then((next) => {
        if (cancelled) return;
        setGraph(next);
        setSelected((current) => (current && next.nodes.some((node) => node.id === current) ? current : null));
      })
      .catch((problem: unknown) => {
        if (!cancelled) setError(problem instanceof Error ? problem.message : String(problem));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [project, focus, depth, watch?.revision]);

  useEffect(() => {
    if (!project) return;
    let live = true;
    const read = () => {
      void api
        .codeGraphWatchStatus(project.root)
        .then((next) => live && setWatch(next))
        .catch(() => undefined);
    };
    read();
    const timer = window.setInterval(read, 20_000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [project]);

  const focusId = useMemo(() => {
    if (!focus || !graph) return null;
    const needle = focus.trim().toLowerCase();
    if (!needle) return null;
    return (
      graph.nodes.find((node) => node.label.toLowerCase() === needle)?.id ??
      graph.nodes.find((node) => node.label.toLowerCase().includes(needle))?.id ??
      null
    );
  }, [graph, focus]);
  const positions = useMemo(
    () => (graph ? layout(graph.nodes, graph.edges, focusId) : new Map<string, Point>()),
    [graph, focusId],
  );
  const byId = useMemo(() => new Map((graph?.nodes ?? []).map((node) => [node.id, node])), [graph]);

  if (!project) {
    return (
      <main id="main" tabIndex={-1} className="graph-view graph-empty">
        <p className="eyebrow">Code graph</p>
        <h2>Open a project to draw its graph</h2>
        <p className="muted">
          Rigel reads the files locally, finds their top-level symbols and imports, and shows what
          connects them. Nothing leaves this machine.
        </p>
      </main>
    );
  }

  const selectedNode = selected ? byId.get(selected) ?? null : null;
  const linked = selectedNode && graph ? neighbours(graph.edges, selectedNode.id) : [];

  return (
    <main id="main" tabIndex={-1} className="graph-view">
      <header className="graph-head">
        <div>
          <p className="eyebrow">{project.name} · local, read-only</p>
          <h2>Code graph</h2>
          <p className="lede">
            {graph
              ? `${graph.nodes.length.toLocaleString()} nodes · ${graph.edges.length.toLocaleString()} edges · ${graph.files.toLocaleString()} files`
              : "Scanning the project…"}
            {graph?.truncated ? " · bounded view, some files or nodes were left out" : ""}
          </p>
          {watch ? (
            <p className="graph-watch">
              {watch.error
                ? `watcher: ${watch.error}`
                : `indexed ${duration(watch.updatedUnix)} · ${watch.concepts} concepts`}
              {!watch.error && watch.agent ? ` · ${watch.agent}` : ""}
            </p>
          ) : null}
        </div>
        <form
          className="graph-search"
          onSubmit={(event) => {
            event.preventDefault();
            setFocus(query.trim() || null);
          }}
        >
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Focus a file or symbol…"
            aria-label="Focus a file or symbol"
          />
          <button type="submit" className="chip">
            Focus
          </button>
          <button
            type="button"
            className="chip"
            onClick={() => {
              setFocus(null);
              setQuery("");
              setSelected(null);
            }}
          >
            All
          </button>
          <label className="graph-depth">
            depth
            <select value={depth} onChange={(event) => setDepth(Number(event.target.value))}>
              <option value={1}>1</option>
              <option value={2}>2</option>
              <option value={3}>3</option>
            </select>
          </label>
        </form>
      </header>

      {error ? (
        <p className="banner bad" role="alert">
          {error}
        </p>
      ) : null}
      {loading && !graph ? <p className="empty subtle">Reading the project…</p> : null}

      <div className="graph-body">
        <div className="graph-canvas-wrap">
          <svg
            className="graph-canvas"
            viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
            role="img"
            aria-label="Project code graph"
            onClick={() => setSelected(null)}
          >
            {graph?.edges.map((edge) => {
              const from = positions.get(edge.from);
              const to = positions.get(edge.to);
              if (!from || !to) return null;
              const active =
                selected !== null && (edge.from === selected || edge.to === selected);
              return (
                <line
                  key={`${edge.from}:${edge.to}:${edge.kind}`}
                  x1={from.x}
                  y1={from.y}
                  x2={to.x}
                  y2={to.y}
                  stroke={active ? "var(--accent)" : "var(--hairline)"}
                  strokeWidth={active ? 1.6 : edge.kind === "imports" ? 1.1 : 0.7}
                  opacity={active ? 0.9 : edge.kind === "defines" ? 0.35 : 0.55}
                />
              );
            })}
            {graph?.nodes.map((node) => {
              const point = positions.get(node.id);
              if (!point) return null;
              const radius = node.kind === "file" ? 6 : node.kind === "concept" ? 5 : 3.6;
              const chosen = selected === node.id;
              const linkedToSelection =
                selected !== null &&
                graph.edges.some(
                  (edge) =>
                    (edge.from === selected && edge.to === node.id) ||
                    (edge.to === selected && edge.from === node.id),
                );
              return (
                <g key={node.id} className="graph-node">
                  <circle
                    cx={point.x}
                    cy={point.y}
                    r={chosen ? radius + 3 : linkedToSelection ? radius + 1.5 : radius}
                    fill={colour(node, chosen)}
                    opacity={chosen || linkedToSelection ? 1 : 0.82}
                    onClick={(event) => {
                      event.stopPropagation();
                      setSelected(node.id);
                    }}
                  >
                    <title>
                      {node.label}
                      {node.path && node.path !== node.label ? ` · ${node.path}` : ""}
                      {node.line ? `:${node.line}` : ""}
                    </title>
                  </circle>
                  {(node.kind !== "symbol" || chosen || linkedToSelection) && (
                    <text
                      x={point.x + radius + 4}
                      y={point.y + 3}
                      fontSize={11}
                      fill={chosen ? "var(--text)" : "var(--muted)"}
                    >
                      {node.label.length > 28 ? `${node.label.slice(0, 27)}…` : node.label}
                    </text>
                  )}
                </g>
              );
            })}
          </svg>
          {graph && graph.nodes.length === 0 ? (
            <p className="empty subtle">No source files matched the supported languages.</p>
          ) : null}
        </div>

        <aside className="graph-inspector" aria-label="Graph details">
          {selectedNode ? (
            <>
              <p className="eyebrow">{selectedNode.kind === "file" ? "File" : "Symbol"}</p>
              <h3>{selectedNode.label}</h3>
              <p className="muted graph-path">
                {selectedNode.path}
                {selectedNode.line ? `:${selectedNode.line}` : ""} · {selectedNode.language}
              </p>
              <div className="graph-actions">
                <button
                  className="chip"
                  onClick={() => {
                    setFocus(selectedNode.label);
                    setQuery(selectedNode.label);
                  }}
                >
                  Focus
                </button>
                <button className="chip" onClick={() => setSelected(null)}>
                  Clear
                </button>
              </div>
              <p className="eyebrow">Connected · {linked.length}</p>
              <ul className="graph-links">
                {linked.slice(0, 60).map((link) => {
                  const node = byId.get(link.id);
                  return (
                    <li key={`${link.id}:${link.kind}`}>
                      <button
                        className="graph-link"
                        onClick={() => setSelected(link.id)}
                        title={node?.path ?? link.id}
                      >
                        <span>{node?.label ?? link.id}</span>
                        <kbd>{link.kind}</kbd>
                      </button>
                    </li>
                  );
                })}
                {linked.length === 0 ? <li className="muted">Nothing links to this node.</li> : null}
              </ul>
            </>
          ) : (
            <>
              <p className="eyebrow">Read the graph</p>
              <h3>Structure, not a call log</h3>
              <p className="muted">
                Files and their top-level symbols are nodes. Imports, definitions and name
                references are the edges. Click a node to see what connects to it, or focus a
                symbol to keep its neighbourhood.
              </p>
              <ul className="graph-legend">
                <li>
                  <span className="graph-dot file" /> file
                </li>
                <li>
                  <span className="graph-dot symbol" /> symbol
                </li>
                <li>
                  <span className="graph-dot concept" /> concept, named by a free agent
                </li>
                <li>
                  <span className="graph-line imports" /> imports
                </li>
                <li>
                  <span className="graph-line references" /> references
                </li>
              </ul>
              <p className="muted">
                The scan is bounded to {graph?.files ?? 0} files and stops at generated trees. It
                reads names, not types; use the Diff and Session surfaces for what a change did.
              </p>
            </>
          )}
        </aside>
      </div>
    </main>
  );
}
