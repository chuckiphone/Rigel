/**
 * Roadmap 124: the About surface.
 *
 * Every fact here is local: the version comes from the build, the commit from the stamp Rust
 * baked in at compile time, the licences from the same CycloneDX SBOM the release publishes, and
 * the diagnostics block is assembled from those facts. Nothing is uploaded and nothing about the
 * machine is added to the block. The dialog and the Settings page render the same panel.
 */

import { useEffect, useState } from "react";
import { api, PREVIEW_ABOUT, type AboutInfo } from "./api";
import { RigelMark } from "./mark";

/** The exact bytes the Copy button writes; the rendered `<pre>` shows this same string. */
export function aboutText(info: AboutInfo): string {
  return [
    `Rigel ${info.version}`,
    `commit ${info.commit}`,
    `licences ${info.components} SBOM components · ${info.licences.length} expressions`,
    `channel ${info.update_channel}`,
    `privacy ${info.privacy}`,
  ].join("\n");
}

function useAbout(enabled: boolean): AboutInfo {
  const [info, setInfo] = useState<AboutInfo>(PREVIEW_ABOUT);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    void api
      .aboutInfo()
      .then((next) => {
        if (alive) setInfo(next);
      })
      .catch(() => {
        if (alive) setInfo(PREVIEW_ABOUT);
      });
    return () => {
      alive = false;
    };
  }, [enabled]);
  return info;
}

export function AboutPanel({ info, onClose }: { info: AboutInfo; onClose?: () => void }) {
  const [copied, setCopied] = useState(false);
  const diagnostics = aboutText(info);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(diagnostics);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <>
      <div className="about-head">
        <RigelMark size={24} />
        <div>
          <h2>Rigel</h2>
          <p className="fine">A local-first agent workbench for Windows.</p>
        </div>
        <span className="pill">{info.version}</span>
      </div>
      <dl className="about-facts">
        <div>
          <dt>Version</dt>
          <dd>{info.version}</dd>
        </div>
        <div>
          <dt>Commit</dt>
          <dd className="mono">{info.commit}</dd>
        </div>
        <div>
          <dt>Licences</dt>
          <dd>
            MIT OR Apache-2.0 · {info.components} SBOM components, {info.licences.length}{" "}
            expressions in the published SBOM
          </dd>
        </div>
        <div>
          <dt>Update channel</dt>
          <dd>{info.update_channel}</dd>
        </div>
        <div>
          <dt>Privacy</dt>
          <dd>{info.privacy}</dd>
        </div>
        <div>
          <dt>Data</dt>
          <dd>Sessions live under .rigel/ in the project; the skill index under AppData.</dd>
        </div>
      </dl>
      <details className="about-licences">
        <summary>Dependency licence expressions ({info.licences.length})</summary>
        <ul>
          {info.licences.map((expression) => (
            <li key={expression} className="mono">
              {expression}
            </li>
          ))}
        </ul>
      </details>
      <section className="about-diagnostics-section">
        <h3>Diagnostics block</h3>
        <p className="fine">The exact text Copy writes. It stays local unless you paste it.</p>
        <pre className="about-diagnostics" aria-label="Diagnostics block">
          {diagnostics}
        </pre>
        <div className="settings-actions">
          <button type="button" className="chip" onClick={() => void copy()}>
            Copy diagnostics
          </button>
          {copied ? (
            <span className="fine" role="status">
              Copied.
            </span>
          ) : null}
          {onClose ? (
            <button type="button" className="primary" onClick={onClose}>
              Close
            </button>
          ) : null}
        </div>
      </section>
    </>
  );
}

/** The Settings page uses the same panel as the dialog, without the modal shell. */
export function AboutSettingsPage() {
  const info = useAbout(true);
  return (
    <div className="about-panel">
      <AboutPanel info={info} />
    </div>
  );
}

export default function AboutDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const info = useAbout(open);
  if (!open) return null;
  return (
    <div className="about-backdrop" role="presentation" onClick={onClose}>
      <section
        className="about-card"
        role="dialog"
        aria-modal="true"
        aria-label="About Rigel"
        onClick={(event) => event.stopPropagation()}
      >
        <AboutPanel info={info} onClose={onClose} />
      </section>
    </div>
  );
}
