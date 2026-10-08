//! gRPC service implementation, split by domain.
//!
//! The `Daemon` trait requires a single `impl Daemon for DaemonService` block,
//! so each domain file provides the handlers as inherent `pub(crate)`
//! `DaemonService` methods and the trait impl below delegates to them. Inherent
//! methods win Rust's method resolution over the trait ones, so the delegates
//! always reach the real handler (never recurse).

mod reports;
mod blackboard;
mod blueprint;
mod chat;
mod config;
mod concierge;
mod files;
mod jobs;
mod registry;
mod sandbox;
mod state;
mod versioning;
mod workspace;

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::sandbox::approval::ApprovalBroker;

use super::proto::daemon_server::Daemon;
use super::proto::{
    AbortChatRequest, AddonInfo, AddonList, ApprovalDecisionRequest, AuditLogList, Blueprint,
    CancelRequest, ChatEvent, ChatSessionList, CloseWorkspaceRequest, CompileDslRequest,
    Config as ProtoConfig, ContinueExecutionRequest, CreateDirRequest, CreateSnapshotRequest,
    DecompileBlueprintRequest, DecompileDslResponse, DeleteChatSessionRequest,
    DeleteFunctionRequest, Empty, ExecuteBlueprintRequest, ExecutionEvent, ExecutionList,
    ExecutionTree, FileHistory, FileInfo, FileList, FunctionList, GetChatSessionRequest,
    GetChatSessionResponse, GetConfigRequest, GetExecutionTreeRequest, GetExecutionUsageRequest,
    GetFileAtSnapshotRequest, GetFileAtSnapshotResponse, GetFileHistoryRequest,
    InstallAddonRequest, InterruptRequest, JobEvent, JobList, KillJobRequest, KillJobResponse,
    ListAddonsRequest, ListAuditLogRequest, ListChatSessionsRequest, ListExecutionsRequest,
    ListFilesRequest, ListFunctionsRequest, ListJobsRequest, ListSnapshotsRequest,
    LoadBlueprintRequest, LoadFunctionRequest, LoadFunctionResponse, McpServerList, NodeKindList,
    OpenWorkspaceRequest, PauseRequest, ReadFileRequest, ReadFileResponse, RemoveFileRequest,
    RenameFileRequest, ResumeRequest, RevealInExplorerRequest, RollbackRequest,
    SaveBlueprintRequest, SaveFunctionRequest, SaveFunctionResponse, SendChatRequest,
    SetAddonEnabledRequest, SetConfigRequest, SnapshotInfo, SnapshotList, StatFileRequest,
    ToolList, UninstallAddonRequest, UsageSummary, WatchEvent, WatchJobsRequest,
    WatchWorkspaceRequest, WorkspaceInfo, WorkspaceList, WriteFileRequest,
};

pub use state::AppState;
pub(crate) use state::*;

/// The tonic service implementing the `Daemon` RPCs.
pub struct DaemonService {
    state: Arc<AppState>,
}

impl DaemonService {
    /// Creates a new service backed by the given state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
        }
    }

    /// Returns the approval broker of the active run (execution or chat) for a
    /// workspace, if any.
    pub(crate) async fn approval_broker_for(
        &self,
        ws_key: &std::path::Path,
    ) -> Option<Arc<ApprovalBroker>> {
        if let Some(entry) = self.state.running.read().await.get(ws_key) {
            return entry.approvals.clone();
        }
        self.state.chats.read().await.get(ws_key).and_then(|c| c.approvals.clone())
    }
}

#[tonic::async_trait]
impl Daemon for DaemonService {
    async fn list_oversight_reports(&self, request: Request<super::proto::OversightReportsRequest>) -> Result<Response<super::proto::OversightReports>, Status> { self.list_oversight_reports(request).await }

