/**
 * The plugin marketplace.
 *
 * Two sections, one read of the core: the catalogue (tier, category, author, version, source,
 * capabilities, licence and the exact install command) and what is installed on this machine.
 * A skill entry installs the same reviewed body the agent can already search; a plugin entry
 * installs a tree into the plugin store and becomes callable through `plugin_call` once
 * enabled. External marketplace listings are discovery-only and name their upstream command.
 *
 * Installing is always a click on a named entry. Nothing is enabled on launch and nothing is
 * fetched until the click, which is the point of showing the capabilities first.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  api,
  type PluginCatalog,
  type PluginCatalogEntry,
  type PluginRow,
  type PluginsState,
} from "../api";

const TIERS = ["all", "official", "verified", "community"] as const;
type Tier = (typeof TIERS)[number];
/** A page of cards, not the whole directory: the catalogue is thousands of entries. */
const VISIBLE = 120;

export default function PluginsPage({ onNotice }: { onNotice: (notice: string) => void }) {
  const [catalog, setCatalog] = useState<PluginCatalog | null>(null);
  const [installed, setInstalled] = useState<PluginsState | null>(null);
  const [query, setQuery] = useState("");
  // The directory is thousands of rows; let a burst of typing settle before reading it.
  const [settledQuery, setSettledQuery] = useState("");
  const [category, setCategory] = useState<string | null>(null);
  const [tier, setTier] = useState<Tier>("all");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [visible, setVisible] = useState(VISIBLE);
  // A filter change can overlap an install's refresh. Only the newest request may write state,
  // or a slow answer for an old query replaces the catalogue the user is looking at now.
  const latest = useRef(0);

  useEffect(() => {
    const timer = window.setTimeout(() => setSettledQuery(query), 120);
    return () => window.clearTimeout(timer);
  }, [query]);

  const refresh = useCallback(async () => {
    const ticket = ++latest.current;
    setError(null);
    setLoading(true);
    try {
      const [nextCatalog, nextInstalled] = await Promise.all([
        api.plugins.catalog(settledQuery, category, tier === "all" ? null : tier),
        api.plugins.list(),
      ]);
      if (ticket !== latest.current) return;
      setCatalog(nextCatalog);
      setInstalled(nextInstalled);
    } catch (failure: unknown) {
      if (ticket !== latest.current) return;
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (ticket === latest.current) setLoading(false);
    }
  }, [settledQuery, category, tier]);

  useEffect(() => {
    setVisible(VISIBLE);
    void refresh();
  }, [refresh]);

  const installedNames = useMemo(
    () => new Set((installed?.plugins ?? []).map((plugin) => plugin.name)),
    [installed],
  );
  const brokenNames = useMemo(
    () => new Set((installed?.broken ?? []).map((row) => row.name)),
    [installed],
  );

  const install = async (entry: PluginCatalogEntry) => {
    setBusy(entry.name);
    setError(null);
    try {
      await api.plugins.install(entry.name);
      onNotice(`${entry.name} installed`);
      await refresh();
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (name: string) => {
    setBusy(name);
    setError(null);
    try {
      const removed = await api.plugins.remove(name);
      if (!removed) throw new Error(`${name} was not installed; nothing was removed`);
      onNotice(`${name} removed`);
      await refresh();
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  };

  const toggle = async (row: PluginRow) => {
    setBusy(row.name);
    setError(null);
    try {
      await api.plugins.setEnabled(row.name, !row.enabled);
      await refresh();
    } catch (failure: unknown) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      {error ? (
        <p className="notice" role="alert" data-testid="plugins-error">
          {error}
        </p>
      ) : null}

      <section className="settings-section">
        <div className="settings-section-head">
          <div>
            <h3>Catalogue</h3>
            <p className="fine">
              First-party entries install a reviewed skill body or a bundled plugin tree. The
              directory also carries external marketplace listings with their upstream install
              command; Rigel shows those for discovery and does not fetch them.
            </p>
          </div>
          <span className="badge">{catalog?.entries.length ?? 0} entries</span>
        </div>

        <div className="plugins-filters">
          <label className="search">
            <span aria-hidden>⌕</span>
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search names, authors, sources, capabilities…"
              aria-label="Search the plugin catalogue"
              spellCheck={false}
            />
          </label>
          <div className="plugins-chips" role="group" aria-label="Catalogue tier">
            {TIERS.map((value) => (
              <button
                key={value}
                type="button"
                className={`chip${tier === value ? " on" : ""}`}
                aria-pressed={tier === value}
                onClick={() => setTier(value)}
              >
                {value}
              </button>
            ))}
          </div>
          {query !== "" || category !== null || tier !== "all" ? (
            <button
              type="button"
              className="chip"
              data-testid="plugins-clear"
              onClick={() => {
                setQuery("");
                setCategory(null);
                setTier("all");
              }}
            >
              Clear filters
            </button>
          ) : null}
        </div>

        <div className="plugins-chips" role="group" aria-label="Catalogue category">
          <button
            type="button"
            className={`chip${category === null ? " on" : ""}`}
            aria-pressed={category === null}
            onClick={() => setCategory(null)}
          >
            every shelf
          </button>
          {(catalog?.categories ?? []).map((value) => (
            <button
              key={value}
              type="button"
              className={`chip${category === value ? " on" : ""}`}
              aria-pressed={category === value}
              onClick={() => setCategory(value)}
            >
              {value}
            </button>
          ))}
        </div>

        <div className="plugins-grid">
          {(catalog?.entries ?? []).slice(0, visible).map((entry) => (
            <article
              key={entry.name}
              className="plugins-card"
              data-testid="plugin-card"
              data-plugin={entry.name}
            >
              <header>
                <div>
                  <h4>{entry.title}</h4>
                  <span className="fine mono">{entry.name}</span>
                </div>
                <span className={`badge plugins-tier ${entry.tier}`}>{entry.tier}</span>
              </header>
              <p>{entry.description}</p>
              <div className="plugins-chips">
                <span className="chip">{entry.kind}</span>
                <span className="chip">{entry.category}</span>
                {entry.author ? <span className="chip">{entry.author}</span> : null}
                {entry.version ? (
                  <span className="chip mono">v{entry.version}</span>
                ) : null}
                {entry.source ? <span className="chip mono">{entry.source}</span> : null}
                {entry.license ? <span className="chip">{entry.license}</span> : null}
                {entry.capabilities.map((capability) => (
                  <span key={capability} className="chip warn">
                    {capability}
                  </span>
                ))}
              </div>
              {entry.note ? <p className="fine">{entry.note}</p> : null}
              {entry.install ? (
                <p className="fine">
                  {entry.kind === "external" ? "Install upstream" : "Tool"}:{" "}
                  <code className="plugins-install">{entry.install}</code>
                </p>
              ) : null}
              <footer>
                <button
                  type="button"
                  className="chip on"
                  data-testid="plugin-install"
                  disabled={
                    busy === entry.name ||
                    entry.kind === "external" ||
                    (entry.kind === "plugin" &&
                      (installedNames.has(entry.name) || brokenNames.has(entry.name)))
                  }
                  onClick={() => void install(entry)}
                >
                  {busy === entry.name
                    ? "Installing…"
                    : entry.kind === "skill"
                      ? "Install skill"
                      : entry.kind === "plugin"
                        ? brokenNames.has(entry.name)
                          ? "Broken — remove below"
                          : installedNames.has(entry.name)
                            ? "Installed"
                            : "Install plugin"
                        : "Discovery only"}
                </button>
                {entry.homepage ? (
                  <a className="fine" href={entry.homepage} target="_blank" rel="noreferrer">
                    Project page
                  </a>
                ) : null}
              </footer>
            </article>
          ))}
          {catalog !== null && catalog.entries.length === 0 ? (
            <p className="empty">No entry matches this filter.</p>
          ) : null}
          {catalog !== null && visible < catalog.entries.length ? (
            <button
              type="button"
              className="load-more"
              data-testid="plugins-more"
              onClick={() => setVisible((current) => current + VISIBLE)}
            >
              Load more · {Math.min(visible, catalog.entries.length).toLocaleString()} of{" "}
              {catalog.entries.length.toLocaleString()}
            </button>
          ) : null}
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-section-head">
          <div>
            <h3>Installed plugins</h3>
            <p className="fine">
              An enabled plugin offers its tools through <code>plugin_call</code>. A disabled
              one keeps its files and receipt but is not offered to the model. Removing deletes
              the installed tree.
            </p>
          </div>
          <div className="settings-actions">
            <button
              type="button"
              className="chip"
              data-testid="plugins-refresh"
              disabled={loading}
              onClick={() => void refresh()}
            >
              {loading ? "Refreshing…" : "Refresh"}
            </button>
            <span className="badge">{installed?.plugins.length ?? 0} installed</span>
          </div>
        </div>

        {loading && installed === null ? (
          <p className="empty">Reading the plugin store…</p>
        ) : null}

        {installed !== null && (installed?.plugins ?? []).length === 0 ? (
          <p className="empty">
            No plugins installed. Install <code>rigel-hello</code> from the catalogue to see the
            protocol and the capability broker working.
          </p>
        ) : null}

        <div className="plugins-installed">
          {(installed?.plugins ?? []).map((row) => (
            <div
              key={row.name}
              className="plugins-installed-row"
              data-testid="plugin-installed"
              data-plugin={row.name}
            >
              <div>
                <strong>{row.name}</strong>{" "}
                <span className="fine mono">{row.version}</span>
                <p className="fine">{row.description}</p>
                <p className="fine mono">{row.tools.join(" · ")}</p>
                <p className="fine">capabilities: {row.capabilities.join(" · ")}</p>
                <p className="fine" data-testid="plugin-receipt">
                  source: {row.source ?? "unknown"}
                  {row.license ? ` · ${row.license}` : ""}
                  {row.installed_at
                    ? ` · installed ${new Date(row.installed_at).toLocaleString()}`
                    : ""}
                </p>
              </div>
              <div className="plugins-actions">
                <button
                  type="button"
                  className="chip"
                  data-testid="plugin-toggle"
                  disabled={busy === row.name}
                  onClick={() => void toggle(row)}
                >
                  {row.enabled ? "Disable" : "Enable"}
                </button>
                <button
                  type="button"
                  className="chip"
                  data-testid="plugin-remove"
                  disabled={busy === row.name}
                  onClick={() => void remove(row.name)}
                >
                  Remove
                </button>
              </div>
            </div>
          ))}
        </div>

        {(installed?.broken ?? []).length > 0 ? (
          <div className="notice" role="alert" data-testid="plugins-broken">
            <strong>Broken installs</strong>
            {(installed?.broken ?? []).map((row) => (
              <div key={row.name} className="plugins-broken-row" data-broken={row.name}>
                <p className="fine">
                  {row.name}: {row.error}
                </p>
                <button
                  type="button"
                  className="chip"
                  data-testid="plugin-broken-remove"
                  disabled={busy === row.name}
                  onClick={() => void remove(row.name)}
                >
                  Remove
                </button>
              </div>
            ))}
          </div>
        ) : null}
      </section>
    </>
  );
}
