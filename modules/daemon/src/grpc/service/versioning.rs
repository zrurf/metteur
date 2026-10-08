//! Versioning RPCs: snapshots, executions and file history.

use std::path::PathBuf;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::execution::interrupt::InterruptBus;
use crate::execution::{DbCheckpointSink, RunStatus};
use crate::observability::audit::AuditWriter;

use super::super::acl::subject_from_request;
use super::super::proto::{
    self, ContinueExecutionRequest, CreateSnapshotRequest, Empty, ExecutionEvent, ExecutionInfo,
    ExecutionList, ExecutionTree, FileHistory, FileHistoryEntry, GetExecutionTreeRequest,
    GetExecutionUsageRequest, GetFileHistoryRequest, ListExecutionsRequest, ListSnapshotsRequest,
    RollbackRequest, SnapshotInfo, SnapshotList, UsageSummary,
};
use super::*;

impl DaemonService {
    pub(crate) async fn create_snapshot(
        &self,
        request: Request<CreateSnapshotRequest>,
    ) -> Result<Response<SnapshotInfo>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let snapshot = ws
            .version_manager
            .create_snapshot_with(
                &req.description,
                if req.alias.is_empty() {
                    None
                } else {
                    Some(req.alias.as_str())
                },
            )
            .map_err(to_status)?;
        Ok(Response::new(SnapshotInfo {
            id: snapshot.id.to_string(),
            description: snapshot.description,
            created_at: (snapshot.created_at / 1000) as i64,
            alias: snapshot.alias.unwrap_or_default(),
        }))
    }

    pub(crate) async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<SnapshotList>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let snapshots = ws.version_manager.list_snapshots().map_err(to_status)?;
        let infos = snapshots
            .into_iter()
            .map(|s| SnapshotInfo {
                id: s.id.to_string(),
                description: s.description,
                created_at: (s.created_at / 1000) as i64,
                alias: s.alias.unwrap_or_default(),
            })
            .collect();
        Ok(Response::new(SnapshotList {
            snapshots: infos,
        }))
    }

    pub(crate) async fn rollback(
        &self,
        request: Request<RollbackRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let _admission = ws.activity_gate.lock().await;
        if self.state.running.read().await.contains_key(&ws.root)
            || self.state.chats.read().await.contains_key(&ws.root)
        {
            return Err(Status::failed_precondition(
                "stop the active execution/chat before restoring workspace files",
            ));
        }
        ws.reconcile_files().map_err(to_status)?;
        if !req.alias.is_empty() {
            ws.version_manager.rollback_by_alias(&req.alias).map_err(to_status)?;
        } else {
            let id = uuid::Uuid::parse_str(&req.snapshot_id)
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            ws.version_manager.rollback(id).map_err(to_status)?;
        }
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn list_executions(
        &self,
        request: Request<ListExecutionsRequest>,
    ) -> Result<Response<ExecutionList>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let checkpoints = DbCheckpointSink::list(&ws.db).map_err(to_status)?;
        let running = self.state.running.read().await;
        let live = running.get(&ws.root);
        let executions = checkpoints
            .into_iter()
            .map(|cp| {
                let current = live.filter(|r| r.run_id == cp.run_id);
                let active = current.is_some();
                let mut data = serde_json::to_value(&cp).unwrap_or_default();
                data["runtime"] = serde_json::json!({
                    "active": active,
                    "pause_requested": current.is_some_and(|r| r.pause_requested.load(std::sync::atomic::Ordering::SeqCst)),
                    "cancel_requested": current.is_some_and(|r| r.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)),
                    "pending_approval_ids": current.and_then(|r| r.approvals.as_ref()).map(|b| b.pending_ids()).unwrap_or_default(),
                });
                let status = if cp.in_flight.is_some() && !active && cp.status.resumable() {
                    "RecoveryRequired".to_string()
                } else if cp.status == RunStatus::Running && !active {
                    "Suspended".to_string()
                } else {
                    status_str(cp.status)
                };
                ExecutionInfo {
                    run_id: cp.run_id.to_string(),
                    blueprint_id: cp.blueprint_id.to_string(),
                    status,
                    started_at: cp.started_at as i64,
                    updated_at: cp.updated_at as i64,
                    executed_nodes: cp.executed.len() as i32,
                    data_json: data.to_string(),
                }
            })
            .collect();
        Ok(Response::new(ExecutionList {
            executions,
        }))
    }

    pub(crate) async fn get_execution_tree(
        &self,
        request: Request<GetExecutionTreeRequest>,
    ) -> Result<Response<ExecutionTree>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let run_id = uuid::Uuid::parse_str(&req.run_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let checkpoint = DbCheckpointSink::load(&ws.db, run_id)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("execution not found"))?;
        let mut nodes: Vec<proto::ExecTreeNode> = checkpoint
            .exec_tree
            .nodes
            .values()
            .map(|n| {
                let kind = match &n.kind {
                    crate::execution::tree::TreeNodeKind::Run => "run".to_string(),
                    crate::execution::tree::TreeNodeKind::BlueprintNode(kind) => {
                        format!("node:{kind}")
                    }
                    crate::execution::tree::TreeNodeKind::SubAgent => "subagent".to_string(),
                    crate::execution::tree::TreeNodeKind::Function(name) => {
                        format!("function:{name}")
                    }
                };
                let status = match &n.status {
                    crate::execution::tree::TreeNodeStatus::Running => "running".to_string(),
                    crate::execution::tree::TreeNodeStatus::Done => "done".to_string(),
                    crate::execution::tree::TreeNodeStatus::Failed(reason) => {
                        format!("failed:{reason}")
                    }
                };
                proto::ExecTreeNode {
                    id: n.id.clone(),
                    kind,
                    label: n.label.clone(),
                    parent: n.parent.clone().unwrap_or_default(),
                    children: n.children.clone(),
                    status,
                    tokens: n.tokens,
                    started_at: n.started_at_ms as i64,
                    finished_at: n.finished_at_ms.unwrap_or(0) as i64,
                }
            })
            .collect();
        nodes.sort_by_key(|n| n.started_at);
        Ok(Response::new(ExecutionTree {
            nodes,
            roots: checkpoint.exec_tree.roots.clone(),
        }))
    }

    pub(crate) async fn continue_execution(
        &self,
        request: Request<ContinueExecutionRequest>,
    ) -> Result<
        Response<tokio_stream::wrappers::ReceiverStream<Result<ExecutionEvent, Status>>>,
        Status,
    > {
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".to_string());
        let req = request.into_inner();
        let ws_path = PathBuf::from(&req.workspace_path);
        let ws = self
            .state
            .workspaces
            .get(&ws_path)
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let run_id = uuid::Uuid::parse_str(&req.run_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let checkpoint = DbCheckpointSink::load(&ws.db, run_id)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("execution not found"))?;
        if !checkpoint.status.resumable() {
            return Err(Status::failed_precondition("execution is not resumable"));
        }
        crate::replan::application::ensure_resolved(&ws.db).map_err(to_status)?;
        let blueprint = crate::storage::blueprint_files::load(
            &ws.db,
            &ws.version_manager,
            checkpoint.blueprint_id,
        )
        .map_err(to_status)?;
        let bound = crate::storage::blueprint_files::binding(&ws.db, checkpoint.blueprint_id)
            .map_err(to_status)?;
        let identity = |v: &Option<crate::storage::versioning::VersionRef>| {
            v.as_ref().map(|v| (v.blueprint_uri.clone(), v.blob_hash.clone()))
        };
        if identity(&bound) != identity(&checkpoint.blueprint_version) {
            return Err(Status::failed_precondition(
                "blueprint differs from the run checkpoint; explicitly start a new run",
            ));
        }
        let ws_key = ws.root().to_path_buf();

        let interrupt_bus = InterruptBus::new();
        let pause_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let registry=self.state.registry_for(Some(ws.root()),true).await?;
        checkpoint.ensure_addons(&registry).map_err(to_status)?;
        let addon_fragments = registry.addon_fragments.values().flatten().cloned().collect();

        let stream = spawn_execution(
            &self.state,
            ws_key,
            ws.db.clone(),
            ws.config.clone(),
            ws.root().to_path_buf(),
            registry,
            self.state.llm_factory.clone(),
            AuditWriter::new(ws.db.clone()),
            subject,
            Arc::new(DbCheckpointSink::new(ws.db.clone(), run_id)),
            blueprint,
            Some(checkpoint),
            interrupt_bus,
            pause_flag,
            cancel_flag,
            ws.lsp(),
            addon_fragments,
            Some(ws.version_manager.clone()),
            ws.jobs(),
        )
        .await?;

        Ok(Response::new(stream))
    }

    /// Returns a file's content as recorded by a snapshot.
    ///
    /// An empty `snapshot_id` reads the most recent snapshot, which is what a
    /// "compare with the last snapshot" view asks for.
    pub(crate) async fn get_file_at_snapshot(
        &self,
        request: Request<GetFileAtSnapshotRequest>,
    ) -> Result<Response<GetFileAtSnapshotResponse>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let manager = &ws.version_manager;
        let snapshot_id = match req.snapshot_id.trim() {
            "" => manager.latest_snapshot_id().map_err(to_status)?,
            raw => Some(
                uuid::Uuid::parse_str(raw).map_err(|e| Status::invalid_argument(e.to_string()))?,
            ),
        };
        let Some(snapshot_id) = snapshot_id else {
            // No snapshots yet: everything is "added since", with no baseline.
            return Ok(Response::new(GetFileAtSnapshotResponse {
                found: false,
                content: String::new(),
                snapshot_id: String::new(),
            }));
        };
        let found = manager.file_at_snapshot(snapshot_id, &req.path).map_err(to_status)?;
        Ok(Response::new(match found {
            Some((id, content)) => GetFileAtSnapshotResponse {
                found: true,
                content,
                snapshot_id: id.to_string(),
            },
            None => GetFileAtSnapshotResponse {
                found: false,
                content: String::new(),
                snapshot_id: snapshot_id.to_string(),
            },
        }))
    }

    pub(crate) async fn get_file_history(
        &self,
        request: Request<GetFileHistoryRequest>,
    ) -> Result<Response<FileHistory>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let history = ws.version_manager.file_history(&req.path).map_err(to_status)?;
        let entries = history
            .into_iter()
            .map(|entry| FileHistoryEntry {
                snapshot_id: entry.snapshot.id.to_string(),
                description: entry.snapshot.description,
                created_at: (entry.snapshot.created_at / 1000) as i64,
                status: match entry.status {
                    crate::storage::versioning::FileChangeStatus::Added => "Added".to_string(),
                    crate::storage::versioning::FileChangeStatus::Modified => {
                        "Modified".to_string()
                    }
                    crate::storage::versioning::FileChangeStatus::Deleted => "Deleted".to_string(),
                    crate::storage::versioning::FileChangeStatus::Unchanged => {
                        "Unchanged".to_string()
                    }
                },
                content_hash: entry.hash.unwrap_or_default(),
            })
            .collect();
        Ok(Response::new(FileHistory {
            entries,
        }))
    }

    pub(crate) async fn get_execution_usage(
        &self,
        request: Request<GetExecutionUsageRequest>,
    ) -> Result<Response<UsageSummary>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let config = ws.config.read().await;
        let summary = crate::llm::billing::run_usage(&ws.db, &config.billing, &req.run_id)
            .map_err(to_status)?;
        let oversight_json = match uuid::Uuid::parse_str(&req.run_id) {
            Ok(run) if crate::execution::DbCheckpointSink::load(&ws.db, run).map_err(to_status)?.is_some() => {
                let settings = metteur_shared::config::oversight::OversightConfig::from_config(&config)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;
                serde_json::to_string(&crate::oversight::budget::summary(&ws.db, run, &settings).map_err(to_status)?)
                    .map_err(|e| Status::internal(e.to_string()))?
            }
            _ => String::new(),
        };
        drop(config);
        Ok(Response::new(UsageSummary {
            oversight_json,
            currency: summary.currency,
            total_cost_micros: summary.total_cost_micros as u64,
            models: summary
                .models
                .into_iter()
                .map(|m| proto::ModelUsage {
                    model: m.model,
                    tokens_complete: m.tokens_complete,
                    cache_complete: m.cache_complete,
                    cost_complete: m.cost_complete,
                    calls: m.calls,
                    input_tokens: m.input_tokens,
                    output_tokens: m.output_tokens,
                    reasoning_tokens: m.reasoning_tokens,
                    cost_micros: m.cost_micros,
                    cached_input_tokens: m.cached_input_tokens,
                    cache_write_input_tokens: m.cache_write_input_tokens,
                })
                .collect(),
        }))
    }
}
