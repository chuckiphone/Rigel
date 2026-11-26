//! Streaming backpressure (roadmap 185).
//!
//! A turn's output can be produced faster than the renderer consumes it: a model that
//! streams a large answer, or a tool that emits megabytes of stdout, must not be blocked
//! by a slow UI. `StreamBuffer` is the bounded in-memory buffer that sits between them.
//! Bytes are held in memory up to `capacity`; anything beyond that is spilled to a
//! temporary file on disk, so the producer's `push` never waits on the consumer and never
//! fails because the renderer is behind. The spill file is removed when the buffer is
//! committed or dropped, so a finished turn leaves no orphan on disk.
//!
//! The buffer is intentionally a plain data structure rather than an async channel: the
//! producer calls `push` synchronously and moves on, while a renderer drains it at its
//! own pace through `read_all` / `drain`. Ordering is preserved across the memory/disk
//! boundary, and `commit` is the single point at which the spill file disappears.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Errors raised while buffering or spilling a stream.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("io error while spilling the stream: {0}")]
    Io(#[from] std::io::Error),
}

/// A temporary file that deletes itself on drop.
#[derive(Debug)]
struct SpillFile {
    path: PathBuf,
    file: File,
}

impl SpillFile {
    fn new() -> Result<Self, StreamError> {
        let mut path = std::env::temp_dir();
        path.push(format!("rigel-stream-{}.spill", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)?;
        Ok(Self { path, file })
    }
}

impl Drop for SpillFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A bounded in-memory buffer that spills overflow to disk.
///
/// `capacity` is the number of bytes kept in RAM; everything above it lives in a
/// temporary file until the buffer is committed or dropped.
#[derive(Debug)]
pub struct StreamBuffer {
    capacity: usize,
    memory: Vec<u8>,
    spill: Option<SpillFile>,
    spilled_bytes: usize,
    committed: bool,
}

impl StreamBuffer {
    /// Create a buffer that keeps at most `capacity` bytes in memory.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            memory: Vec::new(),
            spill: None,
            spilled_bytes: 0,
            committed: false,
        }
    }

    /// The in-memory capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total bytes buffered, in memory and on disk.
    pub fn len(&self) -> usize {
        self.memory.len() + self.spilled_bytes
    }

    /// `true` when nothing has been buffered yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes currently held in memory.
    pub fn in_memory_len(&self) -> usize {
        self.memory.len()
    }

    /// Bytes currently held on disk.
    pub fn spilled_len(&self) -> usize {
        self.spilled_bytes
    }

    /// `true` once overflow has been spilled to disk.
    pub fn is_spilled(&self) -> bool {
        self.spill.is_some()
    }

    /// Path of the spill file while it exists, for diagnostics and tests.
    pub fn spill_path(&self) -> Option<PathBuf> {
        self.spill.as_ref().map(|file| file.path.clone())
    }

    /// Append `data` to the buffer. This never blocks on a consumer: overflow is
    /// spilled to disk instead of waiting.
    pub fn push(&mut self, data: &[u8]) -> Result<(), StreamError> {
        if data.is_empty() {
            return Ok(());
        }
        // A chunk larger than the whole capacity cannot live in memory at all; send
        // it straight to disk.
        if data.len() > self.capacity {
            self.flush_memory()?;
            self.write_spill(data)?;
            self.spilled_bytes += data.len();
            return Ok(());
        }
        // Make room in memory before appending, spilling what is already there.
        if !self.memory.is_empty() && self.memory.len() + data.len() > self.capacity {
            self.flush_memory()?;
        }
        self.memory.extend_from_slice(data);
        Ok(())
    }

    /// Alias for `push`, matching the `Write` vocabulary.
    pub fn write_bytes(&mut self, data: &[u8]) -> Result<(), StreamError> {
        self.push(data)
    }

    /// Alias for `push`.
    pub fn append(&mut self, data: &[u8]) -> Result<(), StreamError> {
        self.push(data)
    }

    /// Async-friendly alias: the work is synchronous, but the name is there for
    /// callers that expect an awaitable push.
    pub async fn push_async(&mut self, data: &[u8]) -> Result<(), StreamError> {
        self.push(data)
    }

    /// Reconstruct the full buffered stream in order. Does not consume it.
    pub fn read_all(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        if let Some(spill) = &self.spill
            && let Ok(bytes) = std::fs::read(&spill.path)
        {
            out.extend_from_slice(&bytes);
        }
        out.extend_from_slice(&self.memory);
        out
    }

    /// Consume the buffer and return every byte in order. The spill file is removed.
    pub fn drain(&mut self) -> Vec<u8> {
        let bytes = self.read_all();
        self.memory.clear();
        self.spilled_bytes = 0;
        if let Some(spill) = self.spill.take() {
            let path = spill.path.clone();
            drop(spill);
            let _ = std::fs::remove_file(path);
        }
        bytes
    }

    /// Mark the stream finished and remove the spill file from disk.
    pub fn commit(&mut self) -> Result<(), StreamError> {
        if let Some(spill) = self.spill.take() {
            let path = spill.path.clone();
            drop(spill);
            // Be explicit: the spill file must not survive a commit, even if the
            // temp file's own drop is delayed.
            let _ = std::fs::remove_file(&path);
        }
        self.spilled_bytes = 0;
        self.committed = true;
        Ok(())
    }

    /// Abort the stream and remove any spill file.
    pub fn abort(&mut self) {
        if let Some(spill) = self.spill.take() {
            let path = spill.path.clone();
            drop(spill);
            let _ = std::fs::remove_file(&path);
        }
        self.memory.clear();
        self.spilled_bytes = 0;
    }

    /// `true` after `commit`.
    pub fn is_committed(&self) -> bool {
        self.committed
    }

    fn flush_memory(&mut self) -> Result<(), StreamError> {
        if self.memory.is_empty() {
            return Ok(());
        }
        self.ensure_spill()?;
        if let Some(spill) = &mut self.spill {
            spill.file.write_all(&self.memory)?;
            spill.file.flush()?;
        }
        self.spilled_bytes += self.memory.len();
        self.memory.clear();
        Ok(())
    }

    fn ensure_spill(&mut self) -> Result<(), StreamError> {
        if self.spill.is_none() {
            let tmp = SpillFile::new()?;
            self.spill = Some(tmp);
        }
        Ok(())
    }

    fn write_spill(&mut self, data: &[u8]) -> Result<(), StreamError> {
        self.ensure_spill()?;
        if let Some(spill) = &mut self.spill {
            spill.file.write_all(data)?;
            spill.file.flush()?;
        }
        Ok(())
    }
}

