use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CONTRACT_KEY: &str = "_workflow_contract";
pub const SCHEMA_VERSION: u32 = 1;
pub const TEMPLATE_REVISION: u32 = 1;
pub const MAX_CRITERIA: usize = 16;
pub const MAX_PATHS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    FixBug,
    AddTests,
    Refactor,
    AddFeature,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateInfo {
    pub id: TemplateKind,
    pub revision: u32,
    pub title: &'static str,
    pub description: &'static str,
    pub acceptance: &'static str,
}

pub fn templates() -> Vec<TemplateInfo> {
    use TemplateKind::*;
    [
        (
            FixBug,
            "Fix a bug",
            "Reproduce a known failure, make a focused fix and verify the result.",
            "The fixed regression fails before the change and passes afterwards.",
        ),
        (
            AddTests,
            "Add tests",
            "Capture existing behavior and prove the tests detect a specified defect.",
            "The tests pass on the correct code and detect the frozen defect.",
        ),
        (
            Refactor,
            "Refactor safely",
            "Improve implementation while preserving observable behavior and public interfaces.",
            "Independent behavior and interface checks remain unchanged.",
        ),
        (
            AddFeature,
            "Add a small feature",
            "Implement a bounded feature with normal, edge and failure cases.",
            "The new acceptance cases and existing regression checks pass.",
        ),
    ]
    .into_iter()
    .map(|(id, title, description, acceptance)| TemplateInfo {
        id,
        revision: TEMPLATE_REVISION,
        title,
        description,
        acceptance,
    })
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCheck {
    pub id: String,
    pub description: String,
    pub command: String,
    pub expected_exit_code: i32,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualCheck {
    pub id: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateInput {
    pub instance_id: Uuid,
    pub template: TemplateKind,
    pub name: String,
    pub goal: String,
    pub constraints: String,
    pub scope_paths: Vec<String>,
    pub evidence_paths: Vec<String>,
    pub model: String,
    pub max_iterations: u32,
    pub baseline: CommandCheck,
    pub checks: Vec<CommandCheck>,
    pub manual_checks: Vec<ManualCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckBinding {
    Command {
        command_node: Uuid,
        validator_node: Uuid,
        command: String,
        expected_exit_code: i32,
    },
    Manual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CriterionPhase {
    Baseline,
    Acceptance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub phase: CriterionPhase,
    pub check: CheckBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub title: String,
    pub nodes: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowContract {
    pub schema_version: u32,
    pub instance_id: Uuid,
    pub template: TemplateKind,
    pub template_revision: u32,
    pub goal: String,
    pub constraints: String,
    pub scope_paths: Vec<String>,
    pub evidence_paths: Vec<String>,
    pub criteria: Vec<Criterion>,
    pub stages: Vec<Stage>,
}
