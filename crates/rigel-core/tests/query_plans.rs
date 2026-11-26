//! Query-plan contracts for Rigel's hottest project-database reads.

use std::path::Path;

use rigel_core::{StorageError, Store};
use sqlx::{SqliteConnection, SqlitePool};
use tempfile::TempDir;

struct HotQuery {
    name: &'static str,
    sql: &'static str,
    binds: &'static [&'static str],
    indexes: &'static [&'static str],
}

const QUERIES: [HotQuery; 10] = [
    HotQuery {
        name: "agents by session",
        sql: "SELECT id, session_id, harness, role, model, provider, effort, worktree, status, started_at, spend_cents, spend_micros, tokens_in, tokens_out, tool_overrides FROM agents WHERE session_id = ? ORDER BY started_at, rowid",
        binds: &["s-1"],
        indexes: &["agents_session"],
    },
    HotQuery {
        name: "items by turn",
        sql: "SELECT id, turn_id, seq, kind, status, body_json, created_at, raw_json FROM items WHERE turn_id = ? ORDER BY seq",
        binds: &["t-a-s-1"],
        indexes: &["sqlite_autoindex_items_2"],
    },
    HotQuery {
        name: "approval lookup by id",
        sql: "SELECT decision, scope, requested_at, decided_at FROM approvals WHERE id = ?",
        binds: &["approval-1"],
        indexes: &["sqlite_autoindex_approvals_1"],
    },
    HotQuery {
        name: "latest turn for agent",
        sql: "SELECT id, agent_id, seq, status, prompt, started_at, completed_at, usage_json, stop_reason, parent_turn_id, images_json FROM turns WHERE agent_id = ? ORDER BY started_at DESC, seq DESC LIMIT 1",
        binds: &["a-s-1"],
        indexes: &["turns_agent_latest"],
    },
    HotQuery {
        name: "cross-turn notes lookup",
        sql: "SELECT body_json FROM items WHERE kind = 'note' AND turn_id = (SELECT id FROM turns WHERE agent_id = ? AND id != ? ORDER BY started_at DESC, seq DESC LIMIT 1) ORDER BY seq",
        binds: &["a-s-1", "none"],
        indexes: &["turns_agent_latest", "items_turn_kind_seq"],
    },
    HotQuery {
        name: "stalled failed and budget-exceeded turns",
        sql: "SELECT t.id AS turn_id, t.seq, t.status AS turn_status, t.stop_reason, t.completed_at, t.agent_id, a.role, s.id AS session_id, s.title AS session_title, s.status AS session_status, s.archived_at AS session_archived_at FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id WHERE (t.status = 'failed' OR t.stop_reason = 'budget_exceeded') AND NOT EXISTS (SELECT 1 FROM turns later WHERE later.agent_id = t.agent_id AND later.seq > t.seq AND later.status = 'completed') ORDER BY t.completed_at DESC, t.seq DESC",
        binds: &[],
        indexes: &["turns_stalled"],
    },
    HotQuery {
        name: "spend report with recorded usage",
        sql: "SELECT t.agent_id, a.session_id, s.title, a.role, t.usage_json, t.started_at FROM turns t JOIN agents a ON a.id = t.agent_id JOIN sessions s ON s.id = a.session_id WHERE t.usage_json IS NOT NULL",
        binds: &[],
        indexes: &["turns_usage_present", "sessions_spend_title"],
    },
    HotQuery {
        name: "subagent nodes by session",
        sql: "SELECT parent_turn_id, node_id, role, depends_on_json, state, detail, updated_at FROM subagent_nodes WHERE session_id = ? ORDER BY parent_turn_id, updated_at, rowid",
        binds: &["s-1"],
        indexes: &["subagent_nodes_by_session"],
    },
    HotQuery {
        name: "recent bookmarks by session",
        sql: "SELECT id, session_id, kind, target_id, label, note, created_at FROM bookmarks WHERE session_id = ? ORDER BY created_at DESC, id DESC",
        binds: &["s-1"],
        indexes: &["bookmarks_session_recent"],
    },
    HotQuery {
        name: "session list ordering",
        sql: "SELECT id, project_root, title, status, budget_cents, spend_cents, spend_micros, interventions, tasks_done, created_at, ended_at, pinned_at, archived_at, title_source FROM sessions ORDER BY (pinned_at IS NULL), pinned_at DESC, created_at DESC, id DESC",
        binds: &[],
        indexes: &["sessions_list_order"],
    },
];

async fn open_project(root: &Path) -> Result<Store, StorageError> {
    Store::open_project(root).await
}

