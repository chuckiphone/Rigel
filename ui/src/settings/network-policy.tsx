/**
 * Roadmap 61: the project network policy editor.
 *
 * The kill switch, ordered allow/deny rules and the disabled-session list all round-trip through
 * the same `NetworkPolicy::save` the CLI uses. The check button asks the core for the same
 * decision `fetch_url` makes; the browser preview has no Rust process, so it says so rather than
 * inventing a second matcher in the window.
 */

import { useCallback, useEffect, useState } from "react";

import {
  api,
  type NetworkAction,
  type NetworkCheck,
  type NetworkDefault,
  type NetworkLimits,
  type NetworkPolicy,
  type NetworkRule,
} from "../api";

const EMPTY_POLICY: NetworkPolicy = {
  enabled: true,
  default: "ask",
  rules: [],
  limits: {
    window_seconds: 60,
    per_origin_requests: 30,
    per_agent_requests: 120,
    max_response_bytes: 262_144,
    pin_dns: true,
    allow_private_addresses: false,
  },
  disabled_sessions: [],
};

export default function NetworkPolicyEditor({
  project,
  session,
  onNotice,
}: {
  project: string | null;
  session: string | null;
  onNotice: (notice: string) => void;
}) {
  const [policy, setPolicy] = useState<NetworkPolicy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [checkUrl, setCheckUrl] = useState("https://");
  const [check, setCheck] = useState<NetworkCheck | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [sessionInput, setSessionInput] = useState("");

  useEffect(() => {
    let live = true;
    setPolicy(null);
    setError(null);
    setCheck(null);
    setCheckError(null);
    setDirty(false);
    if (!project) return;
    api
      .networkGet(project)
      .then((next) => {
        if (!live) return;
        setPolicy(next);
      })
      .catch((failure: unknown) => {
        if (!live) return;
        setError(failure instanceof Error ? failure.message : String(failure));
        setPolicy(EMPTY_POLICY);
      });
    return () => {
      live = false;
    };
  }, [project]);

  const edit = useCallback((patch: Partial<NetworkPolicy>) => {
    setPolicy((current) => (current ? { ...current, ...patch } : current));
    setDirty(true);
  }, []);

  const editLimits = useCallback((patch: Partial<NetworkLimits>) => {
    setPolicy((current) =>
      current ? { ...current, limits: { ...current.limits, ...patch } } : current,
    );
    setDirty(true);
  }, []);

  const editRule = useCallback((index: number, patch: Partial<NetworkRule>) => {
    setPolicy((current) => {
      if (!current) return current;
      const rules = current.rules.map((rule, position) =>
        position === index ? { ...rule, ...patch } : rule,
      );
      return { ...current, rules };
    });
    setDirty(true);
  }, []);

  const save = async () => {
    if (!project || !policy || busy) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await api.networkSet(project, policy);
      setPolicy(saved);
      setDirty(false);
      onNotice(
        `Network policy saved: ${saved.rules.length} rule${
          saved.rules.length === 1 ? "" : "s"
        }, ${saved.enabled ? "kill switch off" : "kill switch on"}.`,
      );
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  const explain = async () => {
    if (!project || !checkUrl.trim()) return;
    setCheckError(null);
    setCheck(null);
    try {
      const result = await api.networkCheck(project, checkUrl.trim(), session);
      if (!result) {
        setCheckError(
          "The browser preview has no core policy check. Open the desktop app to explain a URL.",
        );
        return;
      }
      setCheck(result);
    } catch (failure) {
      setCheckError(failure instanceof Error ? failure.message : String(failure));
    }
  };

  if (!project) {
    return <p className="fine">Open a project to edit its network policy.</p>;
  }

  return (
    <>
      <section className="settings-section">
        <div className="settings-head">
          <h3>Project network policy</h3>
          <span className={`pill ${policy?.enabled === false ? "cancelled" : "completed"}`}>
            {policy ? (policy.enabled ? "network on" : "network off") : "…"}
          </span>
        </div>
        <p className="fine">
          The core checks this before any <code>fetch_url</code>, <code>browser_open</code> or Exa
          request. An <strong>allow</strong> rule only means the origin may ask; the once/session/
          always approval still happens. Rules are ordered and the first match wins.
        </p>
        {error ? <p className="fine danger-text">{error}</p> : null}
        {!policy ? (
          <p className="fine">Reading the project policy…</p>
        ) : (
          <>
            <div className="settings-field-grid">
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={policy.enabled}
                  onChange={(event) => edit({ enabled: event.target.checked })}
                  aria-label="Project network kill switch"
                />
                network enabled
              </label>
              <label>
                <span>Default when no rule matches</span>
                <select
                  value={policy.default}
                  aria-label="Default network decision"
                  onChange={(event) =>
                    edit({ default: event.target.value as NetworkDefault })
                  }
                >
                  <option value="ask">ask per origin</option>
                  <option value="deny">deny</option>
                </select>
              </label>
            </div>

            <div className="network-limits">
              <div className="settings-head">
                <h3>Egress limits</h3>
                <span className="pill">per process</span>
              </div>
              <p className="fine">
                A fixed window counts requests per origin and per agent. An exhausted window
                refuses the next request before it is sent; the counter starts again after the
                window. A zero request limit means unbounded.
              </p>
              <div className="settings-field-grid">
                <label>
                  <span>Window (seconds)</span>
                  <input
                    type="number"
                    min={1}
                    aria-label="Egress window seconds"
                    value={policy.limits.window_seconds}
                    onChange={(event) =>
                      editLimits({ window_seconds: Number(event.target.value) })
                    }
                  />
                </label>
                <label>
                  <span>Requests per origin</span>
                  <input
                    type="number"
                    min={0}
                    aria-label="Requests per origin"
                    value={policy.limits.per_origin_requests}
                    onChange={(event) =>
                      editLimits({ per_origin_requests: Number(event.target.value) })
                    }
                  />
                </label>
                <label>
                  <span>Requests per agent</span>
                  <input
                    type="number"
                    min={0}
                    aria-label="Requests per agent"
                    value={policy.limits.per_agent_requests}
                    onChange={(event) =>
                      editLimits({ per_agent_requests: Number(event.target.value) })
                    }
                  />
                </label>
                <label>
                  <span>Response cap (bytes)</span>
                  <input
                    type="number"
                    min={1}
                    aria-label="Response byte cap"
                    value={policy.limits.max_response_bytes}
                    onChange={(event) =>
                      editLimits({ max_response_bytes: Number(event.target.value) })
                    }
                  />
                </label>
              </div>
              <div className="settings-field-grid">
                <label className="toggle">
                  <input
                    type="checkbox"
                    checked={policy.limits.pin_dns}
                    onChange={(event) => editLimits({ pin_dns: event.target.checked })}
                    aria-label="Pin DNS resolution"
                  />
                  pin DNS (resolve once)
                </label>
                <label className="toggle">
                  <input
                    type="checkbox"
                    checked={policy.limits.allow_private_addresses}
                    onChange={(event) =>
                      editLimits({ allow_private_addresses: event.target.checked })
                    }
                    aria-label="Allow private network addresses"
                  />
                  allow loopback and private addresses
                </label>
              </div>
              <p className="fine">
                Pinning resolves the host, validates the answer, then connects only to that
                address. A second answer that differs is a rebind refusal. Private, loopback,
                link-local and metadata addresses are refused unless explicitly allowed here.
              </p>
            </div>

            {policy.allowlist?.length ? (
              <p className="fine">
                This file still carries the legacy allowlist. Saving converts each entry to an
                allow rule and sets the default to deny.
              </p>
            ) : null}

            <div className="network-rules">
              <div className="settings-head">
                <h3>Rules</h3>
                <button
                  type="button"
                  className="chip"
                  onClick={() =>
                    edit({
                      rules: [...policy.rules, { action: "deny", pattern: "", note: null }],
                    })
                  }
                  disabled={busy}
                >
                  + rule
                </button>
              </div>
              {policy.rules.length === 0 ? (
                <p className="fine">
                  No rules. The default above decides every origin; the kill switch still wins.
                </p>
              ) : null}
              {policy.rules.map((rule, index) => (
                <div className="network-rule" key={index}>
                  <select
                    value={rule.action}
                    aria-label={`Rule ${index + 1} action`}
                    onChange={(event) =>
                      editRule(index, { action: event.target.value as NetworkAction })
                    }
                  >
                    <option value="allow">allow</option>
                    <option value="deny">deny</option>
                  </select>
                  <input
                    value={rule.pattern}
                    aria-label={`Rule ${index + 1} pattern`}
                    placeholder="example.com, *.example.com or https://api.example.com"
                    spellCheck={false}
                    onChange={(event) => editRule(index, { pattern: event.target.value })}
                  />
                  <input
                    value={rule.note ?? ""}
                    aria-label={`Rule ${index + 1} note`}
                    placeholder="why (optional)"
                    onChange={(event) =>
                      editRule(index, { note: event.target.value || null })
                    }
                  />
                  <button
                    type="button"
                    className="chip danger"
                    aria-label={`Remove rule ${index + 1}`}
                    onClick={() =>
                      edit({
                        rules: policy.rules.filter((_, position) => position !== index),
                      })
                    }
                    disabled={busy}
                  >
                    remove
                  </button>
                </div>
              ))}
            </div>

            <div className="network-sessions">
              <div className="settings-head">
                <h3>Disabled sessions</h3>
              </div>
              <p className="fine">
                A disabled session keeps the project policy but may not open a request. The
                project switch still applies to every session.
              </p>
              <div className="cap-form">
                <input
                  value={sessionInput}
                  aria-label="Session id to disable"
                  placeholder="session id"
                  onChange={(event) => setSessionInput(event.target.value)}
                />
                <button
                  type="button"
                  className="chip"
                  disabled={busy || !sessionInput.trim()}
                  onClick={() => {
                    const id = sessionInput.trim();
                    if (!policy.disabled_sessions.includes(id)) {
                      edit({ disabled_sessions: [...policy.disabled_sessions, id] });
                    }
                    setSessionInput("");
                  }}
                >
                  disable
                </button>
                {session && !policy.disabled_sessions.includes(session) ? (
                  <button
                    type="button"
                    className="chip"
                    disabled={busy}
                    onClick={() =>
                      edit({ disabled_sessions: [...policy.disabled_sessions, session] })
                    }
                  >
                    disable this session
                  </button>
                ) : null}
              </div>
              <div className="network-session-list">
                {policy.disabled_sessions.length === 0 ? (
                  <span className="fine">none</span>
                ) : (
                  policy.disabled_sessions.map((id) => (
                    <button
                      type="button"
                      className="chip"
                      key={id}
                      title="Enable this session again"
                      onClick={() =>
                        edit({
                          disabled_sessions: policy.disabled_sessions.filter(
                            (candidate) => candidate !== id,
                          ),
                        })
                      }
                      disabled={busy}
                    >
                      {id} ×
                    </button>
                  ))
                )}
              </div>
            </div>

            <div className="cap-form">
              <button className="primary" type="button" onClick={() => void save()} disabled={busy}>
                {busy ? "Saving…" : "Save network policy"}
              </button>
              {dirty ? <span className="fine">unsaved changes</span> : null}
            </div>
          </>
        )}
      </section>

      <section className="settings-section">
        <div className="settings-head">
          <h3>Explain a URL</h3>
          <span className="pill">no request sent</span>
        </div>
        <p className="fine">
          Runs the same decision the egress tools run, including this session's kill switch. A
          denied result names the rule or default that refused it.
        </p>
        <div className="cap-form">
          <input
            value={checkUrl}
            aria-label="URL to explain"
            spellCheck={false}
            onChange={(event) => setCheckUrl(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void explain();
            }}
          />
          <button className="chip" type="button" onClick={() => void explain()}>
            explain
          </button>
        </div>
        {checkError ? <p className="fine danger-text">{checkError}</p> : null}
        <p className="fine">
          Limits: {policy?.limits.per_origin_requests ?? 0} request(s)/origin ·{" "}
          {policy?.limits.per_agent_requests ?? 0} request(s)/agent ·{" "}
          {policy?.limits.window_seconds ?? 0}s window ·{" "}
          {policy?.limits.max_response_bytes ?? 0} byte cap ·{" "}
          {policy?.limits.pin_dns ? "DNS pinned" : "DNS not pinned"}
        </p>
        {check ? (
          <div className="network-check" data-allowed={check.decision.allowed}>
            <p className="fine">
              <code>{check.origin}</code> · {check.decision.allowed ? "allowed" : "denied"} ·{" "}
              {check.decision.source}
            </p>
            <p className={check.decision.allowed ? "fine" : "fine danger-text"}>
              {check.decision.allowed ? "" : `${check.decision.code}: `}
              {check.decision.reason}
            </p>
          </div>
        ) : null}
        <p className="fine">
          Stored in <code>.rigel/network.json</code> in the project, next to the session database.
          A rule this build cannot enforce is refused on save; it is never written and ignored.
        </p>
      </section>
    </>
  );
}
