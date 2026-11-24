//! The skill shelf and its catalogue: search, indexing, installs and MCP/plugin rows.
//!
//! These are the `#[tauri::command]` functions only. Shared types and startup helpers stay
//! in the crate root, which owns the window lifecycle.

use crate::*;

/// Shelf search. Reads only the local index, so it works offline (§9, Shelf: offline state).
/// `offset` pages through the whole catalogue; a page is capped at 500 rows in the core.
#[tauri::command]
pub(crate) fn skills_search(
    query: String,
    limit: usize,
    offset: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .search_page(
                &query,
                limit,
                offset.unwrap_or(0),
                rigel_core::skills::search::SearchOrder::DEFAULT,
            )
            .await
            .map_err(message)
    })
}

/// How many rows the current query matches (the whole shelf when the query is empty).
/// The list header and the status bar show this instead of the size of one page.
#[tauri::command]
pub(crate) fn skills_count(query: String) -> Result<i64, String> {
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .count(&query)
            .await
            .map_err(message)
    })
}

/// Everything the detail pane shows: metadata, the static review, evidence and variants.
#[tauri::command]
pub(crate) fn skills_load(id: String) -> Result<SkillDetail, String> {
    tauri::async_runtime::block_on(async move {
        match SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .get(&id)
            .await
            .map_err(message)?
        {
            Some(detail) => Ok(detail),
            None => Err(message(SkillsError::NotIndexed(id))),
        }
    })
}

/// Source health for the status bar. Never makes a network call.
#[tauri::command]
pub(crate) fn skills_status() -> Result<Vec<rigel_core::SourceStatus>, String> {
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .status()
            .await
            .map_err(message)
    })
}

/// Walk a repository from the shelf's empty state. The only skill command that uses the
/// network, and the only one that can take seconds.
#[tauri::command]
pub(crate) fn skills_index(repo: String, force: bool) -> Result<rigel_core::IngestReport, String> {
    tauri::async_runtime::block_on(async move {
        let index = SkillIndex::open(&index_dir()).await.map_err(message)?;
        let policy = if force {
            SyncPolicy::forced()
        } else {
            SyncPolicy::normal()
        };
        index.index_repo(&repo, policy).await.map_err(message)
    })
}

/// Re-sync every skill source that is not fresh (`SkillIndex::refresh_stale`). The Shelf page's
/// Refresh button and the launch-time auto-update both call this; it makes network calls, so it
/// is never on the path of a search.
#[tauri::command]
pub(crate) fn skills_refresh() -> Result<rigel_core::skills::RefreshReport, String> {
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .refresh_stale()
            .await
            .map_err(message)
    })
}

/// Copy the exact indexed body into the harness stores. Nothing is fetched here.
#[tauri::command]
pub(crate) fn skills_install(id: String) -> Result<rigel_core::skills::InstallReport, String> {
    let roots = skill_roots()?;
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .install(&id, &roots)
            .await
            .map_err(message)
    })
}

/// Whether the selected row is already in either harness store.
#[tauri::command]
pub(crate) fn skills_installed(id: String) -> Result<bool, String> {
    let roots = skill_roots()?;
    tauri::async_runtime::block_on(async move {
        SkillIndex::open(&index_dir())
            .await
            .map_err(message)?
            .installed(&id, &roots)
            .await
            .map_err(message)
    })
}

/// The curated catalogue: offline, compiled in, optionally overridden by a local file.
#[tauri::command]
pub(crate) fn plugins_catalog(
    query: String,
    category: Option<String>,
    tier: Option<String>,
) -> Result<serde_json::Value, String> {
    let catalog = rigel_core::plugin::Catalog::load(&index_dir()).map_err(message)?;
    let entries = catalog.search(&query, category.as_deref(), tier.as_deref());
    Ok(serde_json::json!({
        "entries": entries,
        "categories": catalog.categories(),
        "updated": catalog.updated,
    }))
}

