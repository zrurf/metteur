//! Blueprint RPCs: save/load/execute with control, the function library and
//! the DSL compile/decompile endpoints.

use std::path::PathBuf;
use std::sync::Arc;

use metteur_shared::model::function::{FnPin, FunctionEntry, FunctionSource};
use tonic::{Request, Response, Status};

use crate::error::DaemonError;
use crate::execution::DbCheckpointSink;
use crate::execution::interrupt::{Interrupt, InterruptBus, InterruptPriority};
use crate::observability::audit::AuditWriter;

use super::super::acl::subject_from_request;
use super::super::proto::{
    self, Blueprint, CancelRequest, CompileDslRequest, DecompileBlueprintRequest,
    DecompileDslResponse, DeleteFunctionRequest, Empty, ExecuteBlueprintRequest, ExecutionEvent,
    FnPin as ProtoFnPin, FunctionInfo as ProtoFunctionInfo, FunctionList, InterruptRequest,
    ListFunctionsRequest, LoadBlueprintRequest, LoadFunctionRequest, LoadFunctionResponse,
    PauseRequest, ResumeRequest, SaveBlueprintRequest, SaveFunctionRequest, SaveFunctionResponse,
};
use super::*;

impl DaemonService {
    pub(super) async fn ensure_blueprint_idle(
        &self,
        root: &std::path::Path,
        id: uuid::Uuid,
    ) -> Result<(), Status> {
        if self.state.running.read().await.get(root).is_some_and(|run| run.blueprint_id == id) {
            return Err(Status::failed_precondition(
                "blueprint is running; use an approved replan or stop the run before saving",
            ));
        }
        Ok(())
    }

