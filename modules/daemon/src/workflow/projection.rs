use super::evidence::{FileEvidence, capture};
use crate::{
    execution::checkpoint::{ExecutionCheckpoint, RunStatus},
    storage::resources::{Result, digest, fault},
};
use metteur_shared::workflow::{CheckBinding, CriterionPhase, read_contract};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct CriterionResult {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub phase: CriterionPhase,
    pub status: String,
    pub reason: String,
    pub command: Option<String>,
    pub output: Option<String>,
    pub files: Vec<FileEvidence>,
    pub sequence: Option<u64>,
    pub decision: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowReview {
    pub run_id: uuid::Uuid,
    pub goal: String,
    pub status: RunStatus,
    pub digest: String,
    pub revision: u64,
    pub criteria: Vec<CriterionResult>,
    pub can_review: bool,
    pub coverage: &'static str,
}

pub fn project(root: &Path, checkpoint: &ExecutionCheckpoint) -> Result<WorkflowReview> {
    let graph = checkpoint
        .view
        .root
        .as_ref()
        .ok_or_else(|| fault("unavailable", "This run has no recorded blueprint"))?;
    let contract = read_contract(graph)
        .map_err(|e| fault("invalid_contract", &e.to_string()))?
        .ok_or_else(|| fault("no_criteria", "This run has no acceptance criteria"))?;
    let current = capture(root, &contract);
    let terminal = matches!(
        checkpoint.status,
        RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
    );
    let mut criteria = Vec::new();
    for criterion in contract.criteria {
        let mut row = CriterionResult {
            id: criterion.id,
            description: criterion.description,
            required: criterion.required,
            phase: criterion.phase,
            status: "missing".into(),
            reason: "No committed check result is available.".into(),
            command: None,
            output: None,
            files: Vec::new(),
            sequence: None,
            decision: None,
        };
        match criterion.check {
            CheckBinding::Manual => {
                row.files = current.files.clone();
                if current.files.iter().all(|f| f.status == "available") {
                    row.status = "needs_review".into();
                    row.reason =
                        "Review the current file changes before accepting this requirement.".into();
                } else {
                    row.reason = "Some declared evidence files are missing or unavailable.".into();
                }
            }
            CheckBinding::Command {
                command_node,
                validator_node,
                command,
                expected_exit_code,
            } => {
                row.command = Some(command);
                let check = checkpoint.view.invocations.iter().rev().find(|i| {
                    i.current && i.node_id == validator_node && i.scope == graph.id.to_string()
                });
                let command = checkpoint.view.invocations.iter().rev().find(|i| {
                    i.current && i.node_id == command_node && i.scope == graph.id.to_string()
                });
                if let (Some(check), Some(command)) = (check, command) {
                    row.sequence = Some(check.sequence);
                    let text = serde_json::to_string(&command.outputs).unwrap_or_default();
                    row.output = Some(text);
                    let result_pin = graph
                        .node(command_node)
                        .and_then(|n| n.pins.iter().find(|p| p.name == "Result"));
                    let result = result_pin
                        .and_then(|p| command.outputs.get(p.id.to_string()))
                        .and_then(|v| {
                            v.as_str()
                                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                                .or_else(|| v.is_object().then(|| v.clone()))
                        });
                    let snapshot = checkpoint.view.workflow_evidence.get(&command.sequence);
                    if let Some(snapshot) = snapshot {
                        row.files = snapshot.files.clone();
                    }
                    if check.finished_at.is_none()
                        || command.finished_at.is_none()
                        || command.sequence >= check.sequence
                        || check.frame != command.frame
                        || check.status != "Completed"
                        || command.status != "Completed"
                    {
                        row.reason =
                            "The command and validator have not committed a matching result."
                                .into();
                    } else if checkpoint.blueprint_version.is_none()
                        || check.version != command.version
                        || check.version != checkpoint.blueprint_version
                        || snapshot.is_none_or(|s| s.contract_digest != current.contract_digest)
                    {
                        row.status = "stale".into();
                        row.reason =
                            "The check is not bound to this contract and blueprint version.".into();
                    } else if check.check == Some(false) {
                        row.status = "failed".into();
                        row.reason =
                            "The recorded command did not meet its expected exit code.".into();
                    } else if check.check != Some(true) {
                        row.reason =
                            "The host did not record a deterministic pass or failure.".into();
                    } else if result.as_ref().is_none_or(|v| {
                        v["state"] != "exited"
                            || v["exit_code"].as_i64() != Some(i64::from(expected_exit_code))
                    }) {
                        row.reason="A matching successful command completion was not recorded. A timeout, killed process or tool error cannot satisfy this check.".into();
                    } else if criterion.phase == CriterionPhase::Acceptance
                        && snapshot.is_some_and(|s| s.files != current.files)
                    {
                        row.status = "stale".into();
                        row.reason="A declared file changed after the check. The earlier pass is historical.".into();
                    } else if criterion.phase == CriterionPhase::Acceptance
                        && current.files.iter().any(|f| f.status != "available")
                    {
                        row.reason =
                            "The check passed, but some declared evidence files are unavailable."
                                .into();
                    } else {
                        row.status = "passed".into();
                        row.reason=if criterion.phase==CriterionPhase::Baseline{"The frozen baseline matched before implementation. Its file versions are historical."}else{"The recorded check passed and its evidence matches the current declared files."}.into();
                    }
                } else if !criterion.required && terminal {
                    row.status = "skipped".into();
                    row.reason =
                        "This optional check did not execute; it does not count as passed.".into();
                }
            }
        }
        criteria.push(row);
    }
    let binding = digest(
        &serde_json::to_vec(&(
            checkpoint.run_id,
            &checkpoint.blueprint_version,
            checkpoint.status,
            &current.contract_digest,
            &current.files,
            &criteria,
        ))
        .map_err(|_| fault("invalid_record", "Cannot encode review binding"))?,
    );
    Ok(WorkflowReview {
        run_id: checkpoint.run_id,
        goal: contract.goal,
        status: checkpoint.status,
        digest: binding,
        revision: 0,
        criteria,
        can_review: terminal && checkpoint.in_flight.is_none(),
        coverage: "Only declared file snapshots and recorded commands are covered. Shell, network and other external effects may be untracked.",
    })
}
