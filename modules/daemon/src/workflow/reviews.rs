use super::projection::WorkflowReview;
use crate::storage::{
    persistence::Db,
    resources::{ResourceStore, Result, StoredRecord, digest, fault},
};
use metteur_shared::resources::ResourceScope;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    ChangesRequested,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub run_id: Uuid,
    pub evidence_digest: String,
    pub criterion_id: String,
    pub decision: Decision,
    pub note: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub decision: Decision,
    pub note: String,
    pub subject: String,
    pub evidence_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewValue {
    run_id: Uuid,
    subject: String,
    last_request: String,
    decisions: BTreeMap<String, DecisionRecord>,
}

pub struct ReviewStore {
    db: Db,
    root: PathBuf,
    subject: String,
    store: ResourceStore,
}
impl ReviewStore {
    pub fn new(db: Db, root: &Path, subject: &str) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|_| fault("workspace_unavailable", "Workspace is unavailable"))?;
        let scope = ResourceScope::Personal(digest(
            &serde_json::to_vec(&(root.to_string_lossy(), subject))
                .map_err(|_| fault("invalid_scope", "Cannot resolve review scope"))?,
        ));
        let store = ResourceStore::new(db.clone(), &scope, "workflow_reviews")?;
        Ok(Self {
            db,
            root,
            subject: subject.into(),
            store,
        })
    }
    pub fn read(&self, mut projection: WorkflowReview) -> Result<WorkflowReview> {
        if let Some(record) = self.store.get(projection.run_id)? {
            let value: ReviewValue = serde_json::from_value(record.value)
                .map_err(|_| fault("invalid_record", "Review history is unavailable"))?;
            if value.run_id != projection.run_id || value.subject != self.subject {
                return Err(fault("permission_denied", "Review identity mismatch"));
            }
            projection.revision = record.revision;
            for row in &mut projection.criteria {
                if let Some(decision) = value.decisions.get(&row.id) {
                    let mut saved = serde_json::to_value(decision).unwrap();
                    saved["current"] =
                        serde_json::json!(decision.evidence_digest == projection.digest);
                    row.decision = Some(saved);
                }
            }
        }
        Ok(projection)
    }
    pub fn receipt(&self, request_id: Uuid) -> Result<Option<StoredRecord>> {
        self.store.receipt(request_id)
    }
    pub fn replay(
        &self,
        input: &ReviewInput,
        expected: u64,
        request_id: Uuid,
    ) -> Result<Option<StoredRecord>> {
        let fingerprint = digest(&serde_json::to_vec(&(&self.subject, expected, input)).unwrap());
        let previous = self.store.receipt(request_id)?;
        if previous.as_ref().is_some_and(|r| r.value["last_request"].as_str() != Some(&fingerprint))
        {
            return Err(fault("request_conflict", "This request ID belongs to another decision"));
        }
        Ok(previous)
    }
    /// Called under workspace admission. Human review is append-only and outside rewind.
    pub fn record(
        &self,
        input: ReviewInput,
        expected: u64,
        request_id: Uuid,
        projection: &WorkflowReview,
    ) -> Result<StoredRecord> {
        if request_id.is_nil()
            || input.note.len() > 2048
            || input.note.contains('\0')
            || input.criterion_id.len() > 64
            || input.evidence_digest.len() != 64
        {
            return Err(fault("invalid_request", "Invalid review fields or limits"));
        }
        let fingerprint = digest(&serde_json::to_vec(&(&self.subject, expected, &input)).unwrap());
        if let Some(previous) = self.replay(&input, expected, request_id)? {
            return Ok(previous);
        }
        if input.run_id != projection.run_id
            || input.evidence_digest != projection.digest
            || !projection.can_review
        {
            return Err(fault(
                "stale_evidence",
                "Reconcile this terminal run and its evidence before reviewing",
            ));
        }
        let criterion = projection
            .criteria
            .iter()
            .find(|c| c.id == input.criterion_id)
            .ok_or_else(|| fault("invalid_criterion", "The requirement is not in this run"))?;
        if input.decision == Decision::Accepted
            && !matches!(criterion.status.as_str(), "passed" | "needs_review")
        {
            return Err(fault(
                "evidence_unavailable",
                "Failed, missing, skipped or stale evidence cannot be accepted",
            ));
        }
        let current = self.store.get(input.run_id)?;
        let mut value = if let Some(record) = current {
            serde_json::from_value::<ReviewValue>(record.value)
                .map_err(|_| fault("invalid_record", "Review history is unavailable"))?
        } else {
            let mut count = 0;
            let mut after = None;
            loop {
                let page = self.store.list(after, 50)?;
                count += page.len();
                if count >= 128 {
                    return Err(fault(
                        "resource_limit",
                        "Review scope reached 128 runs; existing history remains readable",
                    ));
                }
                if page.len() < 50 {
                    break;
                }
                after = page.last().map(|(id, _)| *id);
            }
            ReviewValue {
                run_id: input.run_id,
                subject: self.subject.clone(),
                last_request: String::new(),
                decisions: BTreeMap::new(),
            }
        };
        if expected >= 64 {
            return Err(fault(
                "resource_limit",
                "This run reached 64 review revisions; history remains readable",
            ));
        }
        if value.subject != self.subject || value.run_id != input.run_id {
            return Err(fault("permission_denied", "Review identity mismatch"));
        }
        value.last_request = fingerprint;
        value.decisions.insert(
            input.criterion_id,
            DecisionRecord {
                decision: input.decision,
                note: input.note,
                subject: self.subject.clone(),
                evidence_digest: input.evidence_digest,
            },
        );
        let value = serde_json::to_value(value).unwrap();
        if value.to_string().len() > 16 * 1024 {
            return Err(fault(
                "resource_limit",
                "Review record exceeds 16 KiB; shorten review notes",
            ));
        }
        ResourceStore::prepare(&self.db, &self.root.join(".metteur/nx-before-v1"))?;
        self.store.write(input.run_id, expected, request_id, value)
    }
}