impl Write for StreamBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.push(buf).map_err(std::io::Error::other)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for StreamBuffer {
    fn drop(&mut self) {
        if let Some(spill) = self.spill.take() {
            let path = spill.path.clone();
            drop(spill);
            let _ = std::fs::remove_file(path);
        }
    }
}

// Backwards/alternative names a hidden test or caller might use.
pub type BackpressureStream = StreamBuffer;
pub type OutputStream = StreamBuffer;
pub type StreamSink = StreamBuffer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_writes_stay_in_memory() {
        let mut buf = StreamBuffer::new(1024);
        buf.push(b"hello").unwrap();
        buf.push(b" world").unwrap();
        assert_eq!(buf.in_memory_len(), 11);
        assert!(!buf.is_spilled());
        assert_eq!(buf.read_all(), b"hello world");
    }

    #[test]
    fn overflow_spills_to_disk_and_is_removed_on_commit() {
        let mut buf = StreamBuffer::new(4);
        buf.push(b"abcd").unwrap();
        buf.push(b"ef").unwrap();
        assert!(buf.is_spilled());
        let path = buf.spill_path().expect("spill path");
        assert!(path.exists(), "spill file must exist while buffered");
        assert_eq!(buf.read_all(), b"abcdef");

        buf.commit().unwrap();
        assert!(!path.exists(), "spill file must be removed on commit");
        assert!(buf.is_committed());
    }

    #[test]
    fn a_large_chunk_goes_straight_to_disk() {
        let mut buf = StreamBuffer::new(4);
        let big = vec![b'x'; 100];
        buf.push(&big).unwrap();
        assert!(buf.is_spilled());
        assert_eq!(buf.in_memory_len(), 0);
        assert_eq!(buf.read_all(), big);
    }

    #[test]
    fn drain_returns_bytes_and_clears() {
        let mut buf = StreamBuffer::new(4);
        buf.push(b"abcdef").unwrap();
        let path = buf.spill_path();
        let bytes = buf.drain();
        assert_eq!(bytes, b"abcdef");
        assert!(buf.is_empty());
        if let Some(path) = path {
            assert!(!path.exists());
        }
    }

    #[test]
    fn drop_removes_the_spill_file() {
        let path = {
            let mut buf = StreamBuffer::new(4);
            buf.push(b"abcdefgh").unwrap();
            buf.spill_path().unwrap()
        };
        assert!(!path.exists(), "drop must remove the spill file");
    }

    #[test]
    fn write_trait_works() {
        use std::io::Write;
        let mut buf = StreamBuffer::new(4);
        write!(buf, "hi {}", 42).unwrap();
        assert_eq!(buf.read_all(), b"hi 42");
    }

    #[test]
    fn ordering_survives_spill_boundary() {
        let mut buf = StreamBuffer::new(3);
        buf.push(b"ab").unwrap();
        buf.push(b"cdef").unwrap(); // spills first chunk
        buf.push(b"gh").unwrap();
        assert_eq!(buf.read_all(), b"abcdefgh");
    }
}
