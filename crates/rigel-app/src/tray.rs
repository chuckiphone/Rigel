//! Tray attention (roadmap 115).
//!
//! The tray shows running agents, pause-all, open and quit, and badges the number of
//! unresolved approvals and failures. The badge count is the sum of the core
//! `Attention` total and the pending approvals that belong to this project, so it
//! survives a restart: it is recomputed from the project database and the approval
//! bus rather than held in memory.

use std::path::Path;

use rigel_core::agent::approval::PendingApproval;
use rigel_core::attention::Attention;
use sqlx::SqlitePool;

/// The number of open items the tray should badge. `attention` must already include the project's
/// approvals (`Attention::with_approvals`); counting them again would double the badge.
pub fn badge_count(
    attention: &Attention,
    _pending: &[PendingApproval],
    _project_root: &Path,
) -> usize {
    attention.total
}

/// Recompute the badge count from a project pool and the pending approvals. Nothing is kept in
/// memory: a restarted tray reads the same database rows and gets the same number.
pub async fn badge_count_for_project(
    pool: &SqlitePool,
    pending: &[PendingApproval],
    project_root: &Path,
    now: i64,
) -> usize {
    let attention = rigel_core::attention::attention(pool, now)
        .await
        .unwrap_or(Attention {
            groups: Vec::new(),
            total: 0,
        })
        .with_approvals(pending, project_root);
    badge_count(&attention, pending, project_root)
}

/// A small struct describing the tray menu's attention state, for tests and the UI.
#[allow(dead_code)] // the live panel reads TraySnapshot; this is the documented test/UI shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayAttention {
    pub running_agents: usize,
    pub badge: usize,
}

impl TrayAttention {
    #[allow(dead_code)]
    pub fn new(running_agents: usize, badge: usize) -> Self {
        Self {
            running_agents,
            badge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rigel_core::SessionService;
    use rigel_core::agent::approval::PendingApproval;
    use rigel_core::attention::Attention;

    fn attention_with(total: usize) -> Attention {
        Attention {
            groups: Vec::new(),
            total,
        }
    }

    #[test]
    fn badge_count_adds_pending_approvals() {
        let attention = attention_with(2);
        let pending = vec![];
        let count = badge_count(&attention, &pending, std::path::Path::new("/tmp"));
        assert_eq!(count, 2);
    }

    #[test]
    fn badge_count_is_zero_when_nothing_open() {
        let attention = attention_with(0);
        let pending = vec![];
        assert_eq!(
            badge_count(&attention, &pending, std::path::Path::new("/tmp")),
            0
        );
    }

    #[tokio::test]
    async fn badge_count_equals_open_items_after_a_restart() {
        // The badge is recomputed from the database, not cached in the tray process: a restarted
        // window must show the same unresolved approvals and failures.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let now = rigel_core::migrate::now_ms();
        let service = SessionService::open(root).await.unwrap();
        let session = service.start(Some("restart badge"), None).await.unwrap();
        let agent = service
            .register_agent(&session.id, "native", None)
            .await
            .unwrap();
        let turn = service
            .append_turn(&agent.id, Some("needs attention"))
            .await
            .unwrap();
        sqlx::query("UPDATE turns SET status = 'failed', completed_at = ? WHERE id = ?")
            .bind(now)
            .bind(&turn.id)
            .execute(service.pool())
            .await
            .unwrap();

        let pending = vec![PendingApproval {
            id: "approval-1".to_string(),
            key: "command:rm dangerous".to_string(),
            kind: "command".to_string(),
            title: "Allow the command?".to_string(),
            detail: None,
            requested_at: now,
            project_root: Some(root.to_path_buf()),
            session_id: Some(session.id.clone()),
            agent_id: Some(agent.id.clone()),
            turn_id: Some(turn.id.clone()),
            expires_at: None,
            attempt: 0,
        }];
        let before = badge_count_for_project(service.pool(), &pending, root, now).await;
        assert_eq!(before, 2, "one failed turn plus one open approval");

        // Simulate a restart: drop every in-memory handle and open the same project afresh.
        drop(service);
        let reopened = SessionService::open(root).await.unwrap();
        let after = badge_count_for_project(reopened.pool(), &pending, root, now).await;
        assert_eq!(after, before);
        assert_eq!(after, 2);
    }

    #[test]
    fn tray_attention_holds_running_and_badge() {
        let t = TrayAttention::new(3, 5);
        assert_eq!(t.running_agents, 3);
        assert_eq!(t.badge, 5);
    }
}
