//! Daemon error types.

use thiserror::Error;

/// Errors that can occur within the daemon.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum DaemonError {
    /// An I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A value could not be serialized or deserialized.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// A required resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A resource already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// The caller lacks permission for the operation.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The workspace is locked by another session.
    #[error("workspace locked: {0}")]
    Locked(String),

    /// An execution failed.
    #[error("execution error: {0}")]
    Execution(String),

    /// A classified supervisor failure without provider or user content.
    #[error("oversight failure: {category:?} at {stage:?}")]
    Oversight {
        category: crate::oversight::diagnostic::Category,
        stage: crate::oversight::diagnostic::Stage,
    },

    /// Durable execution state could not be committed; new effects must stop.
    #[error("persistence failure: {0}")]
    Persistence(String),

    /// An LLM request failed.
    #[error("llm error: {0}")]
    Llm(String),

    /// The provider answered with a non-success HTTP status.
    ///
    /// Kept structured so retry logic can classify the failure (429 and 5xx
    /// are retryable, most 4xx are not) instead of parsing a message.
    #[error("llm returned {status}: {message}")]
    LlmStatus {
        /// The HTTP status code.
        status: u16,
        /// The provider's error body.
        message: String,
    },

    /// The request never produced a response (connect, DNS, timeout).
    #[error("llm transport error: {0}")]
    LlmTransport(String),

    /// Execution was interrupted by the user.
    #[error("execution interrupted: {0}")]
    Interrupted(String),

    /// Execution was paused.
    #[error("execution paused")]
    Paused,

    /// A TLS configuration could not be loaded or applied.
    #[error("tls error: {0}")]
    Tls(String),

    /// A command was rejected by the sandbox or its approval flow.
    #[error("sandbox error: {0}")]
    Sandbox(String),

    /// An MCP server interaction failed.
    #[error("mcp error: {0}")]
    Mcp(String),

    /// A language server interaction failed.
    #[error("lsp error: {0}")]
    Lsp(String),

    /// An addon manifest, install or plugin invocation failed.
    #[error("addon error: {0}")]
    Addon(String),

    /// An internal error occurred.
    #[error("internal error: {0}")]
    Internal(String),

    /// A Windows Service Control Manager operation failed.
    #[error("service error: {0}")]
    Service(String),
}

#[cfg(windows)]
impl From<windows_service::Error> for DaemonError {
    fn from(err: windows_service::Error) -> Self {
        DaemonError::Service(err.to_string())
    }
}

impl DaemonError {
    /// Maps the error to a gRPC status code.
    pub fn to_status(&self) -> tonic::Code {
        match self {
            DaemonError::NotFound(_) => tonic::Code::NotFound,
            DaemonError::AlreadyExists(_) => tonic::Code::AlreadyExists,
            DaemonError::PermissionDenied(_) => tonic::Code::PermissionDenied,
            DaemonError::Locked(_) => tonic::Code::FailedPrecondition,
            DaemonError::Execution(_) | DaemonError::Llm(_) | DaemonError::Oversight { .. } => tonic::Code::Internal,
            DaemonError::Persistence(_) => tonic::Code::FailedPrecondition,
            DaemonError::LlmStatus {
                status,
                ..
            } => match status {
                401 => tonic::Code::Unauthenticated,
                403 => tonic::Code::PermissionDenied,
                404 => tonic::Code::NotFound,
                408 | 504 => tonic::Code::DeadlineExceeded,
                429 => tonic::Code::ResourceExhausted,
                _ => tonic::Code::Unavailable,
            },
            DaemonError::LlmTransport(_) => tonic::Code::Unavailable,
            DaemonError::Interrupted(_) => tonic::Code::Aborted,
            DaemonError::Paused => tonic::Code::Aborted,
            DaemonError::Sandbox(_) => tonic::Code::FailedPrecondition,
            DaemonError::Mcp(_) | DaemonError::Lsp(_) => tonic::Code::Internal,
            DaemonError::Addon(_) => tonic::Code::Internal,
            DaemonError::Io(_)
            | DaemonError::Serialization(_)
            | DaemonError::Internal(_)
            | DaemonError::Service(_)
            | DaemonError::Tls(_) => tonic::Code::Internal,
        }
    }
}

impl From<metteur_shared::SharedError> for DaemonError {
    fn from(err: metteur_shared::SharedError) -> Self {
        match err {
            metteur_shared::SharedError::Fs(e) => DaemonError::Io(e),
            metteur_shared::SharedError::Serialization(e) => {
                DaemonError::Serialization(e.to_string())
            }
            metteur_shared::SharedError::NotFound(e) => DaemonError::NotFound(e),
            other => DaemonError::Internal(other.to_string()),
        }
    }
}

/// Convenience alias for daemon results.
pub type DaemonResult<T> = Result<T, DaemonError>;
