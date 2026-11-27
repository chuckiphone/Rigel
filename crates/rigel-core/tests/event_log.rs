//! Session event log compaction (roadmap 189) integration tests.

use rigel_core::EventLog;

#[test]
fn million_chunks_stay_bounded_and_replay_works() {
    let mut log = EventLog::new();
    for _ in 0..1_000_000 {
        log.push_chunk("m1", "x");
    }
    assert_eq!(log.len(), 1);
    log.finalize("m1", "done");
    assert_eq!(log.len(), 1);
    let replay = log.replay();
    assert_eq!(replay, vec![("m1".to_string(), "done".to_string())]);
}
