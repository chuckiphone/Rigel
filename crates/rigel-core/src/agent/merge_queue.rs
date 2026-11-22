//! One writer per path (roadmap 46, Q122, Q255).
//!
//! Claims are advisory: they say who intends to edit a file, but they do not stop a second
//! writer. This queue is the small piece of enforcement the write path needs. Every patch,
//! worktree merge, conflict write and revert takes a lease keyed by the project root and the
//! project-relative path before it reads the file for a compare-and-swap. A second writer
//! registers as waiting, names the holder it saw, and is admitted only when that lease drops.
//!
//! The lane is per path, not per session: two agents may edit different files at once, but no
//! two writers can be inside the read/check/write window for the same file. The queue is
//! deliberately in memory. A crash ends the process that held a lease; the durable journal in
//! `patches.rs` is what makes the interrupted write recoverable. Queue order is valuable only
//! while the process that is doing the writing is alive.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Notify;

use crate::migrate::now_ms;

/// How a writer appears to the Diff view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeQueueState {
    /// This writer holds the path and is in the patch pipeline.
    Writing,
    /// This writer is behind the holder for the same path.
    Waiting,
}

/// One live queue slot. The diff view renders these beside the claims, so the holder is named
/// in the same place a user looks for the write they are waiting on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergeQueueEntry {
    pub path: String,
    pub writer_id: String,
    /// The writer at the front of this path's queue. For a waiting row this is the writer it
    /// must wait for, not its own id.
    pub holder_id: String,
    pub state: MergeQueueState,
    /// One-based position for a waiting writer; zero for the holder.
    pub position: usize,
    /// When this ticket joined the queue.
    pub since: i64,
}

/// The Diff view's read of every live lane in one project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MergeQueueView {
    pub entries: Vec<MergeQueueEntry>,
}

/// The guard held across one read/check/write window. Dropping it admits the next writer.
pub struct QueueLease {
    gate: Arc<PathGate>,
    ticket_id: String,
    waited_on: Option<String>,
    waited: Duration,
}

impl QueueLease {
    /// The writer that held the path when this ticket joined, if it had to wait.
    pub fn waited_on(&self) -> Option<&str> {
        self.waited_on.as_deref()
    }

    /// How long this ticket waited before it was admitted.
    pub fn waited(&self) -> Duration {
        self.waited
    }
}

impl Drop for QueueLease {
    fn drop(&mut self) {
        release(&self.gate, &self.ticket_id);
    }
}

#[derive(Debug, Clone)]
struct Ticket {
    id: String,
    writer_id: String,
    since: i64,
}

/// Removes a cancelled waiter instead of leaving a dead ticket at the head of the lane. The
/// patch call can be dropped when a turn is cancelled; without this guard the next writer would
/// wait forever for a ticket that no longer has a caller.
struct WaitingTicket {
    gate: Arc<PathGate>,
    id: String,
    admitted: bool,
}

impl Drop for WaitingTicket {
    fn drop(&mut self) {
        if self.admitted {
            return;
        }
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .holder
            .as_ref()
            .is_some_and(|holder| holder.id == self.id)
        {
            state.holder = state.waiting.pop_front();
        } else if let Some(index) = state.waiting.iter().position(|ticket| ticket.id == self.id) {
            state.waiting.remove(index);
        }
        drop(state);
        self.gate.notify.notify_waiters();
    }
}

#[derive(Debug, Default)]
struct GateState {
    holder: Option<Ticket>,
    waiting: VecDeque<Ticket>,
}

#[derive(Debug)]
struct PathGate {
    state: Mutex<GateState>,
    notify: Notify,
}

type Key = (String, String);

fn registry() -> &'static Mutex<HashMap<Key, Arc<PathGate>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<Key, Arc<PathGate>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key is the canonical project root plus the normalised relative path. A worktree has a
/// different root, so its isolated edits do not take the main tree's lane until it is merged.
fn key(root: &Path, path: &str) -> Key {
    (root_key(root), normalize_path(path))
}

fn root_key(root: &Path) -> String {
    let path: PathBuf = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rendered = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        rendered.to_lowercase()
    } else {
        rendered
    }
}

fn normalize_path(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    path
}

fn gate_for(key: Key) -> Arc<PathGate> {
    let mut lanes = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    lanes
        .entry(key)
        .or_insert_with(|| {
            Arc::new(PathGate {
                state: Mutex::new(GateState::default()),
                notify: Notify::new(),
            })
        })
        .clone()
}

