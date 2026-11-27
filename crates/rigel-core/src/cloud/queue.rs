//! Lease-based queue state machine for cloud runs.
//!
//! The queue keeps each original request immutable across retries. A worker receives a lease with
//! an expiry; an expired lease returns the run to the queue unless its bounded attempt budget is
//! exhausted, in which case the run appears in the dead-letter view with its last error.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    /// The complete request body is retained so a manual requeue runs exactly the same request.
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Queued,
    Leased,
    Succeeded,
    Failed,
    Cancelled,
    DeadLetter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub worker: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub request: RunRequest,
    pub state: RunState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub lease: Option<Lease>,
    pub last_error: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("the retry limit must be greater than zero")]
    InvalidRetryLimit,
    #[error("a lease must have a positive duration")]
    InvalidLeaseDuration,
    #[error("run {0} does not exist")]
    UnknownRun(String),
    #[error("run {0} is not in a state that permits this operation")]
    InvalidState(String),
    #[error("worker name cannot be empty")]
    EmptyWorker,
}

/// Deterministic queue state machine. A storage adapter can persist `runs()` between operations.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RunQueue {
    next_id: u64,
    runs: BTreeMap<String, Run>,
}

impl RunQueue {
    pub fn enqueue(&mut self, request: RunRequest, max_attempts: u32) -> Result<Run, QueueError> {
        if max_attempts == 0 {
            return Err(QueueError::InvalidRetryLimit);
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = format!("run-{:08}", self.next_id);
        let run = Run {
            id: id.clone(),
            request,
            state: RunState::Queued,
            attempts: 0,
            max_attempts,
            lease: None,
            last_error: None,
        };
        self.runs.insert(id, run.clone());
        Ok(run)
    }

    /// Lease the next queued run in stable id order. Expired leases are reclaimed first.
    pub fn lease_next(
        &mut self,
        worker: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<Run>, QueueError> {
        if worker.trim().is_empty() {
            return Err(QueueError::EmptyWorker);
        }
        if lease_ms <= 0 {
            return Err(QueueError::InvalidLeaseDuration);
        }
        self.reap_expired(now_ms);
        let id = self
            .runs
            .iter()
            .find_map(|(id, run)| (run.state == RunState::Queued).then(|| id.clone()));
        let Some(id) = id else { return Ok(None) };
        let run = self
            .runs
            .get_mut(&id)
            .ok_or_else(|| QueueError::UnknownRun(id.clone()))?;
        run.attempts = run.attempts.saturating_add(1);
        run.state = RunState::Leased;
        run.lease = Some(Lease {
            worker: worker.to_owned(),
            expires_at_ms: now_ms.saturating_add(lease_ms),
        });
        Ok(Some(run.clone()))
    }

    /// Expired worker leases are retried up to the run's configured limit.
    pub fn reap_expired(&mut self, now_ms: i64) -> Vec<Run> {
        let expired: Vec<String> = self
            .runs
            .iter()
            .filter_map(|(id, run)| {
                run.lease
                    .as_ref()
                    .filter(|lease| lease.expires_at_ms <= now_ms)
                    .map(|_| id.clone())
            })
            .collect();
        expired
            .into_iter()
            .filter_map(|id| {
                let run = self.runs.get_mut(&id)?;
                run.lease = None;
                run.last_error = Some("worker lease expired".to_owned());
                run.state = if run.attempts >= run.max_attempts {
                    RunState::DeadLetter
                } else {
                    RunState::Queued
                };
                Some(run.clone())
            })
            .collect()
    }

    pub fn complete(&mut self, id: &str, worker: &str, now_ms: i64) -> Result<Run, QueueError> {
        self.transition_leased(id, worker, now_ms, |run| {
            run.state = RunState::Succeeded;
            run.last_error = None;
        })
    }

    /// Record a failure. The run is retried until its bounded lease-attempt budget is exhausted.
    pub fn fail(
        &mut self,
        id: &str,
        worker: &str,
        now_ms: i64,
        error: impl Into<String>,
    ) -> Result<Run, QueueError> {
        let error = error.into();
        self.transition_leased(id, worker, now_ms, |run| {
            run.lease = None;
            run.last_error = Some(error);
            run.state = if run.attempts >= run.max_attempts {
                RunState::DeadLetter
            } else {
                RunState::Queued
            };
        })
    }

    /// Cancel only work that has not been leased to a worker.
    pub fn cancel(&mut self, id: &str) -> Result<Run, QueueError> {
        let run = self
            .runs
            .get_mut(id)
            .ok_or_else(|| QueueError::UnknownRun(id.to_owned()))?;
        if run.state != RunState::Queued {
            return Err(QueueError::InvalidState(id.to_owned()));
        }
        run.state = RunState::Cancelled;
        Ok(run.clone())
    }

    /// Manually requeue a failed, cancelled, or dead-letter run without changing its request.
    pub fn requeue(&mut self, id: &str) -> Result<Run, QueueError> {
        let run = self
            .runs
            .get_mut(id)
            .ok_or_else(|| QueueError::UnknownRun(id.to_owned()))?;
        if !matches!(
            run.state,
            RunState::Failed | RunState::Cancelled | RunState::DeadLetter
        ) {
            return Err(QueueError::InvalidState(id.to_owned()));
        }
        run.state = RunState::Queued;
        run.attempts = 0;
        run.lease = None;
        run.last_error = None;
        Ok(run.clone())
    }

    /// Return every terminally failed run, in stable id order, including the last failure reason.
    pub fn dead_letters(&self) -> Vec<Run> {
        self.runs
            .values()
            .filter(|run| run.state == RunState::DeadLetter)
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&Run> {
        self.runs.get(id)
    }

    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.runs.values()
    }

    fn transition_leased(
        &mut self,
        id: &str,
        worker: &str,
        now_ms: i64,
        transition: impl FnOnce(&mut Run),
    ) -> Result<Run, QueueError> {
        let run = self
            .runs
            .get_mut(id)
            .ok_or_else(|| QueueError::UnknownRun(id.to_owned()))?;
        if run.state != RunState::Leased {
            return Err(QueueError::InvalidState(id.to_owned()));
        }
        if !run
            .lease
            .as_ref()
            .is_some_and(|lease| lease.worker == worker && lease.expires_at_ms > now_ms)
        {
            return Err(QueueError::InvalidState(id.to_owned()));
        }
        transition(run);
        Ok(run.clone())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request() -> RunRequest {
        RunRequest {
            payload: json!({"prompt": "keep this", "options": {"model": "m"}}),
        }
    }

    #[test]
    fn expired_lease_is_reclaimed_and_bounded_retries_dead_letter() {
        let mut queue = RunQueue::default();
        let run = queue.enqueue(request(), 2).unwrap();
        queue.lease_next("worker-a", 10, 5).unwrap();
        let retried = queue.reap_expired(15);
        assert_eq!(retried[0].state, RunState::Queued);
        assert_eq!(
            retried[0].last_error.as_deref(),
            Some("worker lease expired")
        );
        queue.lease_next("worker-b", 15, 5).unwrap();
        let failed = queue.reap_expired(20);
        assert_eq!(failed[0].state, RunState::DeadLetter);
        assert_eq!(queue.dead_letters()[0].id, run.id);
    }

    #[test]
    fn failed_run_retries_until_limit_and_cancel_requires_unleased_run() {
        let mut queue = RunQueue::default();
        let run = queue.enqueue(request(), 2).unwrap();
        assert_eq!(queue.cancel(&run.id).unwrap().state, RunState::Cancelled);
        assert_eq!(queue.requeue(&run.id).unwrap().request, run.request);
        queue.lease_next("worker", 0, 10).unwrap();
        assert_eq!(
            queue.fail(&run.id, "worker", 1, "temporary").unwrap().state,
            RunState::Queued
        );
        queue.lease_next("worker", 1, 10).unwrap();
        let dead = queue.fail(&run.id, "worker", 2, "permanent").unwrap();
        assert_eq!(dead.state, RunState::DeadLetter);
        assert_eq!(dead.last_error.as_deref(), Some("permanent"));
        assert_eq!(queue.dead_letters().len(), 1);
        assert!(matches!(
            queue.cancel(&run.id),
            Err(QueueError::InvalidState(_))
        ));
    }

    #[test]
    fn manual_requeue_preserves_the_original_request_and_resets_budget() {
        let mut queue = RunQueue::default();
        let run = queue.enqueue(request(), 1).unwrap();
        queue.lease_next("worker", 0, 10).unwrap();
        queue.fail(&run.id, "worker", 1, "boom").unwrap();
        let requeued = queue.requeue(&run.id).unwrap();
        assert_eq!(requeued.request, run.request);
        assert_eq!(requeued.attempts, 0);
        assert_eq!(requeued.state, RunState::Queued);
        assert_eq!(requeued.last_error, None);
    }

    #[test]
    fn only_the_current_unexpired_lease_holder_can_finish_a_run() {
        let mut queue = RunQueue::default();
        let run = queue.enqueue(request(), 2).unwrap();
        queue.lease_next("worker-a", 0, 10).unwrap();
        assert!(matches!(
            queue.complete(&run.id, "worker-b", 1),
            Err(QueueError::InvalidState(_))
        ));
        assert!(matches!(
            queue.complete(&run.id, "worker-a", 10),
            Err(QueueError::InvalidState(_))
        ));
        assert_eq!(
            queue.complete(&run.id, "worker-a", 9).unwrap().state,
            RunState::Succeeded
        );
    }
}
