/**
 * Per-project retention. A preview is always shown before anything is deleted, and Apply is
 * enabled only when the typed confirmation repeats the exact counts and bytes on screen.
 */

import { useCallback, useEffect, useState } from "react";

import { api, type RetentionPolicy, type RetentionPreview } from "../api";

export function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}

/** The exact phrase Apply expects: the displayed totals, not a remembered value. */
export function confirmationText(preview: RetentionPreview): string {
  return `${preview.total_count} rows / ${preview.total_bytes} bytes`;
}

export function canApply(preview: RetentionPreview | null, confirmation: string): boolean {
  return preview !== null && preview.total_count > 0 && confirmation === confirmationText(preview);
}

/** The preview rows, exported so a renderer test can check the numbers without an IPC call. */
export function RetentionTable({ preview }: { preview: RetentionPreview }) {
  return (
    <table className="retention-table">
      <thead>
        <tr>
          <th>Category</th>
          <th>Rows</th>
          <th>Bytes</th>
        </tr>
      </thead>
      <tbody>
        {CATEGORIES.map(({ preview: key, label }) => (
          <tr key={key} data-category={key}>
            <td>{label}</td>
            <td>{preview[key].count}</td>
            <td>{formatBytes(preview[key].bytes)}</td>
          </tr>
        ))}
      </tbody>
      <tfoot>
        <tr data-category="total">
          <td>Total</td>
          <td>{preview.total_count}</td>
          <td>{formatBytes(preview.total_bytes)}</td>
        </tr>
      </tfoot>
    </table>
  );
}

const CATEGORIES = [
  { field: "turns_days", preview: "turns", label: "Turns" },
  { field: "blobs_days", preview: "blobs", label: "Blobs" },
  { field: "logs_days", preview: "logs", label: "Logs" },
  { field: "snapshots_days", preview: "snapshots", label: "Snapshots" },
] as const;

export default function RetentionSettings({ project }: { project: string | null }) {
  const [policy, setPolicy] = useState<RetentionPolicy>({
    turns_days: 90,
    blobs_days: 90,
    logs_days: 90,
    snapshots_days: 90,
  });
  const [preview, setPreview] = useState<RetentionPreview | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    if (!project) return;
    try {
      setPolicy(await api.retention.getPolicy(project));
      setPreview(await api.retention.preview(project));
      setError(null);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    }
  }, [project]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = async () => {
    if (!project) return;
    setBusy(true);
    try {
      await api.retention.setPolicy(project, policy);
      await refresh();
      setConfirmation("");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    if (!project || !preview || !canApply(preview, confirmation)) return;
    setBusy(true);
    try {
      await api.retention.apply(project, preview.total_count, preview.total_bytes);
      setConfirmation("");
      await refresh();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="settings-section" data-setting-key="retention" tabIndex={-1}>
      <header>
        <p className="eyebrow">Guardrails</p>
        <h2>Retention</h2>
        <p className="lede">
          Days to keep per category in this project. Zero disables a category. Nothing is deleted
          until you preview it and type the displayed totals.
        </p>
      </header>

      <div className="retention-days" role="group" aria-label="Retention days">
        {CATEGORIES.map(({ field, label }) => (
          <label key={field}>
            {label} (days)
            <input
              type="number"
              min={0}
              max={3650}
              aria-label={`${label} retention days`}
              value={policy[field]}
              onChange={(event) =>
                setPolicy((current) => ({ ...current, [field]: Number(event.target.value) }))
              }
            />
          </label>
        ))}
      </div>
      <div className="retention-actions">
        <button type="button" className="chip" onClick={() => void save()} disabled={busy || !project}>
          Save
        </button>
        <button type="button" className="chip" onClick={() => void refresh()} disabled={busy || !project}>
          Preview
        </button>
      </div>

      {preview ? (
        <div className="retention-preview" role="region" aria-label="Retention preview">
          <h3>Preview</h3>
          <RetentionTable preview={preview} />

          <label className="retention-confirm">
            Type <code>{confirmationText(preview)}</code> to confirm this deletion
            <input
              aria-label="Retention confirmation"
              value={confirmation}
              onChange={(event) => setConfirmation(event.target.value)}
              disabled={busy}
              spellCheck={false}
            />
          </label>
          <button
            type="button"
            className="primary"
            onClick={() => void apply()}
            disabled={busy || !canApply(preview, confirmation)}
          >
            Apply retention
          </button>
        </div>
      ) : null}

      {error ? (
        <p className="retention-error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
