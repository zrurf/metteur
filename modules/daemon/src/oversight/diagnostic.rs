//! Bounded, host-owned diagnostics. Provider text and user context are never copied.
use crate::DaemonError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    UnknownLegacy,
    Configuration,
    ProviderHttp,
    ProviderTransport,
    ProviderResponse,
    ToolName,
    ToolArguments,
    StructuredFinal,
    IterationLimit,
    Budget,
    Timeout,
    ApprovalRejected,
    ApprovalExpired,
    VersionStale,
    Application,
    Persistence,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Unknown,
    Configuration,
    Context,
    Budget,
    Provider,
    ToolDispatch,
    StructuredFinal,
    Iteration,
    Approval,
    VersionCheck,
    Application,
    Persistence,
}
impl Category {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnknownLegacy => "Unknown legacy: no failure category was recorded.",
            Self::Configuration => "Supervisor provider configuration is unavailable.",
            Self::ProviderHttp => "The provider returned an unsuccessful HTTP status.",
            Self::ProviderTransport => "The provider connection failed; usage may be unknown.",
            Self::ProviderResponse => "The provider response could not be decoded.",
            Self::ToolName => "The supervisor requested a tool outside its allowed set.",
            Self::ToolArguments => "Supervisor tool arguments failed validation.",
            Self::StructuredFinal => "The supervisor final response failed validation.",
            Self::IterationLimit => "The supervisor iteration limit was reached.",
            Self::Budget => "The oversight token budget is exhausted.",
            Self::Timeout => "The operation timed out; remote usage may continue.",
            Self::ApprovalRejected => "The user rejected this proposal; no approval was granted.",
            Self::ApprovalExpired => "The proposal confirmation expired or became unavailable.",
            Self::VersionStale => "The proposal no longer matches the file or execution state.",
            Self::Application => {
                "Proposal application did not complete; inspect execution and recovery state."
            }
            Self::Persistence => "Durable state could not be committed; recovery may be required.",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub category: Category,
    pub stage: Stage,
    pub message: String,
    pub run_id: Uuid,
    pub review_id: Uuid,
    pub proposal_id: Option<Uuid>,
    pub call_id: Option<Uuid>,
    pub http_status: Option<u16>,
}
impl Diagnostic {
    pub fn new(category: Category, stage: Stage, run: Uuid, review: Uuid) -> Self {
        Self {
            category,
            stage,
            message: category.message().into(),
            run_id: run,
            review_id: review,
            proposal_id: None,
            call_id: None,
            http_status: None,
        }
    }
    pub fn from_error(error: &DaemonError, stage: Stage, run: Uuid, review: Uuid) -> Self {
        let (category, stage) = classify(error, stage);
        let mut value = Self::new(category, stage, run, review);
        if let DaemonError::LlmStatus {
            status,
            ..
        } = error
        {
            value.http_status = Some(*status);
        }
        value
    }
}
pub(crate) fn error(category: Category, stage: Stage) -> DaemonError {
    DaemonError::Oversight {
        category,
        stage,
    }
}
pub(crate) fn classify(error: &DaemonError, stage: Stage) -> (Category, Stage) {
    match error {
        DaemonError::Oversight {
            category,
            stage,
        } => (*category, *stage),
        DaemonError::LlmStatus {
            ..
        } => (Category::ProviderHttp, Stage::Provider),
        DaemonError::LlmTransport(_) => (Category::ProviderTransport, Stage::Provider),
        DaemonError::Llm(_) => (Category::ProviderResponse, Stage::Provider),
        DaemonError::Persistence(_) => (Category::Persistence, Stage::Persistence),
        DaemonError::Io(_) => (Category::Application, Stage::Application),
        DaemonError::Execution(message) if message.contains("budget exhausted") => {
            (Category::Budget, Stage::Budget)
        }
        DaemonError::Sandbox(_) if stage == Stage::Approval => (Category::ApprovalExpired, stage),
        _ if stage == Stage::Persistence => (Category::Persistence, stage),
        _ if stage == Stage::VersionCheck => (Category::VersionStale, stage),
        _ if stage == Stage::StructuredFinal => (Category::StructuredFinal, stage),
        _ => (Category::Application, stage),
    }
}