async fn seed_hot_query_data(pool: &SqlitePool) {
    sqlx::raw_sql(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 500)
         INSERT INTO sessions (id, project_root, title, status, created_at, pinned_at)
         SELECT 's-' || x, '/', 'session ' || x, 'active', x, CASE WHEN x % 3 = 0 THEN x END FROM n;
         WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 4)
         INSERT INTO agents (id, session_id, harness, status, started_at)
         SELECT 'a-' || s.id || '-' || n.x, s.id, 'native', 'running', n.x FROM sessions s CROSS JOIN n;
         WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 40)
         INSERT INTO turns (id, agent_id, seq, status, prompt, started_at, completed_at, usage_json, stop_reason, parent_turn_id)
         SELECT 't-' || a.id || '-' || n.x, a.id, n.x,
           CASE WHEN n.x % 37 = 0 THEN 'failed' ELSE 'completed' END, 'prompt', n.x, n.x,
           CASE WHEN n.x % 20 = 0 THEN '{}' END,
           CASE WHEN n.x % 41 = 0 THEN 'budget_exceeded' END,
           CASE WHEN n.x % 2 = 0 THEN 'parent-turn' END FROM agents a CROSS JOIN n;
         WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 8)
         INSERT INTO items (id, turn_id, seq, kind, status, body_json, raw_json, created_at)
         SELECT 'i-' || t.id || '-' || n.x, t.id, n.x, CASE WHEN n.x = 1 THEN 'note' ELSE 'text' END,
           'complete', '{}', '{}', n.x FROM turns t CROSS JOIN n;
         INSERT INTO approvals (id, agent_id, kind, request_json, requested_at)
         SELECT 'approval-' || id, id, 'command', '{}', started_at FROM agents;
         INSERT INTO subagent_nodes (session_id, parent_turn_id, node_id, role, depends_on_json, state, updated_at)
         SELECT a.session_id, t.id, t.id, 'worker', '[]', 'done', t.started_at FROM turns t JOIN agents a ON a.id = t.agent_id;
         INSERT INTO bookmarks (id, session_id, kind, target_id, label, created_at)
         SELECT 'b-' || t.id, a.session_id, 'item', t.id, 'turn', t.started_at FROM turns t JOIN agents a ON a.id = t.agent_id;
         ANALYZE;",
    )
    .execute(pool)
    .await
    .expect("query-plan fixture data seeds");
}

async fn explain(connection: &mut SqliteConnection, query: &HotQuery) -> Vec<String> {
    let explain_sql = format!("EXPLAIN QUERY PLAN {}", query.sql);
    let mut statement = sqlx::query_as::<_, (i64, i64, i64, String)>(&explain_sql);
    for value in query.binds {
        statement = statement.bind(*value);
    }
    statement
        .fetch_all(&mut *connection)
        .await
        .unwrap_or_else(|error| panic!("{}: query plan is available: {error}", query.name))
        .into_iter()
        .map(|(_, _, _, detail)| detail)
        .collect()
}

fn plan_error(query: &HotQuery, details: &[String]) -> Option<String> {
    let plan = details.join(" | ");
    for index in query.indexes {
        if !plan.contains(index) {
            return Some(format!(
                "{}: expected index {index}; plan was {plan}",
                query.name
            ));
        }
    }
    details
        .iter()
        .find(|detail| detail.starts_with("SCAN ") && !detail.contains(" USING "))
        .map(|scan| format!("{}: unindexed table scan: {scan}", query.name))
}

fn index_ddl(name: &str) -> &'static str {
    match name {
        "agents_session" => "CREATE INDEX agents_session ON agents(session_id, started_at)",
        "turns_agent_latest" => {
            "CREATE INDEX turns_agent_latest ON turns(agent_id, started_at DESC, seq DESC)"
        }
        "items_turn_kind_seq" => "CREATE INDEX items_turn_kind_seq ON items(turn_id, kind, seq)",
        "turns_stalled" => {
            "CREATE INDEX turns_stalled ON turns(completed_at DESC, seq DESC) WHERE status = 'failed' OR stop_reason = 'budget_exceeded'"
        }
        "turns_usage_present" => {
            "CREATE INDEX turns_usage_present ON turns(agent_id, started_at) WHERE usage_json IS NOT NULL"
        }
        "sessions_spend_title" => "CREATE INDEX sessions_spend_title ON sessions(id, title)",
        "subagent_nodes_by_session" => {
            "CREATE INDEX subagent_nodes_by_session ON subagent_nodes(session_id, parent_turn_id)"
        }
        "bookmarks_session_recent" => {
            "CREATE INDEX bookmarks_session_recent ON bookmarks(session_id, created_at DESC, id DESC)"
        }
        "sessions_list_order" => {
            "CREATE INDEX sessions_list_order ON sessions((pinned_at IS NULL), pinned_at DESC, created_at DESC, id DESC)"
        }
        _ => panic!("no drop/recreate assertion registered for index {name}"),
    }
}

#[tokio::test]
async fn ten_hot_queries_use_indexes_instead_of_unindexed_scans() {
    let temp = TempDir::new().unwrap();
    let store = open_project(temp.path()).await.unwrap();
    seed_hot_query_data(store.pool()).await;
    let mut connection = store.pool().acquire().await.unwrap();
    for query in &QUERIES {
        let details = explain(&mut connection, query).await;
        assert_eq!(
            plan_error(query, &details),
            None,
            "{} plan: {}",
            query.name,
            details.join(" | ")
        );
    }
}

#[tokio::test]
async fn removing_a_relevant_index_fails_with_the_query_name() {
    let temp = TempDir::new().unwrap();
    let store = open_project(temp.path()).await.unwrap();
    seed_hot_query_data(store.pool()).await;
    let mut connection = store.pool().acquire().await.unwrap();
    for query in &QUERIES {
        for index in query
            .indexes
            .iter()
            .copied()
            .filter(|index| !index.starts_with("sqlite_autoindex_"))
        {
            sqlx::query(&format!("DROP INDEX {index}"))
                .execute(&mut *connection)
                .await
                .unwrap_or_else(|error| panic!("{}: drop {index}: {error}", query.name));
            let details = explain(&mut connection, query).await;
            let error = plan_error(query, &details).unwrap_or_else(|| {
                panic!(
                    "{} still has an indexed plan after dropping {index}: {}",
                    query.name,
                    details.join(" | ")
                )
            });
            assert!(
                error.contains(query.name),
                "missing index failure must identify query `{}`: {error}",
                query.name
            );
            sqlx::raw_sql(index_ddl(index))
                .execute(&mut *connection)
                .await
                .unwrap_or_else(|error| panic!("{}: restore {index}: {error}", query.name));
        }
    }
}