    type SendConciergeMessageStream = tokio_stream::wrappers::ReceiverStream<Result<super::proto::ConciergeEvent, Status>>;
    async fn get_concierge_state(&self, request: Request<super::proto::ConciergeStateRequest>) -> Result<Response<super::proto::ConciergeState>, Status> {
        self.get_concierge_state(request).await
    }
    async fn send_concierge_message(&self, request: Request<super::proto::SendConciergeMessageRequest>) -> Result<Response<Self::SendConciergeMessageStream>, Status> {
        self.send_concierge_message(request).await
    }
    async fn rewind_chat(
        &self,
        request: Request<super::proto::RewindChatRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        self.rewind_chat(request).await
    }
    type ExecuteBlueprintStream =
        tokio_stream::wrappers::ReceiverStream<Result<ExecutionEvent, Status>>;
    type ContinueExecutionStream =
        tokio_stream::wrappers::ReceiverStream<Result<ExecutionEvent, Status>>;
    type WatchWorkspaceStream = tokio_stream::wrappers::ReceiverStream<Result<WatchEvent, Status>>;
    type WatchJobsStream = tokio_stream::wrappers::ReceiverStream<Result<JobEvent, Status>>;
    type SendChatStream =
        tokio_stream::wrappers::UnboundedReceiverStream<Result<ChatEvent, Status>>;

    async fn open_workspace(
        &self,
        request: Request<OpenWorkspaceRequest>,
    ) -> Result<Response<WorkspaceInfo>, Status> {
        self.open_workspace(request).await
    }

    async fn close_workspace(
        &self,
        request: Request<CloseWorkspaceRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.close_workspace(request).await
    }

    async fn list_workspaces(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<WorkspaceList>, Status> {
        self.list_workspaces(request).await
    }

    async fn save_blueprint(
        &self,
        request: Request<SaveBlueprintRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.save_blueprint(request).await
    }

    async fn load_blueprint(
        &self,
        request: Request<LoadBlueprintRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        self.load_blueprint(request).await
    }

    async fn execute_blueprint(
        &self,
        request: Request<ExecuteBlueprintRequest>,
    ) -> Result<Response<Self::ExecuteBlueprintStream>, Status> {
        self.execute_blueprint(request).await
    }

    async fn cancel_execution(
        &self,
        request: Request<CancelRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.cancel_execution(request).await
    }

    async fn pause_execution(
        &self,
        request: Request<PauseRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.pause_execution(request).await
    }

    async fn resume_execution(
        &self,
        request: Request<ResumeRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.resume_execution(request).await
    }

    async fn send_interrupt(
        &self,
        request: Request<InterruptRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.send_interrupt(request).await
    }

    async fn list_tools(&self, request: Request<metteur_proto::proto::RegistryRequest>) -> Result<Response<ToolList>, Status> {
        self.list_tools(request).await
    }

    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<ProtoConfig>, Status> {
        self.get_config(request).await
    }

    async fn set_config(
        &self,
        request: Request<SetConfigRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.set_config(request).await
    }

    async fn create_snapshot(
        &self,
        request: Request<CreateSnapshotRequest>,
    ) -> Result<Response<SnapshotInfo>, Status> {
        self.create_snapshot(request).await
    }

    async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<SnapshotList>, Status> {
        self.list_snapshots(request).await
    }

    async fn rollback(&self, request: Request<RollbackRequest>) -> Result<Response<Empty>, Status> {
        self.rollback(request).await
    }

    async fn list_executions(
        &self,
        request: Request<ListExecutionsRequest>,
    ) -> Result<Response<ExecutionList>, Status> {
        self.list_executions(request).await
    }

    async fn continue_execution(
        &self,
        request: Request<ContinueExecutionRequest>,
    ) -> Result<Response<Self::ContinueExecutionStream>, Status> {
        self.continue_execution(request).await
    }

    async fn get_execution_tree(
        &self,
        request: Request<GetExecutionTreeRequest>,
    ) -> Result<Response<ExecutionTree>, Status> {
        self.get_execution_tree(request).await
    }

    async fn get_file_history(
        &self,
        request: Request<GetFileHistoryRequest>,
    ) -> Result<Response<FileHistory>, Status> {
        self.get_file_history(request).await
    }

    async fn list_audit_log(
        &self,
        request: Request<ListAuditLogRequest>,
    ) -> Result<Response<AuditLogList>, Status> {
        self.list_audit_log(request).await
    }

    async fn respond_approval(
        &self,
        request: Request<ApprovalDecisionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.respond_approval(request).await
    }

    async fn list_node_kinds(
        &self,
        request: Request<metteur_proto::proto::RegistryRequest>,
    ) -> Result<Response<NodeKindList>, Status> {
        self.list_node_kinds(request).await
    }

    async fn save_function(
        &self,
        request: Request<SaveFunctionRequest>,
    ) -> Result<Response<SaveFunctionResponse>, Status> {
        self.save_function(request).await
    }

    async fn list_functions(
        &self,
        request: Request<ListFunctionsRequest>,
    ) -> Result<Response<FunctionList>, Status> {
        self.list_functions(request).await
    }

    async fn load_function(
        &self,
        request: Request<LoadFunctionRequest>,
    ) -> Result<Response<LoadFunctionResponse>, Status> {
        self.load_function(request).await
    }

    async fn delete_function(
        &self,
        request: Request<DeleteFunctionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.delete_function(request).await
    }

    async fn compile_dsl(
        &self,
        request: Request<CompileDslRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        self.compile_dsl(request).await
    }

    async fn decompile_blueprint(
        &self,
        request: Request<DecompileBlueprintRequest>,
    ) -> Result<Response<DecompileDslResponse>, Status> {
        self.decompile_blueprint(request).await
    }

    async fn get_blackboard(
        &self,
        request: Request<super::proto::GetBlackboardRequest>,
    ) -> Result<Response<super::proto::BlackboardProjection>, Status> {
        self.get_blackboard(request).await
    }

    async fn get_execution_usage(
        &self,
        request: Request<GetExecutionUsageRequest>,
    ) -> Result<Response<UsageSummary>, Status> {
        self.get_execution_usage(request).await
    }

    async fn list_mcp_servers(
        &self,
        request: Request<metteur_proto::proto::RegistryRequest>,
    ) -> Result<Response<McpServerList>, Status> {
        self.list_mcp_servers(request).await
    }

    async fn install_addon(
        &self,
        request: Request<InstallAddonRequest>,
    ) -> Result<Response<AddonInfo>, Status> {
        self.install_addon(request).await
    }

    async fn list_addons(
        &self,
        request: Request<ListAddonsRequest>,
    ) -> Result<Response<AddonList>, Status> {
        self.list_addons(request).await
    }

    async fn uninstall_addon(
        &self,
        request: Request<UninstallAddonRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.uninstall_addon(request).await
    }

    async fn set_addon_enabled(
        &self,
        request: Request<SetAddonEnabledRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.set_addon_enabled(request).await
    }

    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<FileList>, Status> {
        self.list_files(request).await
    }

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<ReadFileResponse>, Status> {
        self.read_file(request).await
    }

    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.write_file(request).await
    }

