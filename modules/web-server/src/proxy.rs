//! A `Daemon` service implementation that forwards every RPC to the real
//! daemon over gRPC, so the generated client can be re-exposed as grpc-web.

use metteur_proto::proto::daemon_client::DaemonClient;
use metteur_proto::proto::daemon_server::Daemon;
use metteur_proto::proto::{
    AbortChatRequest, AddonInfo, AddonList, ApprovalDecisionRequest, AuditLogList, Blueprint,
    CancelRequest, ChatEvent, ChatSessionList, CloseWorkspaceRequest, CompileDslRequest, Config,
    ContinueExecutionRequest, CreateDirRequest, CreateSnapshotRequest, DecompileBlueprintRequest,
    DecompileDslResponse, DeleteChatSessionRequest, DeleteFunctionRequest, Empty,
    ExecuteBlueprintRequest, ExecutionEvent, ExecutionList, ExecutionTree, FileHistory, FileInfo,
    FileList, FunctionList, GetChatSessionRequest, GetChatSessionResponse, GetConfigRequest,
    GetExecutionTreeRequest, GetExecutionUsageRequest, GetFileAtSnapshotRequest,
    GetFileAtSnapshotResponse, GetFileHistoryRequest, InstallAddonRequest, InterruptRequest,
    JobEvent, JobList, KillJobRequest, KillJobResponse, ListAddonsRequest, ListAuditLogRequest,
    ListChatSessionsRequest, ListExecutionsRequest, ListFilesRequest, ListFunctionsRequest,
    ListJobsRequest, ListSnapshotsRequest, LoadBlueprintRequest, LoadFunctionRequest,
    LoadFunctionResponse, McpServerList, NodeKindList, OpenWorkspaceRequest, PauseRequest,
    ReadFileRequest, ReadFileResponse, RemoveFileRequest, RenameFileRequest, ResumeRequest,
    RevealInExplorerRequest, RollbackRequest, SaveBlueprintRequest, SaveFunctionRequest,
    SaveFunctionResponse, SendChatRequest, SetAddonEnabledRequest, SetConfigRequest, SnapshotInfo,
    SnapshotList, StatFileRequest, ToolList, UninstallAddonRequest, UsageSummary, WatchEvent,
    WatchJobsRequest, WatchWorkspaceRequest, WorkspaceInfo, WorkspaceList, WriteFileRequest,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

/// A forwarding proxy exposing a connected daemon as a local gRPC service.
#[derive(Clone)]
pub struct ForwardService {
    client: DaemonClient<Channel>,
}

impl ForwardService {
    /// Creates a proxy backed by a connected daemon client.
    pub fn new(client: DaemonClient<Channel>) -> Self {
        Self {
            client,
        }
    }
}

/// Forwards an unbounded number of messages from a streaming response into a
/// channel consumed by the caller's response stream.
fn pump_stream<T: Send + 'static>(
    response: Response<tonic::Streaming<T>>,
) -> ReceiverStream<Result<T, Status>> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        let mut stream = response.into_inner();
        while let Some(item) = stream.message().await.transpose() {
            if tx.send(item).await.is_err() {
                break;
            }
        }
    });
    ReceiverStream::new(rx)
}

#[tonic::async_trait]
impl Daemon for ForwardService {
    type SendConciergeMessageStream = ReceiverStream<Result<metteur_proto::proto::ConciergeEvent, Status>>;
    async fn list_oversight_reports(&self, request: Request<metteur_proto::proto::OversightReportsRequest>) -> Result<Response<metteur_proto::proto::OversightReports>, Status> { self.client.clone().list_oversight_reports(request).await }
    async fn get_concierge_state(&self, request: Request<metteur_proto::proto::ConciergeStateRequest>) -> Result<Response<metteur_proto::proto::ConciergeState>, Status> {
        self.client.clone().get_concierge_state(request).await
    }
    async fn send_concierge_message(&self, request: Request<metteur_proto::proto::SendConciergeMessageRequest>) -> Result<Response<Self::SendConciergeMessageStream>, Status> {
        Ok(Response::new(pump_stream(self.client.clone().send_concierge_message(request).await?)))
    }
    async fn rewind_chat(
        &self,
        request: Request<metteur_proto::proto::RewindChatRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        self.client.clone().rewind_chat(request).await
    }
    // Workspace management.
    async fn open_workspace(
        &self,
        request: Request<OpenWorkspaceRequest>,
    ) -> Result<Response<WorkspaceInfo>, Status> {
        self.client.clone().open_workspace(request).await
    }

