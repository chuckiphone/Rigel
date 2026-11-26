//! Streaming backpressure (roadmap 185), integration-level: a slow consumer does not
//! block the producer, and the spill file is gone after commit.

use rigel_core::agent::stream::StreamBuffer;
use std::io::Write;

#[tokio::test]
async fn a_slow_consumer_does_not_block_the_producer() {
    let mut buf = StreamBuffer::new(8);
    // Produce far more than the in-memory cap without any consumer reading.
    for _ in 0..1000 {
        buf.push(b"0123456789").unwrap();
    }
    assert!(buf.is_spilled());
    assert_eq!(buf.len(), 10_000);
    // Commit drops the spill file.
    let path = buf.spill_path().unwrap();
    assert!(path.exists());
    buf.commit().unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn commit_removes_the_spill_file_and_drain_clears() {
    let mut buf = StreamBuffer::new(4);
    buf.push(b"abcdefghij").unwrap();
    let path = buf.spill_path().expect("spilled");
    assert!(path.exists());
    let bytes = buf.drain();
    assert_eq!(bytes, b"abcdefghij");
    assert!(!path.exists());
    assert!(buf.is_empty());
}

#[tokio::test]
async fn write_trait_and_async_alias() {
    let mut buf = StreamBuffer::new(4);
    write!(buf, "hello {}", "world").unwrap();
    buf.push_async(b"!").await.unwrap();
    assert_eq!(buf.read_all(), b"hello world!");
}
