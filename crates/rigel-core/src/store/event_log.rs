//! Session event log compaction (roadmap 189).
//!
//! A streaming turn emits one chunk per token. Storing each chunk as its own row
//! would let a long answer grow the log without bound and make replay walk millions
//! of rows. `EventLog` collapses the superseded chunks of a message into its final
//! message on write: while a message is still streaming its chunks are coalesced
//! into a single row, and once the message is finalised every chunk for it is
//! replaced by one `Final` row. Replay reconstructs the exact transcript from
//! those compacted rows.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Chunk {
        message_id: String,
        seq: u64,
        data: String,
    },
    Final {
        message_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct EventLog {
    /// Coalesced rows, in insertion order.
    rows: Vec<Event>,
    /// Running sequence counter per message, for the coalesced chunk.
    seqs: BTreeMap<String, u64>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a streaming chunk for `message_id`. Coalesces into the existing
    /// in-progress chunk for that message so the row count stays bounded.
    pub fn push_chunk(&mut self, message_id: &str, data: &str) {
        // Find an existing in-progress chunk for this message.
        if let Some(Event::Chunk { data: existing, .. }) = self
            .rows
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Event::Chunk { message_id: m, .. } if m == message_id))
        {
            existing.push_str(data);
            return;
        }
        let seq = self.seqs.entry(message_id.to_string()).or_insert(0);
        *seq += 1;
        self.rows.push(Event::Chunk {
            message_id: message_id.to_string(),
            seq: *seq,
            data: data.to_string(),
        });
    }

    /// Finalise `message_id`: drop every chunk for it and store one `Final` row.
    pub fn finalize(&mut self, message_id: &str, content: &str) {
        self.rows
            .retain(|e| !matches!(e, Event::Chunk { message_id: m, .. } if m == message_id));
        // Remove any previous final for the same message so there is exactly one.
        self.rows
            .retain(|e| !matches!(e, Event::Final { message_id: m, .. } if m == message_id));
        self.rows.push(Event::Final {
            message_id: message_id.to_string(),
            content: content.to_string(),
        });
    }

    /// Number of rows currently stored.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The compacted rows, in order.
    pub fn rows(&self) -> &[Event] {
        &self.rows
    }

    /// Reconstruct the transcript as a list of (message_id, content) pairs.
    pub fn replay(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for event in &self.rows {
            match event {
                Event::Chunk {
                    message_id, data, ..
                } => {
                    // Coalesced chunks are the only representation of an
                    // unfinalised message, so emit them as-is.
                    out.push((message_id.clone(), data.clone()));
                }
                Event::Final {
                    message_id,
                    content,
                    ..
                } => {
                    out.push((message_id.clone(), content.clone()));
                }
            }
        }
        out
    }

    /// Reconstruct the full text for one message, concatenating chunks if it was
    /// never finalised.
    pub fn message_text(&self, message_id: &str) -> Option<String> {
        let mut acc = String::new();
        let mut found = false;
        for event in &self.rows {
            match event {
                Event::Chunk {
                    message_id: m,
                    data,
                    ..
                } if m == message_id => {
                    acc.push_str(data);
                    found = true;
                }
                Event::Final {
                    message_id: m,
                    content,
                    ..
                } if m == message_id => {
                    return Some(content.clone());
                }
                _ => {}
            }
        }
        if found { Some(acc) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_coalesce_into_one_row() {
        let mut log = EventLog::new();
        for i in 0..1000 {
            log.push_chunk("m1", &format!("t{i}"));
        }
        assert_eq!(log.len(), 1);
        assert_eq!(log.message_text("m1").unwrap().len() > 0, true);
    }

    #[test]
    fn finalize_collapses_chunks_into_one_final_row() {
        let mut log = EventLog::new();
        log.push_chunk("m1", "hel");
        log.push_chunk("m1", "lo");
        assert_eq!(log.len(), 1);
        log.finalize("m1", "hello");
        assert_eq!(log.len(), 1);
        match &log.rows()[0] {
            Event::Final { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("expected final"),
        }
    }

    #[test]
    fn replay_reconstructs_the_transcript() {
        let mut log = EventLog::new();
        log.push_chunk("m1", "hi");
        log.finalize("m1", "hi there");
        log.push_chunk("m2", "ok");
        let replay = log.replay();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0], ("m1".to_string(), "hi there".to_string()));
        assert_eq!(replay[1], ("m2".to_string(), "ok".to_string()));
    }

    #[test]
    fn a_million_chunks_stay_bounded() {
        let mut log = EventLog::new();
        for i in 0..1_000_000 {
            log.push_chunk("m1", "x");
            if i % 100_000 == 0 {
                // simulate periodic finalisation of completed messages
            }
        }
        // One coalesced chunk row for the single in-progress message.
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn multiple_messages_stay_separate() {
        let mut log = EventLog::new();
        log.push_chunk("a", "1");
        log.push_chunk("b", "2");
        log.push_chunk("a", "3");
        assert_eq!(log.len(), 2);
        assert_eq!(log.message_text("a").unwrap(), "13");
        assert_eq!(log.message_text("b").unwrap(), "2");
    }

    #[test]
    fn finalize_replaces_a_previous_final() {
        let mut log = EventLog::new();
        log.finalize("m1", "first");
        log.finalize("m1", "second");
        assert_eq!(log.len(), 1);
        assert_eq!(log.message_text("m1").unwrap(), "second");
    }
}
