/**
 * The unified Shelf's plugin tab.
 *
 * Skills, MCPs and plugins share one tab now. This pane reads the same catalogue and store as
 * Settings → Plugins: the first-party entries can install a reviewed skill body or a bundled
 * plugin tree, while the larger marketplace directory is discovery-only and names the exact
 * upstream install command instead of pretending Rigel can run it.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  api,
  isTauri,
  type PluginCatalog,
  type PluginCatalogEntry,
  type PluginRow,
  type PluginsState,
} from "./api";
import { humanize, sourceLabel } from "./display";
import { ShelfTabs, type ShelfKind } from "./mcps";

const TIERS = ["all", "official", "verified", "community"] as const;
type Tier = (typeof TIERS)[number];
const KINDS = ["all", "skill", "plugin", "external"] as const;
type Kind = (typeof KINDS)[number];
const VISIBLE = 120;

function mark(kind: PluginCatalogEntry["kind"]): string {
  if (kind === "skill") return "SKL";
  if (kind === "plugin") return "PLG";
  return "EXT";
}

function canInstall(entry: PluginCatalogEntry): boolean {
  return entry.kind === "skill" || entry.kind === "plugin";
}

export default function PluginShelf({
  kind: shelfKind,
  onKind,
  onNotice,
}: {
  kind: ShelfKind;
  onKind: (kind: ShelfKind) => void;
  onNotice: (notice: string) => void;
}) {
  const [catalog, setCatalog] = useState<PluginCatalog | null>(null);
  const [installed, setInstalled] = useState<PluginsState | null>(null);
  const [query, setQuery] = useState("");
  // The directory is thousands of rows; let a burst of typing settle before reading it.
  const [settledQuery, setSettledQuery] = useState("");
  const [category, setCategory] = useState<string | null>(null);
  const [tier, setTier] = useState<Tier>("all");
  const [kindFilter, setKindFilter] = useState<Kind>("all");
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [visible, setVisible] = useState(VISIBLE);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [skillInstalled, setSkillInstalled] = useState<string | null>(null);
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
    } catch (failure) {
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

  const entries = useMemo(
    () =>
      (catalog?.entries ?? []).filter(
        (entry) => kindFilter === "all" || entry.kind === kindFilter,
      ),
    [catalog, kindFilter],
  );

  const installedNames = useMemo(
    () => new Set((installed?.plugins ?? []).map((plugin) => plugin.name)),
    [installed],
  );
  const brokenNames = useMemo(
    () => new Set((installed?.broken ?? []).map((row) => row.name)),
    [installed],
  );

  const selected = useMemo(
    () =>
      entries.find((entry) => entry.name === selectedName) ??
      entries[0] ??
      null,
    [entries, selectedName],
  );

  useEffect(() => {
    if (entries.length === 0) {
      if (selectedName !== null) setSelectedName(null);
      return;
    }
    if (!entries.some((entry) => entry.name === selectedName)) {
      setSelectedName(entries[0].name);
    }
  }, [entries, selectedName]);

  const shown = entries.slice(0, visible);

  const install = useCallback(
    async (entry: PluginCatalogEntry) => {
      setBusy(entry.name);
      setError(null);
      try {
        await api.plugins.install(entry.name);
        if (entry.kind === "skill") setSkillInstalled(entry.name);
        onNotice(`${entry.title} installed`);
        await refresh();
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        setBusy(null);
      }
    },
    [onNotice, refresh],
  );

  const remove = useCallback(
    async (name: string) => {
      setBusy(name);
      setError(null);
      try {
        const removed = await api.plugins.remove(name);
        if (!removed) throw new Error(`${name} was not installed; nothing was removed`);
        onNotice(`${name} removed`);
        await refresh();
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        setBusy(null);
      }
    },
    [onNotice, refresh],
  );

  const toggle = useCallback(
    async (row: PluginRow) => {
      setBusy(row.name);
      setError(null);
      try {
        await api.plugins.setEnabled(row.name, !row.enabled);
        await refresh();
      } catch (failure) {
        setError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  const installedRows = useMemo(
    () => (
      <div className="plugins-installed">
        {(installed?.plugins ?? []).map((row) => (
          <div
            key={row.name}
            className="plugins-installed-row"
            data-testid="shelf-plugin-installed"
            data-plugin={row.name}
          >
            <div>
              <strong>{row.name}</strong>{" "}
              <span className="fine mono">{row.version}</span>
              <p className="fine">{row.description}</p>
              <p className="fine mono">{row.tools.join(" · ")}</p>
              <p className="fine">capabilities: {row.capabilities.join(" · ")}</p>
              <p className="fine">
                Source: {sourceLabel(row.source)}
                {row.license ? ` · ${row.license}` : ""}
                {row.installed_at ? ` · ${new Date(row.installed_at).toLocaleString()}` : ""}
              </p>
            </div>
            <div className="plugins-actions">
              <button
                type="button"
                className="chip"
                data-testid="shelf-plugin-toggle"
                disabled={busy === row.name}
                onClick={() => void toggle(row)}
              >
                {row.enabled ? "Disable" : "Enable"}
              </button>
              <button
                type="button"
                className="chip"
                data-testid="shelf-plugin-remove"
                disabled={busy === row.name}
                onClick={() => void remove(row.name)}
              >
                Remove
              </button>
            </div>
          </div>
        ))}
        {(installed?.broken ?? []).map((row) => (
          <div
            key={row.name}
            className="plugins-installed-row"
            data-testid="shelf-plugin-broken"
            data-plugin={row.name}
          >
            <div>
              <strong>{row.name}</strong>
              <p className="fine">{row.error}</p>
            </div>
            <div className="plugins-actions">
              <button
                type="button"
                className="chip"
                disabled={busy === row.name}
                onClick={() => void remove(row.name)}
              >
                Remove
              </button>
            </div>
          </div>
        ))}
      </div>
    ),
    [busy, installed, remove, toggle],
  );

  const entryInstallLabel = (entry: PluginCatalogEntry): string => {
    if (entry.kind === "skill") {
      return skillInstalled === entry.name || busy === entry.name
        ? busy === entry.name
          ? "Installing…"
          : "✓ Installed"
        : "Install skill";
    }
    if (entry.kind === "plugin") {
      if (busy === entry.name) return "Installing…";
      if (brokenNames.has(entry.name)) return "Broken — remove installed copy";
      return installedNames.has(entry.name) ? "✓ Installed" : "Install plugin";
    }
    return "Discovery only";
  };

  return (
    <>
      <aside className="list-pane shelf-nav">
        <div className="list-head">
          <div className="list-title">
            <h1>Shelf</h1>
            <span>
              {catalog === null ? "…" : `${entries.length.toLocaleString()} plugins`}
            </span>
          </div>

          <ShelfTabs kind={shelfKind} onKind={onKind} />

          <label className="search">
            <span aria-hidden>⌕</span>
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search plugins, skills and sources…"
              aria-label="Search plugins"
              spellCheck={false}
            />
            <kbd>/</kbd>
          </label>

          <div className="plugin-filters">
            <select
              value={kindFilter}
              aria-label="Catalogue kind"
              onChange={(event) => setKindFilter(event.target.value as Kind)}
            >
              {KINDS.map((value) => (
                <option key={value} value={value}>
                  {value === "all" ? "Every kind" : humanize(value)}
                </option>
              ))}
            </select>
            <select
              value={tier}
              aria-label="Catalogue tier"
              onChange={(event) => setTier(event.target.value as Tier)}
            >
              {TIERS.map((value) => (
                <option key={value} value={value}>
                  {value === "all" ? "Every tier" : humanize(value)}
                </option>
              ))}
            </select>
            <select
              value={category ?? ""}
              aria-label="Catalogue category"
              onChange={(event) => setCategory(event.target.value || null)}
            >
              <option value="">Every shelf</option>
              {(catalog?.categories ?? []).map((value) => (
                <option key={value} value={value}>
                  {humanize(value)}
                </option>
              ))}
            </select>
          </div>
        </div>

        {error ? (
          <p className="banner muted" role="alert">
            {error}
          </p>
        ) : null}

        <div className="rows" role="listbox" aria-label="Plugins">
          {shown.map((entry) => (
            <button
              key={entry.name}
              role="option"
              aria-selected={selected?.name === entry.name}
              className={`row${selected?.name === entry.name ? " on" : ""}`}
              data-testid="shelf-plugin-row"
              data-plugin={entry.name}
              onClick={() => setSelectedName(entry.name)}
            >
              <span className="mcp-mark" aria-hidden>
                {mark(entry.kind)}
              </span>
              <span className="row-text">
                <span className="row-name">
                  {entry.title}
                  {entry.kind === "external" ? <span className="tag">external</span> : null}
                </span>
                <span className="row-desc">{entry.description}</span>
              </span>
              <span className="row-meta">
                {entry.source ? sourceLabel(entry.source) : humanize(entry.tier)}
              </span>
            </button>
          ))}

          {loading ? <p className="empty subtle">Reading the catalogue…</p> : null}
          {!loading && entries.length === 0 ? (
            <p className="empty">
              {error
                ? error
                : query
                  ? `No entry matches “${query}” with these filters.`
                  : "The catalogue is empty."}
            </p>
          ) : null}
          {!loading && visible < entries.length ? (
            <button
              className="load-more"
              type="button"
              onClick={() => setVisible((current) => current + VISIBLE)}
            >
              Load more · {visible.toLocaleString()} of {entries.length.toLocaleString()}
            </button>
          ) : null}
          {!loading && entries.length > 0 && visible >= entries.length ? (
            <p className="empty subtle">
              All {entries.length.toLocaleString()} matching entries shown.
            </p>
          ) : null}
        </div>
      </aside>

      <main id="main" tabIndex={-1} className="detail settings-main shelf-main">
        <header className="detail-head">
          <div>
            <p className="eyebrow">
              Plugin catalogue
              {selected?.source ? ` · ${sourceLabel(selected.source)}` : selected ? " · Rigel" : ""}
            </p>
            <h2>{selected?.title ?? "Plugins"}</h2>
            <p className="lede">
              {selected?.description ??
                "A searchable directory of plugins, skills and marketplace listings. First-party entries install into Rigel; external listings show their upstream install command."}
            </p>
          </div>

          {selected ? (
            <div className="detail-actions">
              <button
                className="primary"
                type="button"
                data-testid="shelf-plugin-install"
                onClick={() => void install(selected)}
                disabled={
                  !isTauri() ||
                  !canInstall(selected) ||
                  busy === selected.name ||
                  (selected.kind === "plugin" &&
                    (installedNames.has(selected.name) || brokenNames.has(selected.name))) ||
                  (selected.kind === "skill" && skillInstalled === selected.name)
                }
                title={
                  !isTauri()
                    ? "Plugin installs run in the desktop app"
                    : selected.kind === "external"
                      ? "This is an external listing. Run its install command in the harness that owns it."
                      : selected.kind === "plugin" && brokenNames.has(selected.name)
                        ? "The installed tree is broken; remove it below, then install again"
                        : selected.kind === "plugin" && installedNames.has(selected.name)
                          ? "Already installed"
                          : `Install ${selected.name}`
                }
              >
                {entryInstallLabel(selected)}
              </button>
            </div>
          ) : null}
        </header>

        {error ? (
          <p className="notice" role="alert">
            {error}
          </p>
        ) : null}

        {selected ? (
          <>
            <section className="facts" aria-label="Plugin facts">
              <div>
                <span className="label">Kind</span>
                <span className="value mono">{humanize(selected.kind)}</span>
              </div>
              <div>
                <span className="label">Tier</span>
                <span className="value mono">{humanize(selected.tier)}</span>
              </div>
              <div>
                <span className="label">Category</span>
                <span className="value">{humanize(selected.category)}</span>
              </div>
              <div>
                <span className="label">Author</span>
                <span className="value">{selected.author ?? "—"}</span>
              </div>
              <div>
                <span className="label">Version</span>
                <span className="value mono">{selected.version ?? "—"}</span>
              </div>
              <div>
                <span className="label">Source</span>
                <span className="value mono">
                  {selected.source
                    ? sourceLabel(selected.source)
                    : selected.bundled
                      ? `Bundled · ${selected.bundled}`
                      : "Rigel"}
                </span>
              </div>
              <div>
                <span className="label">Licence</span>
                <span className="value">{selected.license ?? "not declared"}</span>
              </div>
              <div>
                <span className="label">Package</span>
                <span className="value mono">{selected.name}</span>
              </div>
              <div>
                <span className="label">Homepage</span>
                <span className="value">
                  {selected.homepage ? (
                    <a href={selected.homepage} target="_blank" rel="noreferrer">
                      {selected.homepage.replace(/^https?:\/\//, "").replace(/\/$/, "")}
                    </a>
                  ) : (
                    "—"
                  )}
                </span>
              </div>
              {selected.repo ? (
                <div>
                  <span className="label">Repository</span>
                  <span className="value mono">{selected.repo}</span>
                </div>
              ) : null}
              {selected.sha ? (
                <div>
                  <span className="label">Pinned commit</span>
                  <span className="value mono">{selected.sha}</span>
                </div>
              ) : null}
              {selected.subdir ? (
                <div>
                  <span className="label">Subdirectory</span>
                  <span className="value mono">{selected.subdir}</span>
                </div>
              ) : null}
            </section>

            {selected.capabilities.length > 0 || selected.skills.length > 0 ? (
              <section className="settings-section plugin-shelf-section">
                <h3>Declared surface</h3>
                <div className="plugins-chips">
                  {selected.capabilities.map((capability) => (
                    <span key={capability} className="chip warn">
                      {capability}
                    </span>
                  ))}
                  {selected.skills.map((skill) => (
                    <span key={skill} className="chip">
                      skill: {skill}
                    </span>
                  ))}
                </div>
              </section>
            ) : null}

            {selected.note ? (
              <section className="settings-section plugin-shelf-note">
                <strong>{selected.kind === "external" ? "Upstream listing" : "Review note"}</strong>
                <p className="fine">{selected.note}</p>
              </section>
            ) : null}

            {selected.install ? (
              <section className="settings-section plugin-shelf-section">
                <h3>
                  {selected.kind === "external" ? "Install in its harness" : "Install command"}
                </h3>
                <p className="fine">
                  {selected.kind === "external"
                    ? "Rigel does not copy this upstream tree into its own plugin store. Run the command in the harness that owns the plugin."
                    : "The same command the CLI prints. Installing from this pane uses the reviewed catalogue path."}
                </p>
                <code className="plugin-shelf-command">{selected.install}</code>
              </section>
            ) : null}

            <section className="settings-section plugin-shelf-section">
              <div className="settings-section-head">
                <div>
                  <h3>Installed plugins</h3>
                  <p className="fine">
                    Enabled tools are offered through <code>plugin_call</code>. Disabling keeps
                    the tree and receipt; removing deletes the installed tree.
                  </p>
                </div>
                <button
                  type="button"
                  className="chip"
                  data-testid="shelf-plugins-refresh"
                  disabled={loading}
                  onClick={() => void refresh()}
                >
                  {loading ? "Refreshing…" : "Refresh"}
                </button>
              </div>
              {installed !== null &&
              (installed.plugins ?? []).length === 0 &&
              (installed.broken ?? []).length === 0 ? (
                <p className="empty">
                  Nothing installed yet. Install <code>rigel-hello</code> to see the protocol and
                  the capability broker working, or <code>superpowers</code> for the cross-agent
                  workflow.
                </p>
              ) : null}
              {installedRows}
            </section>
          </>
        ) : (
          <section className="settings-section onboarding">
            <h3>{loading ? "Reading the catalogue" : "No entry selected"}</h3>
            <p>
              {loading
                ? "The embedded catalogue is loading."
                : "Search the left pane, or clear the query to browse every shelf."}
            </p>
          </section>
        )}
      </main>
    </>
  );
}