    async fn close_workspace(
        &self,
        request: Request<CloseWorkspaceRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().close_workspace(request).await
    }

    async fn list_workspaces(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<WorkspaceList>, Status> {
        self.client.clone().list_workspaces(request).await
    }

    // Blueprint management.
    async fn save_blueprint(
        &self,
        request: Request<SaveBlueprintRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().save_blueprint(request).await
    }

    async fn load_blueprint(
        &self,
        request: Request<LoadBlueprintRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        self.client.clone().load_blueprint(request).await
    }

    type ExecuteBlueprintStream = ReceiverStream<Result<ExecutionEvent, Status>>;

    async fn execute_blueprint(
        &self,
        request: Request<ExecuteBlueprintRequest>,
    ) -> Result<Response<Self::ExecuteBlueprintStream>, Status> {
        let response = self.client.clone().execute_blueprint(request).await?;
        Ok(Response::new(pump_stream(response)))
    }

    // Execution control.
    async fn cancel_execution(
        &self,
        request: Request<CancelRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().cancel_execution(request).await
    }

    async fn pause_execution(
        &self,
        request: Request<PauseRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().pause_execution(request).await
    }

    async fn resume_execution(
        &self,
        request: Request<ResumeRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().resume_execution(request).await
    }

    async fn send_interrupt(
        &self,
        request: Request<InterruptRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().send_interrupt(request).await
    }

    // Registry.
    async fn list_tools(&self, request: Request<metteur_proto::proto::RegistryRequest>) -> Result<Response<ToolList>, Status> {
        self.client.clone().list_tools(request).await
    }

    // Configuration.
    async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<Config>, Status> {
        self.client.clone().get_config(request).await
    }

    async fn set_config(
        &self,
        request: Request<SetConfigRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().set_config(request).await
    }

    // Version management.
    async fn create_snapshot(
        &self,
        request: Request<CreateSnapshotRequest>,
    ) -> Result<Response<SnapshotInfo>, Status> {
        self.client.clone().create_snapshot(request).await
    }

    async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<SnapshotList>, Status> {
        self.client.clone().list_snapshots(request).await
    }

    async fn rollback(&self, request: Request<RollbackRequest>) -> Result<Response<Empty>, Status> {
        self.client.clone().rollback(request).await
    }

    async fn get_file_history(
        &self,
        request: Request<GetFileHistoryRequest>,
    ) -> Result<Response<FileHistory>, Status> {
        self.client.clone().get_file_history(request).await
    }

    // Execution state.
    async fn list_executions(
        &self,
        request: Request<ListExecutionsRequest>,
    ) -> Result<Response<ExecutionList>, Status> {
        self.client.clone().list_executions(request).await
    }

    async fn get_execution_tree(
        &self,
        request: Request<GetExecutionTreeRequest>,
    ) -> Result<Response<ExecutionTree>, Status> {
        self.client.clone().get_execution_tree(request).await
    }

    type ContinueExecutionStream = ReceiverStream<Result<ExecutionEvent, Status>>;

    async fn continue_execution(
        &self,
        request: Request<ContinueExecutionRequest>,
    ) -> Result<Response<Self::ContinueExecutionStream>, Status> {
        let response = self.client.clone().continue_execution(request).await?;
        Ok(Response::new(pump_stream(response)))
    }

    // Audit.
    async fn list_audit_log(
        &self,
        request: Request<ListAuditLogRequest>,
    ) -> Result<Response<AuditLogList>, Status> {
        self.client.clone().list_audit_log(request).await
    }

    // Sandbox.
    async fn respond_approval(
        &self,
        request: Request<ApprovalDecisionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().respond_approval(request).await
    }

    // Usage.
    async fn get_blackboard(
        &self,
        request: Request<metteur_proto::proto::GetBlackboardRequest>,
    ) -> Result<Response<metteur_proto::proto::BlackboardProjection>, Status> {
        self.client.clone().get_blackboard(request).await
    }

    async fn get_execution_usage(
        &self,
        request: Request<GetExecutionUsageRequest>,
    ) -> Result<Response<UsageSummary>, Status> {
        self.client.clone().get_execution_usage(request).await
    }

    // Registry.
    async fn list_node_kinds(
        &self,
        request: Request<metteur_proto::proto::RegistryRequest>,
    ) -> Result<Response<NodeKindList>, Status> {
        self.client.clone().list_node_kinds(request).await
    }

    // MCP.
    async fn list_mcp_servers(
        &self,
        request: Request<metteur_proto::proto::RegistryRequest>,
    ) -> Result<Response<McpServerList>, Status> {
        self.client.clone().list_mcp_servers(request).await
    }

    // Addons.
    async fn install_addon(
        &self,
        request: Request<InstallAddonRequest>,
    ) -> Result<Response<AddonInfo>, Status> {
        self.client.clone().install_addon(request).await
    }

    async fn list_addons(
        &self,
        request: Request<ListAddonsRequest>,
    ) -> Result<Response<AddonList>, Status> {
        self.client.clone().list_addons(request).await
    }

    async fn uninstall_addon(
        &self,
        request: Request<UninstallAddonRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().uninstall_addon(request).await
    }

    async fn set_addon_enabled(
        &self,
        request: Request<SetAddonEnabledRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().set_addon_enabled(request).await
    }

    // Blueprint function library.
    async fn save_function(
        &self,
        request: Request<SaveFunctionRequest>,
    ) -> Result<Response<SaveFunctionResponse>, Status> {
        self.client.clone().save_function(request).await
    }

    async fn list_functions(
        &self,
        request: Request<ListFunctionsRequest>,
    ) -> Result<Response<FunctionList>, Status> {
        self.client.clone().list_functions(request).await
    }

    async fn load_function(
        &self,
        request: Request<LoadFunctionRequest>,
    ) -> Result<Response<LoadFunctionResponse>, Status> {
        self.client.clone().load_function(request).await
    }

    async fn delete_function(
        &self,
        request: Request<DeleteFunctionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().delete_function(request).await
    }

    // Blueprint DSL.
    async fn compile_dsl(
        &self,
        request: Request<CompileDslRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        self.client.clone().compile_dsl(request).await
    }

    async fn decompile_blueprint(
        &self,
        request: Request<DecompileBlueprintRequest>,
    ) -> Result<Response<DecompileDslResponse>, Status> {
        self.client.clone().decompile_blueprint(request).await
    }

    // File operations.
    async fn list_files(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<FileList>, Status> {
        self.client.clone().list_files(request).await
    }

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<ReadFileResponse>, Status> {
        self.client.clone().read_file(request).await
    }

    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().write_file(request).await
    }

    async fn stat_file(
        &self,
        request: Request<StatFileRequest>,
    ) -> Result<Response<FileInfo>, Status> {
        self.client.clone().stat_file(request).await
    }

    async fn create_dir(
        &self,
        request: Request<CreateDirRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().create_dir(request).await
    }

    async fn remove_file(
        &self,
        request: Request<RemoveFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().remove_file(request).await
    }

    async fn rename_file(
        &self,
        request: Request<RenameFileRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().rename_file(request).await
    }

    async fn reveal_in_explorer(
        &self,
        request: Request<RevealInExplorerRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().reveal_in_explorer(request).await
    }

    type WatchWorkspaceStream = ReceiverStream<Result<WatchEvent, Status>>;

    async fn watch_workspace(
        &self,
        request: Request<WatchWorkspaceRequest>,
    ) -> Result<Response<Self::WatchWorkspaceStream>, Status> {
        let response = self.client.clone().watch_workspace(request).await?;
        Ok(Response::new(pump_stream(response)))
    }

    // Background commands (jobs).
    async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<Response<JobList>, Status> {
        self.client.clone().list_jobs(request).await
    }

    async fn kill_job(
        &self,
        request: Request<KillJobRequest>,
    ) -> Result<Response<KillJobResponse>, Status> {
        self.client.clone().kill_job(request).await
    }

    async fn get_file_at_snapshot(
        &self,
        request: Request<GetFileAtSnapshotRequest>,
    ) -> Result<Response<GetFileAtSnapshotResponse>, Status> {
        self.client.clone().get_file_at_snapshot(request).await
    }

    type WatchJobsStream = ReceiverStream<Result<JobEvent, Status>>;

    async fn watch_jobs(
        &self,
        request: Request<WatchJobsRequest>,
    ) -> Result<Response<Self::WatchJobsStream>, Status> {
        let response = self.client.clone().watch_jobs(request).await?;
        Ok(Response::new(pump_stream(response)))
    }

    // ReAct chat.
    type SendChatStream = ReceiverStream<Result<ChatEvent, Status>>;

    async fn send_chat(
        &self,
        request: Request<SendChatRequest>,
    ) -> Result<Response<Self::SendChatStream>, Status> {
        let response = self.client.clone().send_chat(request).await?;
        Ok(Response::new(pump_stream(response)))
    }

    async fn abort_chat(
        &self,
        request: Request<AbortChatRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().abort_chat(request).await
    }

    async fn list_chat_sessions(
        &self,
        request: Request<ListChatSessionsRequest>,
    ) -> Result<Response<ChatSessionList>, Status> {
        self.client.clone().list_chat_sessions(request).await
    }

    async fn get_chat_session(
        &self,
        request: Request<GetChatSessionRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        self.client.clone().get_chat_session(request).await
    }

    async fn delete_chat_session(
        &self,
        request: Request<DeleteChatSessionRequest>,
    ) -> Result<Response<Empty>, Status> {
        self.client.clone().delete_chat_session(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_proto::proto::ChatSessionInfo;
    use metteur_proto::proto::daemon_server::DaemonServer;
    use std::convert::Infallible;
    use tokio_stream::wrappers::ReceiverStream;
    use tonic::transport::Server;
    use tower::Service;

    /// A canned backend used to verify that the proxy forwards calls and
    /// streams without altering them. Only the exercised methods are real.
    #[derive(Clone, Default)]
    struct TestBackend;

    #[tonic::async_trait]
    impl Daemon for TestBackend {
        async fn list_oversight_reports(&self, request: Request<metteur_proto::proto::OversightReportsRequest>) -> Result<Response<metteur_proto::proto::OversightReports>, Status> { let r=request.into_inner(); if r.workspace_path=="denied" {return Err(Status::permission_denied("denied"));} Ok(Response::new(metteur_proto::proto::OversightReports{reports_json:r.run_id})) }

        type SendConciergeMessageStream = ReceiverStream<Result<metteur_proto::proto::ConciergeEvent, Status>>;
        async fn get_concierge_state(&self, request: Request<metteur_proto::proto::ConciergeStateRequest>) -> Result<Response<metteur_proto::proto::ConciergeState>, Status> {
            let r=request.into_inner();
            if r.workspace_path=="denied" {return Err(Status::permission_denied("denied"));}
            Ok(Response::new(metteur_proto::proto::ConciergeState{state_json:r.run_id}))
        }
        async fn send_concierge_message(&self, request: Request<metteur_proto::proto::SendConciergeMessageRequest>) -> Result<Response<Self::SendConciergeMessageStream>, Status> {
            let r=request.into_inner();
            if r.workspace_path=="denied" {return Err(Status::permission_denied("denied"));}
            let (tx,rx)=tokio::sync::mpsc::channel(2);
            tx.send(Ok(metteur_proto::proto::ConciergeEvent{run_id:r.run_id,message_id:r.message_id,kind:"received".into(),detail_json:r.message})).await.unwrap();
            Ok(Response::new(ReceiverStream::new(rx)))
        }
        async fn rewind_chat(
            &self,
            request: Request<metteur_proto::proto::RewindChatRequest>,
        ) -> Result<Response<GetChatSessionResponse>, Status> {
            let request = request.into_inner();
            assert_eq!(request.workspace_path, "C:/ws");
            assert_eq!(request.snapshot_id, "checkpoint-1");
            Ok(Response::new(GetChatSessionResponse {
                session_id: request.session_id,
                history_json: "[]".into(),
                transcript_json: "[]".into(),
                todos_json: "[]".into(),
                ..Default::default()
            }))
        }
        type ExecuteBlueprintStream = ReceiverStream<Result<ExecutionEvent, Status>>;
        type ContinueExecutionStream = ReceiverStream<Result<ExecutionEvent, Status>>;
        type SendChatStream = ReceiverStream<Result<ChatEvent, Status>>;

        async fn list_workspaces(
            &self,
            _request: Request<Empty>,
        ) -> Result<Response<WorkspaceList>, Status> {
            Ok(Response::new(WorkspaceList {
                workspaces: vec![WorkspaceInfo {
                    path: "C:/demo".to_string(),
                    locked: false,
                }],
            }))
        }

        async fn reveal_in_explorer(
            &self,
            _request: Request<RevealInExplorerRequest>,
        ) -> Result<Response<Empty>, Status> {
            Ok(Response::new(Empty {}))
        }

        async fn execute_blueprint(
            &self,
            _request: Request<ExecuteBlueprintRequest>,
        ) -> Result<Response<Self::ExecuteBlueprintStream>, Status> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tx.try_send(Ok(ExecutionEvent {
                node_id: "n1".to_string(),
                kind: "started".to_string(),
                message: String::new(),
                detail_json: String::new(),
            }))
            .unwrap();
            tx.try_send(Ok(ExecutionEvent {
                node_id: "n1".to_string(),
                kind: "finished".to_string(),
                message: String::new(),
                detail_json: String::new(),
            }))
            .unwrap();
            Ok(Response::new(ReceiverStream::new(rx)))
        }

        // The remaining methods are unreachable in this test.
        async fn open_workspace(
            &self,
            _: Request<OpenWorkspaceRequest>,
        ) -> Result<Response<WorkspaceInfo>, Status> {
            Err(Status::unimplemented("open_workspace"))
        }
        async fn close_workspace(
            &self,
            _: Request<CloseWorkspaceRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("close_workspace"))
        }
        async fn save_blueprint(
            &self,
            _: Request<SaveBlueprintRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("save_blueprint"))
        }
        async fn load_blueprint(
            &self,
            _: Request<LoadBlueprintRequest>,
        ) -> Result<Response<Blueprint>, Status> {
            Err(Status::unimplemented("load_blueprint"))
        }
        async fn cancel_execution(
            &self,
            _: Request<CancelRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("cancel_execution"))
        }
        async fn pause_execution(
            &self,
            _: Request<PauseRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("pause_execution"))
        }
        async fn resume_execution(
            &self,
            _: Request<ResumeRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("resume_execution"))
        }
        async fn send_interrupt(
            &self,
            _: Request<InterruptRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("send_interrupt"))
        }
        async fn list_tools(&self, _: Request<metteur_proto::proto::RegistryRequest>) -> Result<Response<ToolList>, Status> {
            Err(Status::unimplemented("list_tools"))
        }
        async fn get_config(
            &self,
            _: Request<GetConfigRequest>,
        ) -> Result<Response<Config>, Status> {
            Err(Status::unimplemented("get_config"))
        }
        async fn set_config(
            &self,
            _: Request<SetConfigRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("set_config"))
        }
        async fn create_snapshot(
            &self,
            _: Request<CreateSnapshotRequest>,
        ) -> Result<Response<SnapshotInfo>, Status> {
            Err(Status::unimplemented("create_snapshot"))
        }
        async fn list_snapshots(
            &self,
            _: Request<ListSnapshotsRequest>,
        ) -> Result<Response<SnapshotList>, Status> {
            Err(Status::unimplemented("list_snapshots"))
        }
        async fn rollback(&self, _: Request<RollbackRequest>) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("rollback"))
        }
        async fn get_file_history(
            &self,
            _: Request<GetFileHistoryRequest>,
        ) -> Result<Response<FileHistory>, Status> {
            Err(Status::unimplemented("get_file_history"))
        }
        async fn list_executions(
            &self,
            _: Request<ListExecutionsRequest>,
        ) -> Result<Response<ExecutionList>, Status> {
            Err(Status::unimplemented("list_executions"))
        }
        async fn get_execution_tree(
            &self,
            _: Request<GetExecutionTreeRequest>,
        ) -> Result<Response<ExecutionTree>, Status> {
            Err(Status::unimplemented("get_execution_tree"))
        }
        async fn continue_execution(
            &self,
            _: Request<ContinueExecutionRequest>,
        ) -> Result<Response<Self::ContinueExecutionStream>, Status> {
            Err(Status::unimplemented("continue_execution"))
        }
        async fn list_audit_log(
            &self,
            _: Request<ListAuditLogRequest>,
        ) -> Result<Response<AuditLogList>, Status> {
            Err(Status::unimplemented("list_audit_log"))
        }
        async fn respond_approval(
            &self,
            _: Request<ApprovalDecisionRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("respond_approval"))
        }
        async fn get_blackboard(
            &self,
            request: Request<metteur_proto::proto::GetBlackboardRequest>,
        ) -> Result<Response<metteur_proto::proto::BlackboardProjection>, Status> {
            let req = request.into_inner();
            if req.workspace_path != "allowed" {
                return Err(Status::permission_denied("query denied"));
            }
            Ok(Response::new(metteur_proto::proto::BlackboardProjection {
                projection_json: req.query_json,
            }))
        }

        async fn get_execution_usage(
            &self,
            _: Request<GetExecutionUsageRequest>,
        ) -> Result<Response<UsageSummary>, Status> {
            Err(Status::unimplemented("get_execution_usage"))
        }
        async fn list_node_kinds(
            &self,
            _: Request<metteur_proto::proto::RegistryRequest>,
        ) -> Result<Response<NodeKindList>, Status> {
            Err(Status::unimplemented("list_node_kinds"))
        }
        async fn list_mcp_servers(
            &self,
            _: Request<metteur_proto::proto::RegistryRequest>,
        ) -> Result<Response<McpServerList>, Status> {
            Err(Status::unimplemented("list_mcp_servers"))
        }
        async fn install_addon(
            &self,
            _: Request<InstallAddonRequest>,
        ) -> Result<Response<AddonInfo>, Status> {
            Err(Status::unimplemented("install_addon"))
        }
        async fn list_addons(
            &self,
            _: Request<ListAddonsRequest>,
        ) -> Result<Response<AddonList>, Status> {
            Err(Status::unimplemented("list_addons"))
        }
        async fn uninstall_addon(
            &self,
            _: Request<UninstallAddonRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("uninstall_addon"))
        }
        async fn set_addon_enabled(
            &self,
            _: Request<SetAddonEnabledRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("set_addon_enabled"))
        }
        async fn list_files(
            &self,
            _: Request<ListFilesRequest>,
        ) -> Result<Response<FileList>, Status> {
            Err(Status::unimplemented("list_files"))
        }
        async fn read_file(
            &self,
            _: Request<ReadFileRequest>,
        ) -> Result<Response<ReadFileResponse>, Status> {
            Err(Status::unimplemented("read_file"))
        }
        async fn write_file(
            &self,
            _: Request<WriteFileRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("write_file"))
        }
        async fn stat_file(
            &self,
            _: Request<StatFileRequest>,
        ) -> Result<Response<FileInfo>, Status> {
            Err(Status::unimplemented("stat_file"))
        }
        async fn create_dir(
            &self,
            _: Request<CreateDirRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("create_dir"))
        }
        async fn remove_file(
            &self,
            _: Request<RemoveFileRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("remove_file"))
        }
        async fn rename_file(
            &self,
            _: Request<RenameFileRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("rename_file"))
        }
        type WatchJobsStream = ReceiverStream<Result<JobEvent, Status>>;
        async fn list_jobs(
            &self,
            _: Request<ListJobsRequest>,
        ) -> Result<Response<JobList>, Status> {
            Err(Status::unimplemented("list_jobs"))
        }
        async fn watch_jobs(
            &self,
            _: Request<WatchJobsRequest>,
        ) -> Result<Response<Self::WatchJobsStream>, Status> {
            Err(Status::unimplemented("watch_jobs"))
        }
        async fn kill_job(
            &self,
            _: Request<KillJobRequest>,
        ) -> Result<Response<KillJobResponse>, Status> {
            Err(Status::unimplemented("kill_job"))
        }
        async fn get_file_at_snapshot(
            &self,
            _: Request<GetFileAtSnapshotRequest>,
        ) -> Result<Response<GetFileAtSnapshotResponse>, Status> {
            Err(Status::unimplemented("get_file_at_snapshot"))
        }
        type WatchWorkspaceStream = ReceiverStream<Result<WatchEvent, Status>>;
        async fn watch_workspace(
            &self,
            _: Request<WatchWorkspaceRequest>,
        ) -> Result<Response<Self::WatchWorkspaceStream>, Status> {
            Err(Status::unimplemented("watch_workspace"))
        }
        async fn save_function(
            &self,
            _: Request<SaveFunctionRequest>,
        ) -> Result<Response<SaveFunctionResponse>, Status> {
            Err(Status::unimplemented("save_function"))
        }
        async fn list_functions(
            &self,
            _: Request<ListFunctionsRequest>,
        ) -> Result<Response<FunctionList>, Status> {
            Err(Status::unimplemented("list_functions"))
        }
        async fn load_function(
            &self,
            _: Request<LoadFunctionRequest>,
        ) -> Result<Response<LoadFunctionResponse>, Status> {
            Err(Status::unimplemented("load_function"))
        }
        async fn delete_function(
            &self,
            _: Request<DeleteFunctionRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("delete_function"))
        }
        async fn compile_dsl(
            &self,
            _: Request<CompileDslRequest>,
        ) -> Result<Response<Blueprint>, Status> {
            Err(Status::unimplemented("compile_dsl"))
        }
        async fn decompile_blueprint(
            &self,
            _: Request<DecompileBlueprintRequest>,
        ) -> Result<Response<DecompileDslResponse>, Status> {
            Err(Status::unimplemented("decompile_blueprint"))
        }
        async fn send_chat(
            &self,
            _: Request<SendChatRequest>,
        ) -> Result<Response<Self::SendChatStream>, Status> {
            Err(Status::unimplemented("send_chat"))
        }
        async fn abort_chat(
            &self,
            _: Request<AbortChatRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("abort_chat"))
        }
        async fn list_chat_sessions(
            &self,
            _: Request<ListChatSessionsRequest>,
        ) -> Result<Response<ChatSessionList>, Status> {
            Ok(Response::new(ChatSessionList {
                sessions: vec![ChatSessionInfo {
                    session_id: "s1".to_string(),
                    created_at: 1,
                    updated_at: 2,
                    turns: 3,
                    title: "demo".to_string(),
                    message_count: 4,
                }],
            }))
        }
        async fn get_chat_session(
            &self,
            request: Request<GetChatSessionRequest>,
        ) -> Result<Response<GetChatSessionResponse>, Status> {
            let req = request.into_inner();
            Ok(Response::new(GetChatSessionResponse {
                session_id: req.session_id,
                created_at: 1,
                history_json: "[]".to_string(),
                todos_json: "[]".to_string(),
                transcript_json: "[]".to_string(),
            }))
        }
        async fn delete_chat_session(
            &self,
            _: Request<DeleteChatSessionRequest>,
        ) -> Result<Response<Empty>, Status> {
            Err(Status::unimplemented("delete_chat_session"))
        }
    }

    /// Serves `svc` on an ephemeral port and returns the connected client.
    async fn serve_client<S>(svc: S) -> DaemonClient<Channel>
    where
        S: Service<
                http::Request<tonic::body::Body>,
                Response = http::Response<tonic::body::Body>,
                Error = Infallible,
            > + tonic::server::NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        <S as Service<http::Request<tonic::body::Body>>>::Future: Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            Server::builder()
                .add_service(svc)
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        DaemonClient::connect(format!("http://{addr}")).await.unwrap()
    }

    #[tokio::test]
    async fn proxy_forwards_unary_and_streaming() {
        // Backend server behind the proxy.
        let backend = serve_client(DaemonServer::new(TestBackend)).await;
        let proxy = serve_client(DaemonServer::new(ForwardService::new(backend))).await;

        // Unary passthrough.
        let mut proxy = proxy;
        let list = proxy.list_workspaces(Empty {}).await.unwrap().into_inner();
        assert_eq!(list.workspaces.len(), 1);
        assert_eq!(list.workspaces[0].path, "C:/demo");

        // Streaming passthrough.
        let mut stream =
            proxy.execute_blueprint(ExecuteBlueprintRequest::default()).await.unwrap().into_inner();
        let mut kinds = Vec::new();
        while let Some(event) = stream.message().await.unwrap() {
            kinds.push(event.kind);
        }
        assert_eq!(kinds, vec!["started".to_string(), "finished".to_string()]);
    }

    #[tokio::test]
    async fn proxy_forwards_concierge_state_stream_and_denials() {
        let backend=serve_client(DaemonServer::new(TestBackend)).await;
        let mut proxy=serve_client(DaemonServer::new(ForwardService::new(backend))).await;
        let state=proxy.get_concierge_state(metteur_proto::proto::ConciergeStateRequest{workspace_path:"allowed".into(),run_id:"run".into(),conversation_id:"conversation".into()}).await.unwrap().into_inner();
        assert_eq!(state.state_json,"run");
        let mut stream=proxy.send_concierge_message(metteur_proto::proto::SendConciergeMessageRequest{workspace_path:"allowed".into(),run_id:"run".into(),message_id:"message".into(),message:"request".into(),..Default::default()}).await.unwrap().into_inner();
        let event=stream.message().await.unwrap().unwrap();assert_eq!(event.message_id,"message");assert_eq!(event.detail_json,"request");
        assert!(stream.message().await.unwrap().is_none());
        let denied=proxy.send_concierge_message(metteur_proto::proto::SendConciergeMessageRequest{workspace_path:"denied".into(),..Default::default()}).await.err().unwrap();
        assert_eq!(denied.code(),tonic::Code::PermissionDenied);
        let denied=proxy.get_concierge_state(metteur_proto::proto::ConciergeStateRequest{workspace_path:"denied".into(),..Default::default()}).await.err().unwrap();
        assert_eq!(denied.code(),tonic::Code::PermissionDenied);
    }

    #[tokio::test]
    async fn proxy_forwards_blackboard_query_and_denial() {
        let backend = serve_client(DaemonServer::new(TestBackend)).await;
        let mut proxy = serve_client(DaemonServer::new(ForwardService::new(backend))).await;
        let request = metteur_proto::proto::GetBlackboardRequest {
            workspace_path: "allowed".into(),
            run_id: "run".into(),
            query_json: "{\"entry_id\":\"attempt:1:check\"}".into(),
        };
        let result = proxy.get_blackboard(request.clone()).await.unwrap().into_inner();
        assert_eq!(result.projection_json, request.query_json);
        let denied = proxy
            .get_blackboard(metteur_proto::proto::GetBlackboardRequest {
                workspace_path: "denied".into(),
                ..request
            })
            .await
            .unwrap_err();
        assert_eq!(denied.code(), tonic::Code::PermissionDenied);
    }

    #[tokio::test]
    async fn proxy_forwards_chat_session_rpcs() {
        let backend = serve_client(DaemonServer::new(TestBackend)).await;
        let proxy = serve_client(DaemonServer::new(ForwardService::new(backend))).await;
        let mut proxy = proxy;

        let list = proxy
            .list_chat_sessions(ListChatSessionsRequest {
                workspace_path: "C:/ws".to_string(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(list.sessions.len(), 1);
        assert_eq!(list.sessions[0].session_id, "s1");
        assert_eq!(list.sessions[0].title, "demo");

        let session = proxy
            .get_chat_session(GetChatSessionRequest {
                workspace_path: "C:/ws".to_string(),
                session_id: "s1".to_string(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(session.session_id, "s1");
        assert_eq!(session.history_json, "[]");

        let restored = proxy
            .rewind_chat(metteur_proto::proto::RewindChatRequest {
                workspace_path: "C:/ws".into(),
                session_id: "s1".into(),
                snapshot_id: "checkpoint-1".into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(restored.session_id, "s1");
        assert_eq!(restored.transcript_json, "[]");

        let err = proxy
            .delete_chat_session(DeleteChatSessionRequest {
                workspace_path: "C:/ws".to_string(),
                session_id: String::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }
}