    async fn stat_file(
        &self,
        request: Request<StatFileRequest>,
    ) -> Result<Response<FileInfo>, Status> {
        self.stat_file(request).await
    }

    async fn create_dir(
        &self,
        request: Request<CreateDirRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.create_dir(request).await
    }

    async fn remove_file(
        &self,
        request: Request<RemoveFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.remove_file(request).await
    }

    async fn rename_file(
        &self,
        request: Request<RenameFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.rename_file(request).await
    }

    async fn reveal_in_explorer(
        &self,
        request: Request<RevealInExplorerRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.reveal_in_explorer(request).await
    }

    async fn watch_workspace(
        &self,
        request: Request<WatchWorkspaceRequest>,
    ) -> Result<Response<Self::WatchWorkspaceStream>, Status> {
        self.watch_workspace(request).await
    }

    async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<Response<JobList>, Status> {
        self.list_jobs(request).await
    }

    async fn watch_jobs(
        &self,
        request: Request<WatchJobsRequest>,
    ) -> Result<Response<Self::WatchJobsStream>, Status> {
        self.watch_jobs(request).await
    }

    async fn kill_job(
        &self,
        request: Request<KillJobRequest>,
    ) -> Result<Response<KillJobResponse>, Status> {
        self.kill_job(request).await
    }

    async fn get_file_at_snapshot(
        &self,
        request: Request<GetFileAtSnapshotRequest>,
    ) -> Result<Response<GetFileAtSnapshotResponse>, Status> {
        self.get_file_at_snapshot(request).await
    }

    async fn send_chat(
        &self,
        request: Request<SendChatRequest>,
    ) -> Result<Response<Self::SendChatStream>, Status> {
        self.send_chat(request).await
    }

    async fn abort_chat(
        &self,
        request: Request<AbortChatRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.abort_chat(request).await
    }

    async fn list_chat_sessions(
        &self,
        request: Request<ListChatSessionsRequest>,
    ) -> Result<Response<ChatSessionList>, Status> {
        self.list_chat_sessions(request).await
    }

    async fn get_chat_session(
        &self,
        request: Request<GetChatSessionRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        self.get_chat_session(request).await
    }

    async fn delete_chat_session(
        &self,
        request: Request<DeleteChatSessionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.delete_chat_session(request).await
    }
}
