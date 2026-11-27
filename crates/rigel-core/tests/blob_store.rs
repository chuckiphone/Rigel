//! Blob store integrity (roadmap 184) integration tests.

use rigel_core::BlobStore;
use std::collections::HashSet;

#[test]
fn dedup_verify_orphan_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = BlobStore::new(dir.path());
    let h1 = store.put(b"hello").unwrap();
    let h2 = store.put(b"hello").unwrap();
    assert_eq!(h1, h2);
    assert_eq!(store.get(&h1).unwrap(), b"hello");

    // corrupt
    std::fs::write(dir.path().join(&h1), b"tampered").unwrap();
    assert!(store.get(&h1).is_err());

    // orphan collection
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = BlobStore::new(dir2.path());
    let a = store2.put(b"a").unwrap();
    let b = store2.put(b"b").unwrap();
    let removed = store2.collect_orphans(&HashSet::from([a.clone()])).unwrap();
    assert_eq!(removed, vec![b.clone()]);
    assert!(store2.contains(&a));
    assert!(!store2.contains(&b));

    // bytes per project
    let dir3 = tempfile::tempdir().unwrap();
    let store3 = BlobStore::new(dir3.path());
    store3.put(b"hello").unwrap();
    store3.put_for_project("p", b"world").unwrap();
    let map = store3.bytes_per_project();
    assert!(map.contains_key("default"));
    assert!(map.contains_key("p"));
}
