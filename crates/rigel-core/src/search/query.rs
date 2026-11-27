//! FTS5 queries over local prompts, answers, commands, and patches.
//!
//! FTS terms are built with the same quoted-prefix helper used by the skill index. Metadata
//! filters are exact and bound, and results use a fixed timestamp/source/session/row ordering.

use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Maximum result count accepted by a history search.
pub const MAX_RESULTS: usize = 100;
const DEFAULT_RESULTS: usize = 25;
type SearchRow = (
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
);

/// Search text and optional exact project, agent, model, and millisecond timestamp filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub project: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    /// Inclusive lower bound, in Unix milliseconds.
    pub after_ms: Option<i64>,
    /// Inclusive upper bound, in Unix milliseconds.
    pub before_ms: Option<i64>,
    /// Maximum returned rows, capped at [`MAX_RESULTS`].
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            project: None,
            agent: None,
            model: None,
            after_ms: None,
            before_ms: None,
            limit: DEFAULT_RESULTS,
        }
    }
}

/// One FTS document matching a query. `snippet` contains match markers around the hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub source: String,
    pub record_id: i64,
    pub session_id: String,
    pub turn_id: String,
    pub agent_id: String,
    pub model: String,
    pub project_root: String,
    pub created_at: i64,
    pub kind: String,
    pub path: String,
    pub snippet: String,
}

/// Search all indexed project history, returning stable results for identical inputs.
pub async fn search(
    pool: &SqlitePool,
    query: &SearchQuery,
) -> Result<Vec<SearchResult>, sqlx::Error> {
    let Some(expression) = crate::skills::search::fts_query(&query.text) else {
        return Ok(Vec::new());
    };
    let limit = query.limit.clamp(1, MAX_RESULTS) as i64;
    let mut results = Vec::new();
    for (table, source, joins, turn_id, kind, path, date_column, agent_id) in [
        (
            "turns_fts",
            "turn",
            "JOIN turns t ON t.rowid = turns_fts.rowid
             JOIN agents a ON a.id = t.agent_id
             JOIN sessions s ON s.id = a.session_id",
            "t.id",
            "'prompt'",
            "''",
            "t.started_at",
            "a.id",
        ),
        (
            "items_fts",
            "item",
            "JOIN items i ON i.rowid = items_fts.rowid
             JOIN turns t ON t.id = i.turn_id
             JOIN agents a ON a.id = t.agent_id
             JOIN sessions s ON s.id = a.session_id",
            "t.id",
            "i.kind",
            "''",
            "i.created_at",
            "a.id",
        ),
        (
            "patches_fts",
            "patch",
            "JOIN patches p ON p.rowid = patches_fts.rowid
             JOIN sessions s ON s.id = p.session_id
             LEFT JOIN agents a ON a.id = p.agent_id",
            "''",
            "'patch'",
            "p.path",
            "p.created_at",
            "COALESCE(a.id, p.agent_id)",
        ),
    ] {
        let sql = format!(
            "SELECT '{source}' AS source, {table}.rowid AS record_id, s.id AS session_id,
                    {turn_id} AS turn_id, {agent_id} AS agent_id,
                    COALESCE(a.model, '') AS model, s.project_root,
                    {date_column} AS created_at, {kind} AS kind, {path} AS path,
                    snippet({table}, 8, '⟦', '⟧', '…', 16) AS snippet
             FROM {table} {joins} WHERE {table} MATCH "
        );
        let mut statement = QueryBuilder::<Sqlite>::new(sql);
        statement.push_bind(&expression);
        append_filters(&mut statement, query, date_column, agent_id);
        statement
            .push(" ORDER BY created_at DESC, session_id ASC, turn_id ASC, record_id ASC LIMIT ");
        statement.push_bind(limit);
        let rows: Vec<SearchRow> = statement.build_query_as().fetch_all(pool).await?;
        results.extend(rows.into_iter().map(
            |(
                source,
                record_id,
                session_id,
                turn_id,
                agent_id,
                model,
                project_root,
                created_at,
                kind,
                path,
                snippet,
            )| SearchResult {
                source,
                record_id,
                session_id,
                turn_id,
                agent_id,
                model,
                project_root,
                created_at,
                kind,
                path,
                snippet,
            },
        ));
    }
    results.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then(left.source.cmp(&right.source))
            .then(left.session_id.cmp(&right.session_id))
            .then(left.turn_id.cmp(&right.turn_id))
            .then(left.record_id.cmp(&right.record_id))
    });
    results.truncate(limit as usize);
    Ok(results)
}

fn append_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &SearchQuery,
    date_column: &str,
    agent_column: &str,
) {
    if let Some(project) = query.project.as_deref() {
        builder
            .push(" AND s.project_root = ")
            .push_bind(project.to_owned());
    }
    if let Some(agent) = query.agent.as_deref() {
        builder
            .push(" AND ")
            .push(agent_column)
            .push(" = ")
            .push_bind(agent.to_owned());
    }
    if let Some(model) = query.model.as_deref() {
        builder.push(" AND a.model = ").push_bind(model.to_owned());
    }
    if let Some(after_ms) = query.after_ms {
        builder
            .push(" AND ")
            .push(date_column)
            .push(" >= ")
            .push_bind(after_ms);
    }
    if let Some(before_ms) = query.before_ms {
        builder
            .push(" AND ")
            .push(date_column)
            .push(" <= ")
            .push_bind(before_ms);
    }
}
