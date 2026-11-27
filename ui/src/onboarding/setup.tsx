/**
 * Roadmap 108: first-run setup.
 *
 * Five steps, each backed by a real probe. A step can always be skipped, and the progress bar is
 * derived only from findings that have actually arrived — a pending check is not a completed one.
 */

import { useState, type ReactNode } from "react";

import { api, type SetupFinding, type SetupStep } from "../api";

export const SETUP_STEPS: { id: SetupStep; label: string; detail: string }[] = [
  { id: "os", label: "Operating system", detail: "The platform and architecture Rigel is running on." },
  { id: "shell", label: "Shell", detail: "The shell the terminal uses for project commands." },
  { id: "git", label: "Git", detail: "The git binary Rigel uses for checkpoints and patches." },
  { id: "providers", label: "Providers", detail: "Model doors and which credentials are stored." },
  { id: "harnesses", label: "Harnesses", detail: "External agents found on this machine." },
];

/** Count answered steps: a skip is an answer, an in-flight probe is not. */
export function answeredSteps(findings: readonly SetupFinding[]): number {
  return new Set(findings.map((finding) => finding.step)).size;
}

export function progressPercent(findings: readonly SetupFinding[]): number {
  return Math.round((answeredSteps(findings) / SETUP_STEPS.length) * 100);
}

/** The first step without a finding, or null when every step has been answered. */
export function nextStep(findings: readonly SetupFinding[]): SetupStep | null {
  return SETUP_STEPS.find((step) => !findings.some((finding) => finding.step === step.id))?.id ?? null;
}

export default function SetupWizard({
  probe = (step) => api.setupProbe(step),
  onDone,
}: {
  probe?: (step: SetupStep) => Promise<SetupFinding>;
  onDone?: () => void;
}): ReactNode {
  const [findings, setFindings] = useState<SetupFinding[]>([]);
  const [running, setRunning] = useState<SetupStep | null>(null);
  const [error, setError] = useState<string | null>(null);

  const record = (finding: SetupFinding) =>
    setFindings((current) => [
      ...current.filter((item) => item.step !== finding.step),
      finding,
    ]);

  const run = async (step: SetupStep) => {
    setRunning(step);
    setError(null);
    try {
      // Progress moves only here, after the probe has actually answered.
      record(await probe(step));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setRunning(null);
    }
  };

  const skip = (step: SetupStep) =>
    record({ step, detail: "skipped by the user", skipped: true });

  const answered = answeredSteps(findings);
  const percent = progressPercent(findings);

  return (
    <section className="setup-wizard" aria-label="First-run setup">
      <header className="setup-head">
        <div>
          <h3>First-run setup</h3>
          <p className="fine">Five checks. Skip any of them; nothing is promised until it is observed.</p>
        </div>
        <progress
          aria-label="Setup progress"
          value={answered}
          max={SETUP_STEPS.length}
        />
        <span className="fine">{percent}%</span>
      </header>

      {error ? (
        <p className="setup-error" role="alert">
          {error}
        </p>
      ) : null}

      <ol className="setup-steps" aria-label="Setup steps">
        {SETUP_STEPS.map((step, index) => {
          const finding = findings.find((item) => item.step === step.id);
          const busy = running === step.id;
          return (
            <li
              className={`setup-step${finding ? " answered" : ""}${busy ? " checking" : ""}`}
              key={step.id}
              data-step={step.id}
            >
              <span className="setup-number">{index + 1}</span>
              <div className="setup-copy">
                <strong>{step.label}</strong>
                <span className="fine">{step.detail}</span>
                {finding ? (
                  <span
                    className={`setup-finding${finding.skipped ? " skipped" : ""}`}
                    aria-label={`Finding for ${step.label}`}
                  >
                    {finding.detail}
                  </span>
                ) : null}
              </div>
              <button type="button" disabled={busy || Boolean(finding)} onClick={() => void run(step.id)}>
                {busy ? "Checking…" : finding && !finding.skipped ? "Check again" : "Run check"}
              </button>
              <button type="button" disabled={busy} onClick={() => skip(step.id)}>
                Skip
              </button>
            </li>
          );
        })}
      </ol>

      <footer className="setup-foot">
        <span className="fine">
          {answered === SETUP_STEPS.length
            ? "Every step has an answer."
            : `Next: ${nextStep(findings) ?? "done"}`}
        </span>
        {answered === SETUP_STEPS.length && onDone ? (
          <button type="button" onClick={onDone}>
            Finish setup
          </button>
        ) : null}
      </footer>
    </section>
  );
}
