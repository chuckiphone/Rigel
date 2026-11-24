//! Filtered reads of the approval ledger (roadmap 83).
//!
//! The ledger is per project and already carries the session, agent and turn on every record.
//! This module is the read side the window uses: one filter type, one shared matcher, and a CSV
//! export that writes exactly the rows the matcher selected. A filtered CSV is a view, not a
//! verifiable chain: the full ledger remains the export that `rigel audit verify` can check
//! offline, and this module refuses to produce a view from a chain that does not verify.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::chain::{
    ApprovalRecord, BrokenLink, ChainError, check_records, csv_field, digest_bytes, ledger_path,
    read_records,
};

/// The fields the Audit log page can narrow by. Empty strings mean "do not filter"; they make
/// the URL and the wire shape simple and let a caller clear one field without special values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditFilter {
    /// The project path or its last segment.
    pub project: String,
    /// Free text across every field, including the serialized request.
    pub q: String,
    pub session: String,
    pub agent: String,
    pub turn: String,
    pub decision: String,
}

impl AuditFilter {
    /// Case-insensitive contains on every field. This is a reviewer's search, not a database
    /// query: a partial session id is useful when the full id is not at hand.
    pub fn matches(&self, root: &Path, record: &ApprovalRecord) -> bool {
        let project = root.display().to_string();
        let project_name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let request = record.request.to_string();
        let all = format!(
            "{project} {project_name} {} {} {} {} {} {} {} {request}",
            record.approval_id,
            record.session_id,
            record.agent_id,
            record.turn_id.as_deref().unwrap_or_default(),
            record.kind,
            record.decision,
            record.scope.as_deref().unwrap_or_default(),
        )
        .to_lowercase();

        let project_needle = self.project.trim().to_lowercase();
        if !project_needle.is_empty()
            && !project.to_lowercase().contains(&project_needle)
            && !project_name.to_lowercase().contains(&project_needle)
        {
            return false;
        }

        let needle = self.q.trim().to_lowercase();
        if !needle.is_empty() && !all.contains(&needle) {
            return false;
        }

        for (field, wanted) in [
            (record.session_id.as_str(), self.session.as_str()),
            (record.agent_id.as_str(), self.agent.as_str()),
            (
                record.turn_id.as_deref().unwrap_or_default(),
                self.turn.as_str(),
            ),
            (record.decision.as_str(), self.decision.as_str()),
        ] {
            let wanted = wanted.trim().to_lowercase();
            if !wanted.is_empty() && !field.to_lowercase().contains(&wanted) {
                return false;
            }
        }

        true
    }
}

/// The rows a reviewer asked for, with the chain's verdict. A broken chain still returns the
/// readable rows so the UI can show where the break is; export refuses them.
#[derive(Debug, Clone, Serialize)]
pub struct AuditView {
    pub project: String,
    pub total: usize,
    pub matched: usize,
    pub valid: bool,
    pub first_bad: Option<BrokenLink>,
    pub records: Vec<ApprovalRecord>,
}

/// A filtered CSV plus the evidence needed to say what it is: the exact bytes' digest, the
/// number of rows, and the intact chain tip the view was taken from.
#[derive(Debug, Clone, Serialize)]
pub struct FilteredExport {
    pub csv: String,
    pub rows: usize,
    pub total: usize,
    pub sha256: String,
    pub chain_tip: String,
}

/// Read and filter the ledger without writing anything. A malformed ledger line is an error;
/// a hash-chain break is reported in the view so the page can show `tampered at line N`.
pub fn query(root: &Path, filter: &AuditFilter) -> Result<AuditView, ChainError> {
    let path = ledger_path(root);
    let records = read_records(&path)?;
    let report = check_records(&path, &records);
    let matched: Vec<_> = records
        .iter()
        .filter(|record| filter.matches(root, record))
        .cloned()
        .collect();
    Ok(AuditView {
        project: root.display().to_string(),
        total: records.len(),
        matched: matched.len(),
        valid: report.valid,
        first_bad: report.first_bad,
        records: matched,
    })
}

