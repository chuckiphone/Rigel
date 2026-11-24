/**
 * Settings → Audit log (roadmap 83).
 *
 * The ledger itself is written by the core when an approval is decided. This page is the
 * reviewer's read side: one filter for each identity on a record, a visible chain verdict, and
 * a CSV that is built from the same filtered rows. Nothing here can approve, deny or edit.
 */

import { useEffect, useState } from "react";

import { api, when, type AuditFilter, type AuditRecord, type AuditView } from "./api";
import {
  EMPTY_AUDIT_FILTER,
  auditFilterFromHash,
  auditHash,
  auditFilterIsEmpty,
} from "./audit-filter";

interface Props {
  project: string | null;
  onNotice: (notice: string) => void;
}

const FIELDS: { key: keyof AuditFilter; label: string; placeholder: string }[] = [
  { key: "project", label: "Project", placeholder: "path or folder name" },
  { key: "session", label: "Session", placeholder: "session id" },
  { key: "agent", label: "Agent", placeholder: "agent id" },
  { key: "turn", label: "Turn", placeholder: "turn id" },
];

function requestLabel(request: unknown): string {
  if (request && typeof request === "object" && "command" in request) {
    const command = (request as { command?: unknown }).command;
    if (typeof command === "string") return command;
  }
  try {
    return JSON.stringify(request);
  } catch {
    return String(request);
  }
}

function clip(value: string, length = 140): string {
  return value.length > length ? `${value.slice(0, length - 1)}…` : value;
}

export default function AuditLogPage({ project, onNotice }: Props) {
  const [filter, setFilter] = useState<AuditFilter>(() => auditFilterFromHash(window.location.hash));
  const [view, setView] = useState<AuditView | null>(null);
  const [busy, setBusy] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    window.history.replaceState(null, "", auditHash(filter));
  }, [filter]);

  useEffect(() => {
    if (!project) {
      setView(null);
      setError(null);
      return;
    }
    let live = true;
    setBusy(true);
    const timer = window.setTimeout(() => {
      api.audit
        .query(project, filter)
        .then((next) => {
          if (!live) return;
          setView(next);
          setError(null);
        })
        .catch((failure: unknown) => {
          if (!live) return;
          setView(null);
          setError(failure instanceof Error ? failure.message : String(failure));
        })
        .finally(() => {
          if (live) setBusy(false);
        });
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [project, filter]);

  const setField = (key: keyof AuditFilter, value: string) => {
    setFilter((current) => ({ ...current, [key]: value }));
  };

  const exportCsv = async () => {
    if (!project || !view) return;
    setExporting(true);
    try {
      const report = await api.audit.exportCsv(project, filter);
      const blob = new Blob([report.csv], { type: "text/csv;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `rigel-audit-${new Date().toISOString().replace(/[:.]/g, "-")}.csv`;
      anchor.style.display = "none";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
      onNotice(`Exported ${report.rows} of ${report.total} decisions (${report.sha256.slice(0, 12)}…).`);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setExporting(false);
    }
  };

  const status = !view
    ? null
    : view.valid
      ? <span className="pill ok">chain intact</span>
      : (
          <span className="pill failed">
            tampered at line {view.first_bad?.line ?? "?"}
          </span>
        );

  return (
    <>
      <section className="settings-section audit-log">
        <div className="settings-head">
          <h3>Approval ledger</h3>
          {status}
        </div>
        <p className="fine">
          Every allow, deny and override in this project, oldest first. The rows come from the
          append-only hash chain; the full chain is what <code>rigel audit verify</code> checks.
          A filtered CSV is a view of these rows, not a separately verifiable export.
        </p>

        <div className="audit-filters">
          <label className="audit-query">
            <span>Search</span>
            <input
              type="search"
              value={filter.q}
              placeholder="command, kind, approval, request…"
              onChange={(event) => setField("q", event.target.value)}
            />
          </label>
          {FIELDS.map((field) => (
            <label key={field.key}>
              <span>{field.label}</span>
              <input
                value={filter[field.key]}
                placeholder={field.placeholder}
                onChange={(event) => setField(field.key, event.target.value)}
              />
            </label>
          ))}
          <label>
            <span>Decision</span>
            <select
              value={filter.decision}
              onChange={(event) => setField("decision", event.target.value)}
            >
              <option value="">All decisions</option>
              <option value="allowed">Allowed</option>
              <option value="denied">Denied</option>
            </select>
          </label>
        </div>

        <div className="audit-actions">
          <span className="fine" role="status">
            {!project
              ? "Open a project to read its ledger."
              : busy
                ? "Reading the ledger…"
                : view
                  ? `${view.matched} of ${view.total} decision${view.total === 1 ? "" : "s"}`
                  : "No ledger read yet."}
          </span>
          <button
            className="chip"
            type="button"
            onClick={() => setFilter({ ...EMPTY_AUDIT_FILTER })}
            disabled={auditFilterIsEmpty(filter)}
          >
            Clear filters
          </button>
          <button
            className="chip primary"
            type="button"
            onClick={() => void exportCsv()}
            disabled={!project || !view || !view.valid || exporting}
          >
            {exporting ? "Exporting…" : "Export filtered CSV"}
          </button>
        </div>

        {error ? (
          <p className="notice" role="alert">
            {error}
          </p>
        ) : null}
        {view && !view.valid ? (
          <p className="notice" role="alert">
            The chain is broken at line {view.first_bad?.line ?? "?"}:{" "}
            {view.first_bad?.reason ?? "the verifier gave no reason"}. Export is disabled because
            the rows cannot be shown as intact evidence.
          </p>
        ) : null}

        {view && view.records.length > 0 ? (
          <div className="audit-table-wrap">
            <table className="audit-table" aria-label="Approval ledger rows">
              <thead>
                <tr>
                  <th>When</th>
                  <th>Decision</th>
                  <th>Kind</th>
                  <th>Project</th>
                  <th>Session</th>
                  <th>Agent</th>
                  <th>Turn</th>
                  <th>Request</th>
                  <th>Scope</th>
                  <th>Approval</th>
                </tr>
              </thead>
              <tbody>
                {view.records.map((record: AuditRecord) => (
                  <tr key={record.approval_id}>
                    <td title={when(record.decided_at ?? record.requested_at)}>
                      {when(record.decided_at ?? record.requested_at)}
                    </td>
                    <td>
                      <span
                        className={`pill ${record.decision.includes("denied") ? "failed" : "ok"}`}
                      >
                        {record.decision}
                      </span>
                    </td>
                    <td>{record.kind}</td>
                    <td className="audit-path" title={view.project}>
                      {view.project}
                    </td>
                    <td className="audit-id" title={record.session_id}>
                      {record.session_id}
                    </td>
                    <td className="audit-id" title={record.agent_id}>
                      {record.agent_id}
                    </td>
                    <td className="audit-id" title={record.turn_id ?? "no turn"}>
                      {record.turn_id ?? "—"}
                    </td>
                    <td className="audit-request" title={requestLabel(record.request)}>
                      {clip(requestLabel(record.request))}
                    </td>
                    <td>{record.scope ?? "—"}</td>
                    <td className="audit-id" title={record.approval_id}>
                      {record.approval_id}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : view && !busy ? (
          <p className="fine">
            {view.total === 0
              ? "No approvals have been recorded in this project yet."
              : "No approvals match these filters."}
          </p>
        ) : null}
      </section>
    </>
  );
}