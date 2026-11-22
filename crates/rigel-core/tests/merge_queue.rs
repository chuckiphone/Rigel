//! The merge queue is not just a standalone lock: the patch pipeline must be the writer that
//! takes it. This test holds a lane by hand, starts a real proposal for the same path, and
//! observes the proposal waiting on the named holder before it is allowed to write.

use std::time::Duration;

use rigel_core::PatchPipeline;
use rigel_core::agent::merge_queue;
use rigel_core::session::SessionService;

#[tokio::test]
async fn a_patch_proposal_waits_on_the_queue_holder_and_names_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let service = SessionService::open(&root).await.unwrap();
    let session = service.start(Some("queue"), None).await.unwrap();
    let agent = service
        .register_agent(&session.id, "native", Some("writer"))
        .await
        .unwrap();
    std::fs::write(root.join("answer.txt"), "one\n").unwrap();

    let pipeline = PatchPipeline::new(&root, service.pool().clone());
    let (_, base) = pipeline.read("answer.txt").unwrap();

    // Hold the lane as another writer would, then let the real pipeline try to propose.
    let hold = merge_queue::acquire(&root, "answer.txt", "queue-holder").await;
    let writer = tokio::spawn({
        let root = root.clone();
        let pool = service.pool().clone();
        let session_id = session.id.clone();
        let agent_id = agent.id.clone();
        async move {
            PatchPipeline::new(&root, pool)
                .propose(&session_id, &agent_id, "answer.txt", &base, "two\n")
                .await
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    let view = merge_queue::snapshot(&root);
    assert_eq!(view.entries.len(), 2, "holder and waiter are visible");
    assert_eq!(view.entries[0].writer_id, "queue-holder");
    assert_eq!(view.entries[0].state, merge_queue::MergeQueueState::Writing);
    assert_eq!(view.entries[1].writer_id, agent.id);
    assert_eq!(view.entries[1].state, merge_queue::MergeQueueState::Waiting);
    assert_eq!(view.entries[1].holder_id, "queue-holder");
    assert_eq!(view.entries[1].position, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("answer.txt")).unwrap(),
        "one\n",
        "the waiting proposal has not reached the file"
    );

    drop(hold);
    let patch = writer.await.unwrap().unwrap();
    assert_eq!(patch.status, "applied");
    assert_eq!(
        std::fs::read_to_string(root.join("answer.txt")).unwrap(),
        "two\n"
    );
    assert!(merge_queue::snapshot(&root).entries.is_empty());
}