/// Build the filtered CSV in memory. The same matcher serves [`query`], so the export cannot
/// carry a row the page did not show, or omit one it did.
pub fn export_csv(root: &Path, filter: &AuditFilter) -> Result<FilteredExport, ChainError> {
    let path = ledger_path(root);
    let records = read_records(&path)?;
    let report = check_records(&path, &records);
    if let Some(bad) = report.first_bad {
        return Err(ChainError::Broken {
            line: bad.line,
            reason: bad.reason,
        });
    }
    let matched: Vec<_> = records
        .iter()
        .filter(|record| filter.matches(root, record))
        .collect();

    let project = root.display().to_string();
    let mut csv = String::from(
        "project,seq,approval_id,session_id,agent_id,turn_id,kind,decision,scope,requested_at,decided_at,prev_hash,hash,request\n",
    );
    for record in &matched {
        let request = serde_json::to_string(&record.request)
            .map_err(|error| ChainError::Encode(error.to_string()))?;
        let fields = [
            project.clone(),
            record.seq.to_string(),
            record.approval_id.clone(),
            record.session_id.clone(),
            record.agent_id.clone(),
            record.turn_id.clone().unwrap_or_default(),
            record.kind.clone(),
            record.decision.clone(),
            record.scope.clone().unwrap_or_default(),
            record.requested_at.to_string(),
            record
                .decided_at
                .map(|at| at.to_string())
                .unwrap_or_default(),
            record.prev_hash.clone(),
            record.hash.clone(),
            request,
        ];
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                csv.push(',');
            }
            csv.push_str(&csv_field(field));
        }
        csv.push('\n');
    }

    let chain_tip = records
        .last()
        .map(|record| record.hash.clone())
        .unwrap_or_else(|| super::chain::GENESIS_HASH.to_string());
    Ok(FilteredExport {
        sha256: digest_bytes(csv.as_bytes()),
        csv,
        rows: matched.len(),
        total: records.len(),
        chain_tip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::chain::{ApprovalInput, append};
    use serde_json::json;

    fn approval(id: &str, session: &str, agent: &str, turn: &str, decision: &str) -> ApprovalInput {
        ApprovalInput {
            approval_id: id.to_string(),
            session_id: session.to_string(),
            agent_id: agent.to_string(),
            turn_id: Some(turn.to_string()),
            kind: "command".to_string(),
            request: json!({ "command": format!("echo {id}") }),
            decision: decision.to_string(),
            scope: Some("once".to_string()),
            requested_at: 1_700_000_000_000,
            decided_at: Some(1_700_000_000_100),
        }
    }

    #[test]
    fn every_filter_narrows_the_same_rows_and_export_matches_the_view() {
        let root = tempfile::tempdir().unwrap();
        append(root.path(), approval("a", "s1", "one", "t1", "allowed")).unwrap();
        append(root.path(), approval("b", "s2", "two", "t2", "denied")).unwrap();
        append(root.path(), approval("c", "s1", "two", "t3", "allowed")).unwrap();

        let all = AuditFilter::default();
        assert_eq!(query(root.path(), &all).unwrap().matched, 3);

        let by_session = AuditFilter {
            session: "s1".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_session).unwrap().matched, 2);

        let by_agent = AuditFilter {
            agent: "two".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_agent).unwrap().matched, 2);

        let by_turn = AuditFilter {
            turn: "t2".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_turn).unwrap().matched, 1);

        let by_decision = AuditFilter {
            decision: "denied".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_decision).unwrap().matched, 1);

        let by_text = AuditFilter {
            q: "echo b".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_text).unwrap().matched, 1);

        let project_name = root.path().file_name().unwrap().to_string_lossy();
        let by_project = AuditFilter {
            project: project_name.into_owned(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_project).unwrap().matched, 3);
        let by_wrong_project = AuditFilter {
            project: "some-other-project".to_string(),
            ..AuditFilter::default()
        };
        assert_eq!(query(root.path(), &by_wrong_project).unwrap().matched, 0);

        let export = export_csv(root.path(), &by_agent).unwrap();
        assert_eq!(export.rows, 2);
        assert_eq!(export.total, 3);
        assert!(export.csv.starts_with("project,seq,approval_id"));
        assert!(export.csv.contains(",b,") && export.csv.contains(",c,"));
        assert!(!export.csv.contains("echo a"));
        assert_eq!(export.sha256.len(), 64);
        assert_eq!(export.chain_tip.len(), 64);
    }

    #[test]
    fn a_broken_chain_is_reported_but_not_exported() {
        let root = tempfile::tempdir().unwrap();
        append(root.path(), approval("a", "s1", "one", "t1", "allowed")).unwrap();
        let path = ledger_path(root.path());
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("allowed", "denied")).unwrap();

        let view = query(root.path(), &AuditFilter::default()).unwrap();
        assert!(!view.valid);
        assert_eq!(view.first_bad.as_ref().unwrap().line, 1);
        assert_eq!(view.records.len(), 1);

        assert!(matches!(
            export_csv(root.path(), &AuditFilter::default()),
            Err(ChainError::Broken { line: 1, .. })
        ));
    }
}
