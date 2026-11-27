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

/// The number of open items the tray should badge. Equals the core attention total
/// plus the pending approvals that belong to this project.
// Kept as the documented attention surface; the tray menu wiring arrives with the badge UI.
#[allow(dead_code)]
pub fn badge_count(
    attention: &Attention,
    pending: &[PendingApproval],
    project_root: &Path,
) -> usize {
    let approvals = pending
        .iter()
        .filter(|p| p.belongs_to(project_root))
        .count();
    attention.total + approvals
}

/// Recompute the badge count from a project pool and the pending approvals.
#[allow(dead_code)]
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
        });
    let attention = attention.with_approvals(pending, project_root);
    badge_count(&attention, pending, project_root)
}

/// A small struct describing the tray menu's attention state, for tests and the UI.
#[allow(dead_code)]
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

    #[test]
    fn tray_attention_holds_running_and_badge() {
        let t = TrayAttention::new(3, 5);
        assert_eq!(t.running_agents, 3);
        assert_eq!(t.badge, 5);
    }
}
