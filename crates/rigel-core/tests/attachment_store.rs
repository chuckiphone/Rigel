//! Attachment store quota (roadmap 101) integration tests.

use rigel_core::AttachmentStore;
use std::collections::HashSet;

#[test]
fn quota_type_dedup_purge() {
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path(), 1024, 10, vec!["text/plain".to_string()]);
    let a = store.put("p", b"hello", "text/plain").unwrap();
    let b = store.put("p", b"hello", "text/plain").unwrap();
    assert_eq!(a.hash, b.hash);

    // type validation
    assert!(store.put("p", b"x", "image/png").is_err());

    // purge protects referenced
    let c = store.put("p", b"world", "text/plain").unwrap();
    let removed = store.purge("p", &HashSet::from([a.hash.clone()])).unwrap();
    assert_eq!(removed, vec![c.hash.clone()]);
    assert!(store.contains("p", &a.hash));
}
