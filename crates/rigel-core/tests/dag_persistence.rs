//! Roadmap 42/58: the persisted subagent graph must never go backwards. Every child thread saves
//! the shared graph from its own snapshot, so a prerequisite's stale snapshot can arrive after a
//! dependent was marked blocked. The Pending-vs-Blocked failure in `native.rs` was exactly that
//! overwrite; these tests pin the merge rule at the store.

use rigel_core::SessionService;
use rigel_core::agent::dag::{DagNode, NodeState};
use tempfile::TempDir;

fn node(id: &str, depends_on: &[&str], state: NodeState, detail: Option<&str>) -> DagNode {
    DagNode {
        id: id.to_string(),
        role: id.to_string(),
        depends_on: depends_on.iter().map(|value| value.to_string()).collect(),
        state,
        detail: detail.map(str::to_string),
        depth: 1,
        budget_micros: None,
        spent_micros: 0,
        remaining_micros: None,
    }
}

#[tokio::test]
async fn a_stale_snapshot_cannot_reopen_a_blocked_dependent_or_a_running_node() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("dag persistence"), None).await.unwrap();
    let parent = "parent-turn";

    // The graph as it was created: both children pending.
    service
        .save_dag(
            &session.id,
            parent,
            &[
                node("first", &[], NodeState::Pending, None),
                node("second", &["first"], NodeState::Pending, None),
            ],
        )
        .await
        .unwrap();

    // The live graph after the failure: first failed, second blocked with the reason.
    service
        .save_dag(
            &session.id,
            parent,
            &[
                node(
                    "first",
                    &[],
                    NodeState::Failed,
                    Some("the review found a bug"),
                ),
                node(
                    "second",
                    &["first"],
                    NodeState::Blocked,
                    Some("dependency first ended failed"),
                ),
            ],
        )
        .await
        .unwrap();

    // A slower thread still holds the pre-failure snapshot. Replacing the graph would write
    // `pending` over `blocked`; the merge rule keeps the terminal state and its detail.
    service
        .save_dag(
            &session.id,
            parent,
            &[
                node("first", &[], NodeState::Running, None),
                node("second", &["first"], NodeState::Pending, None),
            ],
        )
        .await
        .unwrap();

    let graphs = service.dag(&session.id).await.unwrap();
    let nodes = &graphs[0].nodes;
    let first = nodes.iter().find(|node| node.id == "first").unwrap();
    let second = nodes.iter().find(|node| node.id == "second").unwrap();
    assert_eq!(first.state, NodeState::Failed);
    assert_eq!(first.detail.as_deref(), Some("the review found a bug"));
    assert_eq!(second.state, NodeState::Blocked);
    assert!(
        second
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("first"),
        "the block reason survives the stale write"
    );
}

#[tokio::test]
async fn a_running_node_is_not_returned_to_pending_by_a_stale_snapshot() {
    let tmp = TempDir::new().unwrap();
    let service = SessionService::open(tmp.path()).await.unwrap();
    let session = service.start(Some("dag running"), None).await.unwrap();
    let parent = "parent-turn";

    service
        .save_dag(
            &session.id,
            parent,
            &[node("child", &[], NodeState::Running, None)],
        )
        .await
        .unwrap();
    service
        .save_dag(
            &session.id,
            parent,
            &[node("child", &[], NodeState::Pending, None)],
        )
        .await
        .unwrap();

    let graphs = service.dag(&session.id).await.unwrap();
    assert_eq!(graphs[0].nodes[0].state, NodeState::Running);
}