/// Every installed plugin, plus the directories that failed to load.
#[tauri::command]
pub(crate) fn plugins_list() -> Result<serde_json::Value, String> {
    let store = rigel_core::plugin::PluginStore::new(&index_dir());
    let (plugins, broken) = store.scan().map_err(message)?;
    let rows: Vec<_> = store.rows().map_err(message)?;
    Ok(serde_json::json!({
        "plugins": rows,
        "broken": broken.iter().map(|(name, error)| serde_json::json!({
            "name": name, "error": error
        })).collect::<Vec<_>>(),
        "enabled_tools": plugins.iter().filter(|plugin| plugin.enabled)
            .flat_map(|plugin| plugin.manifest.qualified_tools()).collect::<Vec<_>>(),
    }))
}

/// Install one catalogue entry. Skill entries seed the built-in body and copy it into the
/// harness stores; plugin entries install a bundled tree or a pinned repository.
#[tauri::command]
pub(crate) fn plugins_install(name: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::block_on(async move {
        let dir = index_dir();
        let catalog = rigel_core::plugin::Catalog::load(&dir).map_err(message)?;
        let entry = catalog
            .get(&name)
            .ok_or_else(|| format!("no catalogue entry named {name}"))?
            .clone();
        match entry.kind {
            rigel_core::plugin::CatalogKind::Skill => {
                let index = SkillIndex::open(&dir).await.map_err(message)?;
                let roots = skill_roots()?;
                let installed = index
                    .install_builtin_skills(&entry.skills, &roots)
                    .await
                    .map_err(message)?;
                Ok(serde_json::json!({
                    "ok": true, "kind": "skill", "name": entry.name,
                    "skills": installed
                }))
            }
            rigel_core::plugin::CatalogKind::Plugin => {
                let store = rigel_core::plugin::PluginStore::new(&dir);
                let report = if let Some(bundled) = &entry.bundled {
                    store.install_bundled(bundled, false)
                } else if let (Some(repo), Some(sha)) = (&entry.repo, &entry.sha) {
                    store.install_git(repo, sha, entry.subdir.as_deref(), false)
                } else {
                    return Err(format!(
                        "{} has no pinned commit; follow its install guide",
                        entry.name
                    ));
                }
                .map_err(message)?;
                Ok(serde_json::json!({ "ok": true, "kind": "plugin", "plugin": report }))
            }
            rigel_core::plugin::CatalogKind::External => Err(format!(
                "{} is a discovery listing; {}",
                entry.name,
                entry
                    .install
                    .clone()
                    .unwrap_or_else(|| "follow the project's install guide".to_string())
            )),
        }
    })
}

#[tauri::command]
pub(crate) fn plugins_remove(name: String) -> Result<bool, String> {
    rigel_core::plugin::PluginStore::new(&index_dir())
        .remove(&name)
        .map_err(message)
}

#[tauri::command]
pub(crate) fn plugins_set_enabled(
    name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let row = rigel_core::plugin::PluginStore::new(&index_dir())
        .set_enabled(&name, enabled)
        .map_err(message)?;
    Ok(serde_json::json!(row))
}

#[tauri::command]
pub(crate) fn mcp_catalog_search(
    query: String,
    limit: usize,
    offset: Option<usize>,
) -> Result<Vec<mcp_catalog::McpServer>, String> {
    mcp_catalog::search(&query, limit, offset.unwrap_or(0))
}

#[tauri::command]
pub(crate) fn mcp_catalog_count(query: String) -> Result<i64, String> {
    mcp_catalog::count(&query)
}

#[tauri::command]
pub(crate) fn mcp_install(
    project: String,
    id: String,
) -> Result<mcp_catalog::InstallReport, String> {
    mcp_catalog::install(&project, &id)
}

#[tauri::command]
pub(crate) fn mcp_installed(project: String, id: String) -> Result<bool, String> {
    mcp_catalog::installed(&project, &id)
}
