//! Operator-funded run reservations and their auditable report lines.
//!
//! A reservation is persisted before a cloud run can be created. The run writer consumes the
//! reservation's sponsor name and writes the corresponding report in the same Firestore commit
//! as the run, so a sponsored queue entry can never exist without a report document.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SponsorError {
    #[error("sponsor name must contain 1 to 120 visible characters")]
    InvalidName,
    #[error("sponsor budget and per-run cap must be positive")]
    InvalidBudget,
    #[error("requested funding must be positive")]
    InvalidAmount,
    #[error("run {0} already has a different sponsor reservation")]
    ConflictingRun(String),
    #[error("requested funding {requested_micros} exceeds the per-run cap {cap_micros}")]
    CapExceeded {
        requested_micros: u64,
        cap_micros: u64,
    },
    #[error(
        "sponsor budget exhausted: {remaining_micros} micros remain, {requested_micros} requested"
    )]
    BudgetExceeded {
        requested_micros: u64,
        remaining_micros: u64,
    },
    #[error("sponsor ledger is missing: {0}")]
    MissingLedger(PathBuf),
    #[error("sponsor ledger already exists: {0}")]
    LedgerExists(PathBuf),
    #[error("sponsor ledger is invalid: {0}")]
    InvalidLedger(String),
    #[error("sponsor ledger I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("sponsor ledger serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sponsor ledger lock was poisoned")]
    LockPoisoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SponsorReport {
    pub run_id: String,
    pub sponsor: String,
    pub amount_micros: u64,
    pub cap_micros: u64,
    /// `reserved` is written before queueing; `queued` is written in the atomic run/report commit.
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LedgerState {
    sponsor: String,
    budget_micros: u64,
    cap_micros: u64,
    #[serde(default)]
    reports: Vec<SponsorReport>,
}

/// A local operator-account budget ledger. The source of truth for a queued sponsored run is the
/// Firestore report document written alongside it; this ledger reserves budget before that
/// request and prevents a second run from consuming the same available balance in this process.
pub struct SponsorLedger {
    path: PathBuf,
    state: Mutex<LedgerState>,
}

impl SponsorLedger {
    pub fn create(
        path: impl Into<PathBuf>,
        sponsor: impl Into<String>,
        budget_micros: u64,
        cap_micros: u64,
    ) -> Result<Self, SponsorError> {
        let sponsor = sponsor.into().trim().to_string();
        if sponsor.is_empty()
            || sponsor.chars().count() > 120
            || sponsor.chars().any(char::is_control)
        {
            return Err(SponsorError::InvalidName);
        }
        if budget_micros == 0 || cap_micros == 0 {
            return Err(SponsorError::InvalidBudget);
        }
        let path = path.into();
        let ledger = Self {
            path: path.clone(),
            state: Mutex::new(LedgerState {
                sponsor,
                budget_micros,
                cap_micros,
                reports: Vec::new(),
            }),
        };
        let parent = ledger.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let state = ledger
            .state
            .lock()
            .map_err(|_| SponsorError::LockPoisoned)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&serde_json::to_vec_pretty(&*state)?)?;
        temporary.persist_noclobber(&ledger.path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                SponsorError::LedgerExists(path)
            } else {
                SponsorError::Io(error.error)
            }
        })?;
        drop(state);
        Ok(ledger)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, SponsorError> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SponsorError::MissingLedger(path.clone())
            } else {
                SponsorError::Io(error)
            }
        })?;
        let state: LedgerState = serde_json::from_slice(&bytes)?;
        if state.sponsor.trim().is_empty() || state.cap_micros == 0 || state.budget_micros == 0 {
            return Err(SponsorError::InvalidLedger(
                "sponsor, budget, and cap must be configured".to_string(),
            ));
        }
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Reserve a run under the operator's visible sponsor name and enforce both caps.
    /// Repeating the same reservation is idempotent; changing its amount is refused.
    pub fn reserve(&self, run_id: &str, amount_micros: u64) -> Result<SponsorReport, SponsorError> {
        if amount_micros == 0 {
            return Err(SponsorError::InvalidAmount);
        }
        let run_id = run_id.trim();
        if run_id.is_empty() || run_id.len() > 200 {
            return Err(SponsorError::InvalidLedger("run id is invalid".to_string()));
        }
        let mut state = self.state.lock().map_err(|_| SponsorError::LockPoisoned)?;
        if let Some(existing) = state.reports.iter().find(|report| report.run_id == run_id) {
            if existing.amount_micros == amount_micros {
                return Ok(existing.clone());
            }
            return Err(SponsorError::ConflictingRun(run_id.to_string()));
        }
        if amount_micros > state.cap_micros {
            return Err(SponsorError::CapExceeded {
                requested_micros: amount_micros,
                cap_micros: state.cap_micros,
            });
        }
        let spent = state
            .reports
            .iter()
            .filter(|report| report.status != "failed")
            .map(|report| report.amount_micros)
            .fold(0_u64, u64::saturating_add);
        let remaining_micros = state.budget_micros.saturating_sub(spent);
        if amount_micros > remaining_micros {
            return Err(SponsorError::BudgetExceeded {
                requested_micros: amount_micros,
                remaining_micros,
            });
        }
        let report = SponsorReport {
            run_id: run_id.to_string(),
            sponsor: state.sponsor.clone(),
            amount_micros,
            cap_micros: state.cap_micros,
            status: "reserved".to_string(),
        };
        state.reports.push(report.clone());
        self.persist_state(&state)?;
        Ok(report)
    }

    /// Record the failure line and release reserved budget if queue creation failed.
    pub fn release(&self, run_id: &str) -> Result<SponsorReport, SponsorError> {
        let mut state = self.state.lock().map_err(|_| SponsorError::LockPoisoned)?;
        let report = state
            .reports
            .iter_mut()
            .find(|report| report.run_id == run_id)
            .ok_or_else(|| SponsorError::InvalidLedger("reservation is missing".to_string()))?;
        report.status = "failed".to_string();
        let report = report.clone();
        self.persist_state(&state)?;
        Ok(report)
    }

    /// Mark the reservation as queued after the run and report were committed together.
    pub fn mark_queued(&self, run_id: &str) -> Result<SponsorReport, SponsorError> {
        let mut state = self.state.lock().map_err(|_| SponsorError::LockPoisoned)?;
        let report = state
            .reports
            .iter_mut()
            .find(|report| report.run_id == run_id)
            .ok_or_else(|| SponsorError::InvalidLedger("reservation is missing".to_string()))?;
        report.status = "queued".to_string();
        let report = report.clone();
        self.persist_state(&state)?;
        Ok(report)
    }

    pub fn reports(&self) -> Result<Vec<SponsorReport>, SponsorError> {
        self.state
            .lock()
            .map(|state| state.reports.clone())
            .map_err(|_| SponsorError::LockPoisoned)
    }

    pub fn sponsor(&self) -> Result<String, SponsorError> {
        self.state
            .lock()
            .map(|state| state.sponsor.clone())
            .map_err(|_| SponsorError::LockPoisoned)
    }

    fn persist_state(&self, state: &LedgerState) -> Result<(), SponsorError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&serde_json::to_vec_pretty(state)?)?;
        temporary
            .persist(&self.path)
            .map_err(|error| SponsorError::Io(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_debits_operator_budget_and_reports_visible_sponsor_and_cap() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = SponsorLedger::create(
            directory.path().join("sponsor.json"),
            "Rigel Labs",
            500,
            200,
        )
        .unwrap();

        let report = ledger.reserve("run-a", 150).unwrap();

        assert_eq!(report.sponsor, "Rigel Labs");
        assert_eq!(report.cap_micros, 200);
        assert_eq!(report.amount_micros, 150);
        assert_eq!(report.status, "reserved");
        assert_eq!(
            SponsorLedger::open(directory.path().join("sponsor.json"))
                .unwrap()
                .reports()
                .unwrap(),
            vec![report]
        );
    }

    #[test]
    fn per_run_cap_and_total_operator_budget_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let ledger =
            SponsorLedger::create(directory.path().join("sponsor.json"), "Rigel", 250, 200)
                .unwrap();

        assert!(matches!(
            ledger.reserve("too-large", 201),
            Err(SponsorError::CapExceeded { .. })
        ));
        ledger.reserve("first", 150).unwrap();
        assert!(matches!(
            ledger.reserve("second", 101),
            Err(SponsorError::BudgetExceeded { .. })
        ));
    }

    #[test]
    fn same_run_reservation_is_idempotent_but_amount_changes_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let ledger =
            SponsorLedger::create(directory.path().join("sponsor.json"), "Rigel", 500, 200)
                .unwrap();

        let first = ledger.reserve("run-a", 100).unwrap();
        assert_eq!(ledger.reserve("run-a", 100).unwrap(), first);
        assert!(matches!(
            ledger.reserve("run-a", 101),
            Err(SponsorError::ConflictingRun(_))
        ));
        assert_eq!(ledger.reports().unwrap().len(), 1);
    }

    #[test]
    fn failed_queue_attempt_keeps_a_report_line_and_releases_budget() {
        let directory = tempfile::tempdir().unwrap();
        let ledger =
            SponsorLedger::create(directory.path().join("sponsor.json"), "Rigel", 150, 150)
                .unwrap();

        ledger.reserve("run-a", 150).unwrap();
        let failed = ledger.release("run-a").unwrap();
        assert_eq!(failed.sponsor, "Rigel");
        assert_eq!(failed.status, "failed");
        assert_eq!(ledger.reserve("run-b", 150).unwrap().sponsor, "Rigel");
    }

    #[test]
    fn configuring_again_cannot_erase_budget_or_audit_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sponsor.json");
        let ledger = SponsorLedger::create(&path, "Rigel", 500, 200).unwrap();
        ledger.reserve("run-a", 100).unwrap();

        assert!(matches!(
            SponsorLedger::create(&path, "Rigel", 10_000, 10_000),
            Err(SponsorError::LedgerExists(_))
        ));
        assert_eq!(
            SponsorLedger::open(path).unwrap().reports().unwrap().len(),
            1
        );
    }
}
