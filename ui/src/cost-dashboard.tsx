/**
 * Roadmap 136: the cost dashboard.
 *
 * The project database is the only source of money. Every group is summed back against the report
 * total in micro-dollars, so a mismatch is visible instead of a rounded total that quietly lies.
 * The same report object feeds the screen, the CSV and the JSON export.
 */

import { useState } from "react";
import type { SpendReport, SpendRow } from "./api";

export const dollars = (micros: number): string => `$${(micros / 1_000_000).toFixed(2)}`;

export interface GroupCheck {
  name: string;
  rows: number;
  micros: number;
  turns: number;
  delta: number;
}

export interface Reconciliation {
  ok: boolean;
  total: number;
  turns: number;
  groups: GroupCheck[];
}

/** Exact micro-dollar arithmetic: a cent is 10,000 micros, and this checks all of them. */
export function reconcile(report: SpendReport): Reconciliation {
  const groups: GroupCheck[] = [
    ["day", report.by_day],
    ["model", report.by_model],
    ["agent", report.by_agent],
    ["session", report.by_session],
  ].map(([name, rows]) => {
    const list = rows as SpendRow[];
    const micros = list.reduce((sum, row) => sum + row.micros, 0);
    const turns = list.reduce((sum, row) => sum + row.turns, 0);
    return { name: name as string, rows: list.length, micros, turns, delta: micros - report.total_micros };
  });
  return {
    ok: groups.every((group) => group.delta === 0 && group.turns === report.turns),
    total: report.total_micros,
    turns: report.turns,
    groups,
  };
}

/** One flat table of every group; the same rows the window draws. */
export function costCsv(report: SpendReport): string {
  const lines = ["group,key,label,micros,turns"];
  for (const [name, rows] of [
    ["day", report.by_day],
    ["model", report.by_model],
    ["agent", report.by_agent],
    ["session", report.by_session],
  ] as const) {
    for (const row of rows) {
      lines.push(
        [name, row.key, row.label, row.micros, row.turns]
          .map((value) => `"${String(value).replace(/"/g, '""')}"`)
          .join(","),
      );
    }
  }
  return `${lines.join("\n")}\n`;
}

/** Pretty JSON of the exact report object; no derived or rounded values. */
export function costJson(report: SpendReport): string {
  return JSON.stringify(report, null, 2);
}

function Group({ name, rows }: { name: string; rows: SpendRow[] }) {
  return (
    <div className="cost-group">
      <h4>by {name}</h4>
      {rows.length === 0 ? (
        <p className="fine">No recorded turns.</p>
      ) : (
        <ul>
          {rows.map((row) => (
            <li key={row.key}>
              <span className="mono cost-label">{row.label}</span>
              <span className="mono">{dollars(row.micros)}</span>
              <span className="faint">{row.turns} turns</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export default function CostDashboard({
  report,
  capMicros = null,
}: {
  report: SpendReport;
  capMicros?: number | null;
}) {
  const [copied, setCopied] = useState<string | null>(null);
  const check = reconcile(report);
  const percent =
    capMicros && capMicros > 0 ? Math.min(100, Math.round((report.total_micros / capMicros) * 100)) : null;

  const copy = async (label: string, body: string) => {
    try {
      await navigator.clipboard.writeText(body);
      setCopied(`${label} copied.`);
    } catch {
      setCopied(`Could not copy ${label.toLowerCase()} from this window.`);
    }
  };

  return (
    <section className="cost-dashboard" aria-label="Cost dashboard">
      <div className="cost-head">
        <div>
          <span className="eyebrow">recorded spend</span>
          <strong className="cost-total">{dollars(report.total_micros)}</strong>
          <span className="fine">
            {report.turns} {report.turns === 1 ? "turn" : "turns"}
          </span>
        </div>
        <div className="cost-actions">
          <button type="button" className="chip" onClick={() => void copy("CSV", costCsv(report))}>
            Copy CSV
          </button>
          <button type="button" className="chip" onClick={() => void copy("JSON", costJson(report))}>
            Copy JSON
          </button>
        </div>
      </div>
      {copied ? (
        <p className="fine" role="status">
          {copied}
        </p>
      ) : null}

      {percent === null ? (
        <p className="fine">No daily cap set for this project.</p>
      ) : (
        <div className="cost-cap">
          <span className="fine">
            {percent}% of ${(capMicros! / 1_000_000).toFixed(2)} cap
          </span>
          <div className="cost-cap-track" role="meter" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
            <div className="cost-cap-fill" style={{ width: `${percent}%` }} />
          </div>
        </div>
      )}

      <p className={`cost-reconcile ${check.ok ? "ok" : "bad"}`} role="status">
        {check.ok
          ? `Totals reconcile: day, model, agent and session all sum to ${dollars(check.total)}.`
          : `Totals disagree: ${check.groups.filter((group) => group.delta !== 0).map((group) => `${group.name} ${dollars(group.delta)}`).join(", ")}.`}
      </p>

      <div className="cost-groups">
        <Group name="day" rows={report.by_day} />
        <Group name="model" rows={report.by_model} />
        <Group name="agent" rows={report.by_agent} />
      </div>

      <table className="cost-ledger">
        <caption>Ledger · every recorded group</caption>
        <thead>
          <tr>
            <th scope="col">group</th>
            <th scope="col">label</th>
            <th scope="col">amount</th>
            <th scope="col">turns</th>
          </tr>
        </thead>
        <tbody>
          {check.groups.flatMap((group) =>
            (group.name === "day"
              ? report.by_day
              : group.name === "model"
                ? report.by_model
                : group.name === "agent"
                  ? report.by_agent
                  : report.by_session
            ).map((row) => (
              <tr key={`${group.name}-${row.key}`}>
                <td>{group.name}</td>
                <td className="mono">{row.label}</td>
                <td className="mono">{dollars(row.micros)}</td>
                <td>{row.turns}</td>
              </tr>
            )),
          )}
        </tbody>
      </table>
    </section>
  );
}

/** Preview fixture: all four groupings sum to the same 123,456 micros. */
export const PREVIEW_SPEND: SpendReport = {
  total_micros: 123_456,
  turns: 4,
  by_day: [
    { key: "2026-11-25", label: "2026-11-25", micros: 90_000, turns: 3 },
    { key: "2026-11-26", label: "2026-11-26", micros: 33_456, turns: 1 },
  ],
  by_model: [
    { key: "openrouter/fast", label: "openrouter/fast", micros: 70_000, turns: 2 },
    { key: "openrouter/deep", label: "openrouter/deep", micros: 53_456, turns: 2 },
  ],
  by_agent: [
    { key: "a1", label: "implementer · a1", micros: 80_000, turns: 3 },
    { key: "b2", label: "reviewer · b2", micros: 43_456, turns: 1 },
  ],
  by_session: [
    { key: "s1", label: "parser fix", micros: 123_456, turns: 4 },
  ],
};
