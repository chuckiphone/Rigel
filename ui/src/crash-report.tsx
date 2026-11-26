/**
 * Roadmap 116: the crash reporter.
 *
 * Rigel is local-first: there is no upload endpoint and no telemetry. This surface shows the
 * already-redacted local crash tail with the build facts from About, and offers one exact payload
 * to copy into an issue. Sending is a consented action that copies those same bytes; the button is
 * disabled without consent and the screen says plainly that no request is made.
 */

import { useEffect, useState } from "react";
import { api, PREVIEW_ABOUT, PREVIEW_CRASH, type AboutInfo, type CrashReport } from "./api";

const SECRET =
  /\b(?:sk-(?:ant-)?[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[A-Za-z0-9_-]{20,})\b/g;

/** Defence in depth: the core redacts before writing, and the preview redacts again. */
export function redactCrashText(text: string): string {
  return text
    .replace(SECRET, "[redacted:secret]")
    .replace(/\bBearer\s+[^\s]+/g, "Bearer [redacted:secret]")
    .replace(/-----BEGIN [^-]+-----[\s\S]*?-----END [^-]+-----/g, "[redacted:key block]");
}

/** The one string used for both the preview and every copy/send action. */
export function crashPayload(report: CrashReport, info: AboutInfo, now: number): string {
  const tail = redactCrashText(
    report.text.trim() || "No crash record has been written on this machine.",
  );
  return [
    "Rigel crash report",
    `version ${info.version}`,
    `commit ${info.commit}`,
    `channel ${info.update_channel}`,
    `source ${report.path}`,
    `created ${new Date(now).toISOString()}`,
    "",
    tail,
  ].join("\n");
}

export function CrashReporter({
  report,
  info,
  now = Date.now(),
}: {
  report: CrashReport;
  info: AboutInfo;
  now?: number;
}) {
  const [consent, setConsent] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // The preview and every copy must be the same bytes, so the timestamp is set once on mount.
  const [createdAt] = useState(() => now);
  const payload = crashPayload(report, info, createdAt);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(payload);
      setNotice("Report copied. Nothing was uploaded.");
    } catch {
      setNotice("Could not copy the report from this window.");
    }
  };

  return (
    <section className="crash-report" aria-label="Crash reporter">
      <h3>Crash report</h3>
      <p className="fine">
        The newest local crash records, redacted before they were written and again before they
        are shown. The preview below is exactly the payload that Copy writes.
      </p>
      <label className="toggle">
        <input
          type="checkbox"
          checked={consent}
          onChange={(event) => setConsent(event.target.checked)}
        />
        I consent to sharing this redacted report
      </label>
      <pre className="crash-preview" aria-label="Redacted crash preview">
        {payload}
      </pre>
      <div className="settings-actions">
        <button type="button" className="chip" onClick={() => void copy()}>
          Copy report
        </button>
        <button
          type="button"
          className="primary"
          disabled={!consent}
          onClick={() => void copy()}
          title="Sharing is consent-only; no upload endpoint exists in this build"
        >
          Send report
        </button>
      </div>
      {notice ? (
        <p className="fine" role="status">
          {notice}
        </p>
      ) : null}
      <p className="fine">
        No upload endpoint exists in this local-first build. Send copies the preview so you can
        paste it into an issue; no request leaves the machine.
      </p>
    </section>
  );
}

/** Loads the local crash tail and the About facts once, when the surface opens. */
export function CrashReportSurface() {
  const [report, setReport] = useState<CrashReport>(PREVIEW_CRASH);
  const [info, setInfo] = useState<AboutInfo>(PREVIEW_ABOUT);
  useEffect(() => {
    let alive = true;
    void Promise.all([api.crashReport(), api.aboutInfo()])
      .then(([nextReport, nextInfo]) => {
        if (alive) {
          setReport(nextReport);
          setInfo(nextInfo);
        }
      })
      .catch(() => {
        if (alive) setReport(PREVIEW_CRASH);
      });
    return () => {
      alive = false;
    };
  }, []);
  return <CrashReporter report={report} info={info} />;
}
