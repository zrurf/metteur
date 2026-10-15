//! Versioned resource identities. Text provenance never grants capabilities.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub const MAX_QUERY_RESULTS: usize = 50;
pub const MAX_QUERY_BYTES: usize = 32 * 1024;

macro_rules! identity {
    ($($name:ident),+) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
    )+};
}
identity!(SessionId, TaskId, TurnId, SourceId, ResourceId);

/// The host resolves owner from the canonical workspace or authenticated subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", content = "owner", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceScope {
    Project(String),
    Personal(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRef {
    pub scope: ResourceScope,
    pub id: ResourceId,
    pub revision: u64,
    pub content_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    ModelOpinion,
    UserNote,
    UserConvention,
    ExecutionEvidence,
    Handoff,
    UserSelectedPrompt,
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validity { Current, NeedsReview, Archived, Tombstone, Unavailable }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub run_id: Uuid,
    pub invocation: Uuid,
    pub attempt: u64,
    pub check: String,
    pub version: String,
    pub validity: Validity,
}

/// Schema only in M1: no scheduler or automatic continuation is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationBinding {
    pub schema_version: u32,
    pub workspace_id: String,
    pub session_id: SessionId,
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub generation: u64,
    pub session_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub id: Uuid,
    pub binding: ContinuationBinding,
    pub source_id: SourceId,
    pub source_revision: u64,
    pub payload_digest: String,
}

/// Expiration fences new writes; it does not prove that a worker has stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectLease {
    pub task_id: TaskId,
    pub generation: u64,
    pub fencing_token: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRole { User, MainAgent, Supervisor, Concierge, ReadOnlyAgent }

impl ResourceRole {
    pub fn can_write_note(self) -> bool { matches!(self, Self::User | Self::MainAgent) }
    pub fn can_promote(self) -> bool { self == Self::User }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceError {
    pub code: String,
    pub message: String,
    pub expected_revision: Option<u64>,
    pub actual_revision: Option<u64>,
    pub current_digest: Option<String>,
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}
impl std::error::Error for ResourceError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag="kind", rename_all="snake_case", deny_unknown_fields)]
pub enum PromptPurpose { Chat, Blueprint { blueprint_id: Uuid, node_id: Uuid } }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSnapshot {
    pub schema_version: u32,
    pub id: Uuid,
    pub reference: ResourceRef,
    pub name: String,
    pub source_raw_digest: String,
    pub rendered_body_digest: String,
    pub body: String,
    pub selected_by: String,
    pub selected_at: u64,
    pub purpose: PromptPurpose,
    pub origin: Origin,
    pub parent_refs: Vec<ResourceRef>,
}