    pub(crate) async fn save_blueprint(
        &self,
        request: Request<SaveBlueprintRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let proto_blueprint =
            req.blueprint.ok_or_else(|| Status::invalid_argument("blueprint is required"))?;
        let blueprint = proto_to_blueprint(&proto_blueprint).map_err(to_status)?;
        let registry=self.state.registry_for(Some(ws.root()),false).await?;
        ensure_valid(&blueprint, &registry)?;
        let _admission = ws.activity_gate.lock().await;
        self.ensure_blueprint_idle(&ws.root, blueprint.id).await?;
        crate::replan::application::ensure_resolved(&ws.db).map_err(to_status)?;
        let uri = if req.file_path.is_empty() {
            crate::storage::blueprint_files::binding(&ws.db, blueprint.id)
                .map_err(to_status)?
                .ok_or_else(|| {
                    Status::failed_precondition(
                        "choose an authoritative blueprint file path before saving",
                    )
                })?
                .blueprint_uri
        } else {
            req.file_path
        };
        let bytes = if req.file_json.is_empty() {
            crate::storage::blueprint_files::encode_native(&blueprint).map_err(to_status)?
        } else {
            req.file_json.into_bytes()
        };
        crate::storage::blueprint_files::save(
            &ws.db,
            &ws.version_manager,
            &blueprint,
            &uri,
            &bytes,
            None,
        )
        .map_err(to_status)?;
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn load_blueprint(
        &self,
        request: Request<LoadBlueprintRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let id = uuid::Uuid::parse_str(&req.blueprint_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let blueprint = crate::storage::blueprint_files::load(&ws.db, &ws.version_manager, id)
            .map_err(to_status)?;
        let registry=self.state.registry_for(Some(ws.root()),false).await?;
        ensure_valid(&blueprint, &registry)?;
        Ok(Response::new(blueprint_to_proto(&blueprint)))
    }

    pub(crate) async fn execute_blueprint(
        &self,
        request: Request<ExecuteBlueprintRequest>,
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
        if self.state.running.read().await.contains_key(&ws.root)
            || self.state.chats.read().await.contains_key(&ws.root)
        {
            return Err(Status::failed_precondition(
                "workspace already has an active execution or chat",
            ));
        }
        let registry=self.state.registry_for(Some(ws.root()),true).await?;
        // Inline content is a consistency assertion, never a hidden persistence path.
        let inline = if req.blueprint_json.trim().is_empty() {
            None
        } else {
            let parsed = crate::storage::blueprint_files::decode(req.blueprint_json.as_bytes())
                .map_err(|e| Status::invalid_argument(format!("invalid blueprint: {e}")))?;
            ensure_valid(&parsed, &registry)?;
            Some(parsed)
        };
        let id = if req.blueprint_id.is_empty() {
            inline
                .as_ref()
                .map(|b| b.id)
                .ok_or_else(|| Status::invalid_argument("blueprint id is required"))?
        } else {
            uuid::Uuid::parse_str(&req.blueprint_id)
                .map_err(|e| Status::invalid_argument(e.to_string()))?
        };
        if inline.as_ref().is_some_and(|b| b.id != id) {
            return Err(Status::invalid_argument("inline blueprint id does not match request"));
        }
        let blueprint = crate::storage::blueprint_files::load(&ws.db, &ws.version_manager, id)
            .map_err(|error| match error {
                DaemonError::NotFound(_) if inline.is_some() => Status::failed_precondition(
                    "save the blueprint to an explicit file before running it",
                ),
                other => to_status(other),
            })?;
        if inline.as_ref().is_some_and(|graph| graph != &blueprint) {
            return Err(Status::failed_precondition(
                "canvas differs from the saved blueprint; save successfully before running",
            ));
        }
        // A blueprint saved before validation existed, or edited directly in
        // the database, is re-checked here so execution never runs a graph that
        // would fail halfway through.
        ensure_valid(&blueprint, &registry)?;
        let run_id = uuid::Uuid::new_v4();
        let ws_key = ws.root().to_path_buf();

        let interrupt_bus = InterruptBus::new();
        let pause_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
            None,
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

    pub(crate) async fn cancel_execution(
        &self,
        request: Request<CancelRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws_path = PathBuf::from(&req.workspace_path);
        let ws = self
            .state
            .workspaces
            .get(&ws_path)
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let running = self.state.running.read().await;
        let entry =
            running.get(&ws_key).ok_or_else(|| Status::not_found("no running execution"))?;
        if !req.run_id.is_empty() && req.run_id != entry.run_id.to_string() {
            return Err(Status::failed_precondition(
                "execution changed; refresh before controlling it",
            ));
        }
        let recorded = crate::oversight::recovery::mark_direct_cancel(&ws.db, entry.run_id);
        // Stop remains effective even if persisting its recovery receipt fails.
        entry.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(broker) = &entry.approvals {
            broker.close();
        }
        recorded.map_err(to_status)?;
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn pause_execution(
        &self,
        request: Request<PauseRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws_path = PathBuf::from(&req.workspace_path);
        let ws = self
            .state
            .workspaces
            .get(&ws_path)
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let running = self.state.running.read().await;
        let entry =
            running.get(&ws_key).ok_or_else(|| Status::not_found("no running execution"))?;
        if !req.run_id.is_empty() && req.run_id != entry.run_id.to_string() {
            return Err(Status::failed_precondition(
                "execution changed; refresh before controlling it",
            ));
        }
        entry.pause_requested.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn resume_execution(
        &self,
        request: Request<ResumeRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws_path = PathBuf::from(&req.workspace_path);
        let ws = self
            .state
            .workspaces
            .get(&ws_path)
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let running = self.state.running.read().await;
        let entry =
            running.get(&ws_key).ok_or_else(|| Status::not_found("no running execution"))?;
        if !req.run_id.is_empty() && req.run_id != entry.run_id.to_string() {
            return Err(Status::failed_precondition(
                "execution changed; refresh before controlling it",
            ));
        }
        entry.pause_requested.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn send_interrupt(
        &self,
        request: Request<InterruptRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws_path = PathBuf::from(&req.workspace_path);
        let ws = self
            .state
            .workspaces
            .get(&ws_path)
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let priority = match req.priority.as_str() {
            "Urgent" => InterruptPriority::Urgent,
            "Emergency" => InterruptPriority::Emergency,
            _ => InterruptPriority::Normal,
        };
        let running = self.state.running.read().await;
        if let Some(entry) = running.get(&ws_key) {
            if priority == InterruptPriority::Normal {
                return Err(Status::failed_precondition(
                    "Normal blueprint messages use SendConciergeMessage; configure oversight.concierge_model",
                ));
            }
            if let Some(bus) = &entry.interrupt_bus {
                bus.send(Interrupt {
                    priority,
                    message: req.message,
                });
            }
            return Ok(Response::new(Empty {}));
        }
        drop(running);
        // A ReAct chat runs in its own slot: without this fallback a message
        // typed while the agent works could not reach it.
        let chats = self.state.chats.read().await;
        let entry =
            chats.get(&ws_key).ok_or_else(|| Status::not_found("no running execution or chat"))?;
        if let Some(bus) = &entry.interrupt_bus {
            bus.send(Interrupt {
                priority,
                message: req.message,
            });
        }
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn save_function(
        &self,
        request: Request<SaveFunctionRequest>,
    ) -> Result<Response<SaveFunctionResponse>, Status> {
        let req = request.into_inner();
        let info = req
            .info
            .ok_or_else(|| Status::invalid_argument("info is required"))?;
        let root = optional_workspace(&req.workspace_path)?;
        let registry = self.state.registry_for(root.as_deref(), true).await?;
        if registry
            .function(&info.name)
            .is_some_and(|f| matches!(f.source, FunctionSource::Addon | FunctionSource::Builtin))
        {
            return Err(Status::permission_denied(
                "Read-only function; import an explicitly named editable copy",
            ));
        }
        let importing = !req.import_from.is_empty();
        if importing
            && (registry.node_executor(&info.name).is_some() || registry.tool(&info.name).is_some())
        {
            return Err(Status::already_exists(
                "Function copy name conflicts with a node or tool",
            ));
        }
        let mut entry = if importing {
            if root.is_none()
                || req.file_path.is_empty()
                || !crate::registry::tool::is_valid_tool_name(&info.name)
                || registry.function(&info.name).is_some()
            {
                return Err(Status::invalid_argument(
                    "Import requires an open workspace, new PascalCase name and explicit new file path",
                ));
            }
            let source = registry
                .function(&req.import_from)
                .filter(|f| f.source == FunctionSource::Addon)
                .ok_or_else(|| Status::not_found("Addon function unavailable"))?;
            let expected: serde_json::Value =
                serde_json::from_str(&req.expected_addon_binding_json).map_err(|_| {
                    Status::failed_precondition("Current addon binding required for import")
                })?;
            if registry.addon_function_bindings.get(&req.import_from) != Some(&expected) {
                return Err(Status::failed_precondition(
                    "Addon function changed; reload before importing",
                ));
            }
            let mut body = source.body;
            body.id = uuid::Uuid::new_v4();
            body.name = info.name.clone();
            if let Some(node) = body.nodes.iter_mut().find(|n| n.id == body.entry_node_id) {
                if !node.data.is_object() {
                    node.data = serde_json::json!({});
                }
                node.data["_imported_from"] = expected;
            }
            FunctionEntry {
                id: body.id,
                name: info.name,
                description: info.description,
                signature: source.signature,
                body,
                source: FunctionSource::Workspace,
            }
        } else {
            let body = proto_to_blueprint(
                &req.body
                    .ok_or_else(|| Status::invalid_argument("body is required"))?,
            )
            .map_err(to_status)?;
            proto_to_function(&info, body).map_err(to_status)?
        };
        crate::registry::library::validate(&entry).map_err(Status::invalid_argument)?;
        ensure_valid(&entry.body, &registry)?;
        entry.signature =
            FunctionEntry::derive_signature(&entry.body).map_err(Status::invalid_argument)?;
        if registry
            .functions()
            .iter()
            .any(|f| f.name != entry.name && (f.id == entry.id || f.body.id == entry.body.id))
        {
            return Err(Status::already_exists(
                "Function identity belongs to another name; create an explicit copy",
            ));
        }
        let mut file_path = String::new();
        if let Some(root) = root {
            let ws = self
                .state
                .workspaces
                .get(&root)
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            let _admission = ws.activity_gate.lock().await;
            if self.state.running.read().await.contains_key(ws.root())
                || self.state.chats.read().await.contains_key(ws.root())
            {
                return Err(Status::failed_precondition(
                    "Function edits require the workspace execution and chat to finish",
                ));
            }
            crate::replan::application::ensure_resolved(&ws.db).map_err(to_status)?;
            let old = crate::registry::library::load_all(&ws.db)
                .map_err(to_status)?
                .into_iter()
                .find(|f| f.name == entry.name);
            if importing && old.is_some() {
                return Err(Status::already_exists("Function copy name is already used"));
            }
            if let Some(old) = old
                && crate::storage::blueprint_files::binding(&ws.db, old.body.id)
                    .map_err(to_status)?
                    .is_some()
                && (entry.id != old.id || entry.body.id != old.body.id)
            {
                return Err(Status::failed_precondition(
                    "Editable copy identity must remain stable",
                ));
            }
            let bound = crate::storage::blueprint_files::binding(&ws.db, entry.body.id)
                .map_err(to_status)?;
            file_path = if req.file_path.is_empty() {
                bound
                    .as_ref()
                    .map(|v| v.blueprint_uri.clone())
                    .unwrap_or_default()
            } else {
                req.file_path
            };
            if !file_path.is_empty() {
                if importing
                    && ws
                        .version_manager
                        .blueprint_path(&file_path)
                        .map_err(to_status)?
                        .exists()
                {
                    return Err(Status::already_exists("Import path already exists"));
                }
                let bytes = crate::storage::blueprint_files::encode_native(&entry.body)
                    .map_err(to_status)?;
                crate::storage::blueprint_files::save(
                    &ws.db,
                    &ws.version_manager,
                    &entry.body,
                    &file_path,
                    &bytes,
                    bound.as_ref(),
                )
                .map_err(to_status)?;
            }
            entry.source = FunctionSource::Workspace;
            crate::registry::library::save(&ws.db, &entry).map_err(to_status)?;
        } else {
            if !req.file_path.is_empty() {
                return Err(Status::invalid_argument(
                    "Versioned function copies belong to a workspace",
                ));
            }
            if !self.state.running.read().await.is_empty()
                || !self.state.chats.read().await.is_empty()
            {
                return Err(Status::failed_precondition(
                    "Global function edits require executions and chats to finish",
                ));
            }
            let db = self
                .state
                .global_db
                .as_ref()
                .ok_or_else(|| Status::unavailable("global database is not enabled"))?;
            entry.source = FunctionSource::Global;
            crate::registry::library::save(db, &entry).map_err(to_status)?;
        }
        self.state.registry.register_function(entry.clone());
        let mut info = function_to_proto(&entry);
        info.file_path = file_path;
        Ok(Response::new(SaveFunctionResponse { info: Some(info) }))
    }

    pub(crate) async fn list_functions(
        &self,
        request: Request<ListFunctionsRequest>,
    ) -> Result<Response<FunctionList>, Status> {
        let req = request.into_inner();
        let root = optional_workspace(&req.workspace_path)?;
        let registry = self.state.registry_for(root.as_deref(), false).await?;
        let ws = if let Some(root) = &root {
            self.state.workspaces.get(root).await
        } else {
            None
        };
        let functions = registry
            .functions()
            .iter()
            .map(|entry| {
                let mut info = function_to_proto(entry);
                info.addon_binding_json = registry
                    .addon_function_bindings
                    .get(&entry.name)
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                if let Some(ws) = &ws {
                    info.file_path =
                        crate::storage::blueprint_files::binding(&ws.db, entry.body.id)
                            .map_err(to_status)?
                            .map(|v| v.blueprint_uri)
                            .unwrap_or_default();
                }
                Ok(info)
            })
            .collect::<Result<Vec<_>, Status>>()?;
        Ok(Response::new(FunctionList { functions }))
    }

    pub(crate) async fn load_function(
        &self,
        request: Request<LoadFunctionRequest>,
    ) -> Result<Response<LoadFunctionResponse>, Status> {
        let req = request.into_inner();
        let root = optional_workspace(&req.workspace_path)?;
        let registry = self.state.registry_for(root.as_deref(), false).await?;
        let entry = registry
            .function(&req.name)
            .ok_or_else(|| Status::not_found("function not found"))?;
        ensure_valid(&entry.body, &registry)?;
        let mut info = function_to_proto(&entry);
        info.addon_binding_json = registry
            .addon_function_bindings
            .get(&entry.name)
            .map(|v| v.to_string())
            .unwrap_or_default();
        if let Some(root) = root
            && let Some(ws) = self.state.workspaces.get(&root).await
        {
            info.file_path = crate::storage::blueprint_files::binding(&ws.db, entry.body.id)
                .map_err(to_status)?
                .map(|v| v.blueprint_uri)
                .unwrap_or_default();
        }
        Ok(Response::new(LoadFunctionResponse {
            info: Some(info),
            body: Some(blueprint_to_proto(&entry.body)),
        }))
    }

    pub(crate) async fn delete_function(
        &self,
        request: Request<DeleteFunctionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let root = optional_workspace(&req.workspace_path)?;
        let registry = self.state.registry_for(root.as_deref(), false).await?;
        if registry
            .function(&req.name)
            .is_some_and(|f| matches!(f.source, FunctionSource::Addon | FunctionSource::Builtin))
        {
            return Err(Status::permission_denied(
                "Read-only function is owned by its package or daemon",
            ));
        }
        if req.workspace_path.is_empty() {
            if !self.state.running.read().await.is_empty()
                || !self.state.chats.read().await.is_empty()
            {
                return Err(Status::failed_precondition(
                    "Global function edits require executions and chats to finish",
                ));
            }
            let db = self
                .state
                .global_db
                .clone()
                .ok_or_else(|| Status::unavailable("global database is not enabled"))?;
            crate::registry::library::delete(&db, &req.name).map_err(to_status)?;
            if let Some(entry) = self.state.registry.function(&req.name)
                && entry.source == FunctionSource::Global
            {
                self.state.registry.unregister_function(&req.name);
            }
        } else {
            let ws = self
                .state
                .workspaces
                .get(&PathBuf::from(&req.workspace_path))
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            let _admission = ws.activity_gate.lock().await;
            if self.state.running.read().await.contains_key(ws.root())
                || self.state.chats.read().await.contains_key(ws.root())
            {
                return Err(Status::failed_precondition(
                    "Function edits require the workspace execution and chat to finish",
                ));
            }
            crate::replan::application::ensure_resolved(&ws.db).map_err(to_status)?;
            crate::registry::library::delete(&ws.db, &req.name).map_err(to_status)?;
            if let Some(entry) = self.state.registry.function(&req.name)
                && entry.source == FunctionSource::Workspace
            {
                self.state.registry.unregister_function(&req.name);
            }
        }
        Ok(Response::new(Empty {}))
    }
    pub(crate) async fn compile_dsl(
        &self,
        request: Request<CompileDslRequest>,
    ) -> Result<Response<Blueprint>, Status> {
        let req = request.into_inner();
        let source = req.source;
        let workspace=optional_workspace(&req.workspace_path)?;
        let registry=self.state.registry_for(workspace.as_deref(),false).await?;
        let blueprint = metteur_shared::dsl::compile_with_catalog(
            &source,
            &registry.authoring_catalog(),
        )
        .map_err(|e| Status::invalid_argument(e.to_string()))?;
        ensure_valid(&blueprint,&registry)?;
        Ok(Response::new(blueprint_to_proto(&blueprint)))
    }

    pub(crate) async fn decompile_blueprint(
        &self,
        request: Request<DecompileBlueprintRequest>,
    ) -> Result<Response<DecompileDslResponse>, Status> {
        let req = request.into_inner();
        // Prefer the in-flight blueprint so the live canvas is exported without
        // depending on the archive (which may be stale or from an older format).
        let blueprint = if let Some(proto_blueprint) = req.blueprint {
            proto_to_blueprint(&proto_blueprint).map_err(to_status)?
        } else {
            let ws = self
                .state
                .workspaces
                .get(&PathBuf::from(&req.workspace_path))
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            let id = uuid::Uuid::parse_str(&req.blueprint_id)
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            crate::storage::blueprint_files::load(&ws.db, &ws.version_manager, id)
                .map_err(to_status)?
        };
        Ok(Response::new(DecompileDslResponse {
            source: metteur_shared::dsl::decompile(&blueprint),
        }))
    }
}

/// Rejects a structurally invalid blueprint before it is stored or run.
///
/// Running an invalid graph surfaces the problem only when the offending node
/// executes, by which point the checkpoint machinery, the LLM budget and any
/// earlier file mutations are already committed. Every problem is reported at
/// once so a client can show the full list rather than one mistake per run.
pub(super) fn ensure_valid(
    blueprint: &metteur_shared::Blueprint,
    registry: &crate::registry::Registry,
) -> Result<(), Status> {
    let report = metteur_shared::model::validate::validate_with_catalog(
        blueprint,
        &registry.node_signatures(),
    );
    if report.is_ok() {
        return Ok(());
    }
    let detail = report.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
    Err(Status::invalid_argument(format!(
        "blueprint is invalid ({} problem(s)): {detail}",
        report.errors.len()
    )))
}

/// Converts a proto blueprint into the shared model.
fn proto_to_blueprint(proto: &Blueprint) -> Result<metteur_shared::Blueprint, DaemonError> {
    let id =
        uuid::Uuid::parse_str(&proto.id).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let nodes = proto.nodes.iter().map(proto_to_node).collect::<Result<Vec<_>, _>>()?;
    let entry_node_id = uuid::Uuid::parse_str(&proto.entry_node_id)
        .map_err(|e| DaemonError::Serialization(e.to_string()))?;
    // `decompile` indexes nodes by id unconditionally; reject dangling
    // references up front instead of panicking downstream.
    if !nodes.iter().any(|n| n.id == entry_node_id) {
        return Err(DaemonError::Serialization(
            "entry node is not present in the blueprint".to_string(),
        ));
    }
    let edges = proto.edges.iter().map(proto_to_edge).collect::<Result<Vec<_>, _>>()?;
    for edge in &edges {
        if !nodes.iter().any(|n| n.id == edge.source_node)
            || !nodes.iter().any(|n| n.id == edge.target_node)
        {
            return Err(DaemonError::Serialization(
                "edge references a node missing from the blueprint".to_string(),
            ));
        }
    }
    Ok(metteur_shared::Blueprint {
        id,
        name: proto.name.clone(),
        nodes,
        edges,
        entry_node_id,
    })
}

/// Converts a proto node into the shared model.
fn proto_to_node(proto: &proto::Node) -> Result<metteur_shared::Node, DaemonError> {
    let node_type = match proto.node_type.as_str() {
        "Event" => metteur_shared::NodeType::Event,
        "Function" => metteur_shared::NodeType::Function,
        "Pure" => metteur_shared::NodeType::Pure,
        "Control" => metteur_shared::NodeType::Control,
        _ => metteur_shared::NodeType::Function,
    };
    let pins = proto.pins.iter().map(proto_to_pin).collect::<Result<Vec<_>, _>>()?;
    Ok(metteur_shared::Node {
        id: uuid::Uuid::parse_str(&proto.id)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        node_type,
        kind: proto.kind.clone(),
        position: (proto.pos_x, proto.pos_y),
        pins,
        data: serde_json::from_str(&proto.data_json).unwrap_or(serde_json::Value::Null),
    })
}

/// Converts a proto pin into the shared model.
fn proto_to_pin(proto: &proto::Pin) -> Result<metteur_shared::Pin, DaemonError> {
    let default = if proto.default_json.is_empty() {
        None
    } else {
        serde_json::from_str::<serde_json::Value>(&proto.default_json).ok()
    };
    Ok(metteur_shared::Pin {
        id: uuid::Uuid::parse_str(&proto.id)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        key: if proto.key.is_empty() {
            None
        } else {
            Some(proto.key.clone())
        },
        name: proto.name.clone(),
        pin_type: match proto.pin_type.as_str() {
            "ExecInput" => metteur_shared::PinType::ExecInput,
            "ExecOutput" => metteur_shared::PinType::ExecOutput,
            "DataInput" => metteur_shared::PinType::DataInput,
            _ => metteur_shared::PinType::DataOutput,
        },
        data_type: proto.data_type.parse().unwrap_or(metteur_shared::DataType::Void),
        default,
        optional: proto.optional,
        choices: proto.choices.clone(),
        description: if proto.description.is_empty() {
            None
        } else {
            Some(proto.description.clone())
        },
    })
}

/// Converts a proto edge into the shared model.
fn proto_to_edge(proto: &proto::Edge) -> Result<metteur_shared::Edge, DaemonError> {
    Ok(metteur_shared::Edge {
        id: uuid::Uuid::parse_str(&proto.id)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        source_node: uuid::Uuid::parse_str(&proto.source_node)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        source_pin: uuid::Uuid::parse_str(&proto.source_pin)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        target_node: uuid::Uuid::parse_str(&proto.target_node)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        target_pin: uuid::Uuid::parse_str(&proto.target_pin)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
    })
}

/// Converts a shared blueprint into the proto model.
fn blueprint_to_proto(blueprint: &metteur_shared::Blueprint) -> Blueprint {
    Blueprint {
        id: blueprint.id.to_string(),
        name: blueprint.name.clone(),
        nodes: blueprint
            .nodes
            .iter()
            .map(|n| proto::Node {
                id: n.id.to_string(),
                node_type: match n.node_type {
                    metteur_shared::NodeType::Event => "Event".to_string(),
                    metteur_shared::NodeType::Function => "Function".to_string(),
                    metteur_shared::NodeType::Pure => "Pure".to_string(),
                    metteur_shared::NodeType::Control => "Control".to_string(),
                },
                kind: n.kind.clone(),
                pos_x: n.position.0,
                pos_y: n.position.1,
                pins: n
                    .pins
                    .iter()
                    .map(|p| proto::Pin {
                        id: p.id.to_string(),
                        name: p.name.clone(),
                        pin_type: match p.pin_type {
                            metteur_shared::PinType::ExecInput => "ExecInput".to_string(),
                            metteur_shared::PinType::ExecOutput => "ExecOutput".to_string(),
                            metteur_shared::PinType::DataInput => "DataInput".to_string(),
                            metteur_shared::PinType::DataOutput => "DataOutput".to_string(),
                        },
                        data_type: p.data_type.to_string(),
                        default_json: p
                            .default
                            .as_ref()
                            .map(serde_json::Value::to_string)
                            .unwrap_or_default(),
                        optional: p.optional,
                        choices: p.choices.clone(),
                        description: p.description.clone().unwrap_or_default(),
                        key: p.key.clone().unwrap_or_default(),
                    })
                    .collect(),
                data_json: serde_json::to_string(&n.data).unwrap_or_else(|_| "null".to_string()),
            })
            .collect(),
        edges: blueprint
            .edges
            .iter()
            .map(|e| proto::Edge {
                id: e.id.to_string(),
                source_node: e.source_node.to_string(),
                source_pin: e.source_pin.to_string(),
                target_node: e.target_node.to_string(),
                target_pin: e.target_pin.to_string(),
            })
            .collect(),
        entry_node_id: blueprint.entry_node_id.to_string(),
    }
}

/// Converts a shared function entry into the proto model.
fn function_to_proto(entry: &FunctionEntry) -> ProtoFunctionInfo {
    let source = match entry.source {
        FunctionSource::Addon => "addon",
        FunctionSource::Builtin => "builtin",
        FunctionSource::Global => "global",
        FunctionSource::Workspace => "workspace",
    };
    ProtoFunctionInfo {
        id: entry.id.to_string(),
        name: entry.name.clone(),
        description: entry.description.clone(),
        inputs: entry.signature.inputs.iter().map(fn_pin_to_proto).collect(),
        outputs: entry.signature.outputs.iter().map(fn_pin_to_proto).collect(),
        source: source.to_string(),
        updated_at: 0,
        addon_binding_json:String::new(),
        file_path:String::new(),
    }
}

/// Converts a proto signature pin into the shared model.
fn proto_to_fn_pin(pin: &ProtoFnPin) -> Result<FnPin, DaemonError> {
    let default = if pin.default_json.is_empty() {
        None
    } else {
        serde_json::from_str::<serde_json::Value>(&pin.default_json).ok()
    };
    Ok(FnPin {
        name: pin.name.clone(),
        data_type: pin.data_type.parse().unwrap_or(metteur_shared::DataType::Void),
        description: if pin.description.is_empty() {
            None
        } else {
            Some(pin.description.clone())
        },
        default,
        optional: pin.optional,
    })
}

/// Converts a shared signature pin into the proto model.
fn fn_pin_to_proto(pin: &FnPin) -> ProtoFnPin {
    ProtoFnPin {
        name: pin.name.clone(),
        data_type: pin.data_type.to_string(),
        description: pin.description.clone().unwrap_or_default(),
        default_json: pin.default.as_ref().map(serde_json::Value::to_string).unwrap_or_default(),
        optional: pin.optional,
    }
}

/// Converts a proto function request into a shared entry.
fn proto_to_function(
    info: &ProtoFunctionInfo,
    body: metteur_shared::Blueprint,
) -> Result<FunctionEntry, DaemonError> {
    let inputs = info.inputs.iter().map(proto_to_fn_pin).collect::<Result<Vec<_>, _>>()?;
    let outputs = info.outputs.iter().map(proto_to_fn_pin).collect::<Result<Vec<_>, _>>()?;
    Ok(FunctionEntry {
        id: uuid::Uuid::parse_str(&info.id)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?,
        name: info.name.clone(),
        description: info.description.clone(),
        signature: metteur_shared::model::function::FunctionSignature {
            inputs,
            outputs,
        },
        body,
        source: FunctionSource::Workspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::PinType;

    fn node(id: &str) -> proto::Node {
        proto::Node {
            id: id.into(),
            node_type: "Pure".into(),
            kind: "Add".into(),
            pos_x: 0.0,
            pos_y: 0.0,
            pins: vec![],
            data_json: "{}".into(),
        }
    }

    fn bp(nodes: Vec<proto::Node>, edges: Vec<proto::Edge>) -> proto::Blueprint {
        let entry = nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        proto::Blueprint {
            id: uuid::Uuid::new_v4().to_string(),
            name: "t".into(),
            entry_node_id: entry,
            nodes,
            edges,
        }
    }

    fn edge(src: &proto::Node, dst: &proto::Node) -> proto::Edge {
        proto::Edge {
            id: uuid::Uuid::new_v4().to_string(),
            source_node: src.id.clone(),
            source_pin: src.pins.first().map(|p| p.id.clone()).unwrap_or_default(),
            target_node: dst.id.clone(),
            target_pin: dst.pins.first().map(|p| p.id.clone()).unwrap_or_default(),
        }
    }

    #[test]
    fn parses_well_formed_blueprint() {
        let n = node(&uuid::Uuid::new_v4().to_string());
        let parsed = proto_to_blueprint(&bp(vec![n], vec![])).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
    }

    #[test]
    fn rejects_missing_entry_node() {
        let n = node(&uuid::Uuid::new_v4().to_string());
        let mut blueprint = bp(vec![n], vec![]);
        blueprint.entry_node_id = uuid::Uuid::new_v4().to_string();
        assert!(proto_to_blueprint(&blueprint).is_err());
    }

    #[test]
    fn rejects_dangling_edge() {
        let n = node(&uuid::Uuid::new_v4().to_string());
        let ghost = node(&uuid::Uuid::new_v4().to_string());
        let mut edges = vec![edge(&n, &ghost)];
        // The ghost node supplies a real pin id so the error is the node ref.
        edges[0].source_pin = uuid::Uuid::new_v4().to_string();
        edges[0].target_pin = uuid::Uuid::new_v4().to_string();
        assert!(proto_to_blueprint(&bp(vec![n], edges)).is_err());
    }

    #[test]
    fn requires_uuid_ids() {
        let n = node("n-add-0");
        assert!(proto_to_blueprint(&bp(vec![n], vec![])).is_err());
    }

    #[test]
    fn rejects_empty_entry_id() {
        let n = node(&uuid::Uuid::new_v4().to_string());
        let mut blueprint = bp(vec![n], vec![]);
        blueprint.entry_node_id = String::new();
        assert!(proto_to_blueprint(&blueprint).is_err());
    }

    #[test]
    fn pin_carries_semantic_fields() {
        let mut n = node(&uuid::Uuid::new_v4().to_string());
        n.pins = vec![proto::Pin {
            id: uuid::Uuid::new_v4().to_string(),
            name: "x".into(),
            pin_type: "DataInput".into(),
            data_type: "int".into(),
            default_json: String::new(),
            optional: false,
            choices: vec![],
            description: String::new(),
            key: "x".into(),
        }];
        let parsed = proto_to_blueprint(&bp(vec![n], vec![])).unwrap();
        assert_eq!(parsed.nodes[0].pins.len(), 1);
        assert!(matches!(parsed.nodes[0].pins[0].pin_type, PinType::DataInput));
        assert_eq!(parsed.nodes[0].pins[0].name, "x");
    }
}
