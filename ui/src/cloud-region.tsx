import React, { useState } from "react";

export interface CloudRegionStatus {
  configured: boolean;
  project_id: string | null;
  data_region: string | null;
}

export function regionMoveConfirmation(project: string, current: string, target: string): string {
  return `MOVE ${project} FROM ${current} TO ${target}`;
}

export function regionMoveConfirmed(
  project: string,
  current: string | null,
  target: string,
  confirmation: string,
): boolean {
  return !current || current === target || confirmation === regionMoveConfirmation(project, current, target);
}

export function DataLocationPanel({
  status,
  busy = false,
  onPin,
}: {
  status: CloudRegionStatus | null;
  busy?: boolean;
  onPin: (region: string, confirmation: string | null) => void;
}) {
  const [regionInput, setRegionInput] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const current = status?.data_region ?? null;
  const target = regionInput.trim().toLowerCase();
  const moving = Boolean(current && target && current !== target);
  const expected = moving && status?.project_id
    ? regionMoveConfirmation(status.project_id, current!, target)
    : null;
  const canSave = Boolean(
    status?.configured &&
      target &&
      !busy &&
      regionMoveConfirmed(status.project_id ?? "", current, target, confirmation),
  );

  return (
    <React.Fragment>
    <section className="settings-section" aria-label="Data location">
      <div className="settings-head">
        <h3>Data location</h3>
        <span className={`pill ${current ? "ok" : "cancelled"}`}>
          {current ?? "not pinned"}
        </span>
      </div>
      <p className="fine">
        {current
          ? `The stored Firebase Firestore region for ${status?.project_id ?? "this project"} is ${current}.`
          : "No Firebase region is pinned in cloud.json. Pin the location selected for this project's Firestore database."}
      </p>
      <p className="fine">
        A region pin records the configured location. Firebase does not move an existing database
        when this value changes; plan and complete any physical migration in Firebase first.
      </p>
      <label className="settings-field">
        <span>Firestore region</span>
        <input
          aria-label="Firestore region"
          value={regionInput}
          placeholder={current ?? "us-central1"}
          spellCheck={false}
          autoComplete="off"
          disabled={!status?.configured || busy}
          onChange={(event) => setRegionInput(event.target.value)}
        />
      </label>
      {expected ? (
        <>
          <p className="fine" role="alert">
            Changing the stored region requires explicit confirmation. Type <code>{expected}</code>.
          </p>
          <label className="settings-field">
            <span>Confirm region change</span>
            <input
              aria-label="Confirm region change"
              value={confirmation}
              autoComplete="off"
              disabled={busy}
              onChange={(event) => setConfirmation(event.target.value)}
            />
          </label>
        </>
      ) : null}
      <div className="settings-actions">
        <button
          className="primary"
          disabled={!canSave}
          onClick={() => {
            if (canSave) onPin(target, expected ? confirmation : null);
          }}
        >
          {current ? "Update region pin" : "Pin region"}
        </button>
      </div>
    </section>
    </React.Fragment>
  );
}