/// Wait for the one writer slot for `path`.
///
/// A free lane is granted without yielding. Otherwise the ticket is appended behind every
/// writer already waiting, and the caller can read [`QueueLease::waited_on`] to see who it is
/// waiting for. The holder is named in [`snapshot`] while the wait is live.
pub async fn acquire(root: &Path, path: &str, writer_id: &str) -> QueueLease {
    let path = normalize_path(path);
    let gate = gate_for(key(root, &path));
    let ticket = Ticket {
        id: uuid::Uuid::new_v4().to_string(),
        writer_id: writer_id.to_string(),
        since: now_ms(),
    };

    let waited_on;
    {
        let mut state = gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.holder.is_none() && state.waiting.is_empty() {
            state.holder = Some(ticket.clone());
            drop(state);
            return QueueLease {
                gate,
                ticket_id: ticket.id,
                waited_on: None,
                waited: Duration::ZERO,
            };
        }
        waited_on = state
            .holder
            .as_ref()
            .map(|holder| holder.writer_id.clone())
            .or_else(|| state.waiting.front().map(|front| front.writer_id.clone()));
        state.waiting.push_back(ticket.clone());
    }

    let mut waiting = WaitingTicket {
        gate: gate.clone(),
        id: ticket.id.clone(),
        admitted: false,
    };
    let started = Instant::now();
    loop {
        let admitted = {
            let state = gate
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // A release hands the lane directly to the next ticket. Checking the id here (not
            // merely `holder.is_none()`) is what keeps two waiters from both believing they
            // were admitted after one release.
            state
                .holder
                .as_ref()
                .is_some_and(|holder| holder.id == ticket.id)
        };
        if admitted {
            waiting.admitted = true;
            return QueueLease {
                gate,
                ticket_id: ticket.id,
                waited_on,
                waited: started.elapsed(),
            };
        }
        // The timeout covers a release that happened between the check and the wait. Writes are
        // short, so a 25 ms fallback is both cheap and enough to keep the queue moving.
        let _ = tokio::time::timeout(Duration::from_millis(25), gate.notify.notified()).await;
    }
}

fn release(gate: &Arc<PathGate>, ticket_id: &str) {
    let mut state = gate
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state
        .holder
        .as_ref()
        .is_some_and(|holder| holder.id == ticket_id)
    {
        // Hand the lane over while the lock is held: the next waiter never observes a gap in
        // which a new writer could jump ahead of it.
        state.holder = state.waiting.pop_front();
    }
    drop(state);
    gate.notify.notify_waiters();
}

/// Read every live lane for one project. A path appears once while it is held, with its holder
/// first, followed by the writers waiting on it in order.
pub fn snapshot(root: &Path) -> MergeQueueView {
    let root_key = root_key(root);
    let lanes = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut entries = Vec::new();
    for ((lane_root, path), gate) in lanes.iter() {
        if lane_root != &root_key {
            continue;
        }
        let state = gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(holder) = &state.holder {
            entries.push(MergeQueueEntry {
                path: path.clone(),
                writer_id: holder.writer_id.clone(),
                holder_id: holder.writer_id.clone(),
                state: MergeQueueState::Writing,
                position: 0,
                since: holder.since,
            });
            for (index, waiter) in state.waiting.iter().enumerate() {
                entries.push(MergeQueueEntry {
                    path: path.clone(),
                    writer_id: waiter.writer_id.clone(),
                    holder_id: holder.writer_id.clone(),
                    state: MergeQueueState::Waiting,
                    position: index + 1,
                    since: waiter.since,
                });
            }
        }
    }
    entries.sort_by(|one, two| {
        one.path
            .cmp(&two.path)
            .then_with(|| one.position.cmp(&two.position))
    });
    MergeQueueView { entries }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Two tickets for one path must not both be writing; the second names the first as its
    /// holder and only starts after the first lease is dropped.
    #[tokio::test]
    async fn a_second_writer_waits_on_the_named_holder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = "src/lib.rs";
        let first = acquire(tmp.path(), path, "writer-a").await;
        assert_eq!(snapshot(tmp.path()).entries.len(), 1);

        let second = tokio::spawn({
            let root = tmp.path().to_path_buf();
            async move { acquire(&root, path, "writer-b").await }
        });
        // Let the ticket register; the first lease is still held.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let view = snapshot(tmp.path());
        assert_eq!(view.entries.len(), 2);
        assert_eq!(view.entries[0].state, MergeQueueState::Writing);
        assert_eq!(view.entries[0].writer_id, "writer-a");
        assert_eq!(view.entries[1].state, MergeQueueState::Waiting);
        assert_eq!(view.entries[1].holder_id, "writer-a");
        assert_eq!(view.entries[1].writer_id, "writer-b");
        assert_eq!(view.entries[1].position, 1);

        drop(first);
        let second = second.await.unwrap();
        assert_eq!(second.waited_on(), Some("writer-a"));
        assert!(second.waited() >= Duration::from_millis(25));
        assert!(snapshot(tmp.path()).entries.iter().all(|entry| {
            entry.state == MergeQueueState::Writing && entry.writer_id == "writer-b"
        }));
    }

    /// The lease is the actual exclusion: the counter can never observe two writers inside the
    /// window for one path, even when four tickets are started together.
    #[tokio::test]
    async fn no_two_writers_reach_one_path_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut writers = Vec::new();
        for index in 0..4 {
            let root = tmp.path().to_path_buf();
            let active = active.clone();
            let peak = peak.clone();
            writers.push(tokio::spawn(async move {
                let _lease = acquire(&root, "shared.txt", &format!("writer-{index}")).await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for writer in writers {
            writer.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert!(snapshot(tmp.path()).entries.is_empty());
    }

    /// Different paths are different lanes: limiting one file does not serialise the whole project.
    #[tokio::test]
    async fn different_paths_do_not_block_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let one = acquire(tmp.path(), "src/one.rs", "writer-a").await;
        let two = acquire(tmp.path(), "src/two.rs", "writer-b").await;
        assert_eq!(two.waited_on(), None);
        assert_eq!(snapshot(tmp.path()).entries.len(), 2);
        drop(one);
        drop(two);
    }
}
