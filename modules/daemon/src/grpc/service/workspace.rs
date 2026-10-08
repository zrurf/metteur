//! Workspace RPCs: open, close and list.

use std::path::PathBuf;

use metteur_shared::model::function::FunctionSource;
use tonic::{Request, Response, Status};

use super::super::acl::subject_from_request;
use super::super::proto::{
    CloseWorkspaceRequest, Empty, OpenWorkspaceRequest, WorkspaceInfo, WorkspaceList,
};
use super::*;

impl DaemonService {
    pub(crate) async fn open_workspace(
        &self,
        request: Request<OpenWorkspaceRequest>,
    ) -> Result<Response<WorkspaceInfo>, Status> {
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".to_string());
        let req = request.into_inner();
        let _config_guard = self.state.config_gate.lock().await;
        let already_open = self.state.workspaces.get(&PathBuf::from(&req.path)).await.is_some();
        let ws = self.state.workspaces.open(&PathBuf::from(req.path)).await.map_err(to_status)?;
        if let Some(host)=&self.state.addon_host {host.opened_workspace(ws.root()).await;}
        // Register this workspace's function library into the shared registry,
        // remembering the names so only this workspace's set is retired later.
        match self.state.registry.load_functions(&ws.db, FunctionSource::Workspace) {
            Ok(names) => {
                self.state.ws_functions.write().await.insert(ws.root().to_path_buf(), names);
            }
            Err(err) => tracing::warn!("failed to load workspace functions: {err}"),
        }
        // A newly opened workspace may declare MCP servers of its own.
        if let Err(error) = self.state.resync_mcp().await {
            // Opening/closing the workspace succeeded. MCP status remains
            // available through ListMcpServers; SetConfig reports this failure.
            tracing::warn!("workspace MCP reload failed: {error}");
        }
        if !already_open && let Some(host) = &self.state.addon_host { host.observe_workspace_open(ws.root()); }
        record_global_audit(
            &self.state,
            &subject,
            "workspace.open",
            serde_json::json!({ "path": ws.root().to_string_lossy() }),
        );
        self.state.metrics.workspaces_active.store(
            self.state.workspaces.list().await.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(Response::new(WorkspaceInfo {
            path: ws.root().to_string_lossy().to_string(),
            locked: true,
        }))
    }

    pub(crate) async fn close_workspace(
        &self,
        request: Request<CloseWorkspaceRequest>,
    ) -> Result<Response<Empty>, Status> {
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".to_string());
        let req = request.into_inner();
        let _config_guard = self.state.config_gate.lock().await;
        // Resolve the workspace first: its `root()` is the normalized key used
        // when its functions were registered.
        let root = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.path))
            .await
            .map(|ws| ws.root().to_path_buf())
            .ok_or_else(|| Status::not_found("workspace is not open"))?;
        if self.state.running.read().await.contains_key(&root) || self.state.chats.read().await.contains_key(&root) {return Err(Status::failed_precondition("Workspace has an active execution or chat turn"));}
        if let Some(host)=&self.state.addon_host {host.close_workspace(&root,&self.state.workspaces).await.map_err(to_status)?;}
        else {self.state.workspaces.close(&root).await.map_err(to_status)?;}
        // Retire exactly this workspace's functions. A name another open
        // workspace also defines is restored from that workspace's database;
        // otherwise the global definition (if any) takes over, so closing one
        // workspace never strips another's library.
        let names = self.state.ws_functions.write().await.remove(&root).unwrap_or_default();
        let global_db = self.state.global_db.clone();
        for name in names {
            self.state.registry.unregister_function(&name);
            let mut restored = false;
            for ws in self.state.workspaces.list().await {
                if ws.root() == root.as_path() {
                    continue;
                }
                if self
                    .state
                    .registry
                    .restore_function(&ws.db, &name, FunctionSource::Workspace)
                    .unwrap_or(false)
                {
                    restored = true;
                    break;
                }
            }
            if !restored
                && let Some(db) = &global_db
            {
                let _ = self.state.registry.restore_function(db, &name, FunctionSource::Global);
            }
        }
        // Servers declared only by the closed workspace are shut down here.
        if let Err(error) = self.state.resync_mcp().await {
            // Opening/closing the workspace succeeded. MCP status remains
            // available through ListMcpServers; SetConfig reports this failure.
            tracing::warn!("workspace MCP reload failed: {error}");
        }
        record_global_audit(&self.state, &subject, "workspace.close", serde_json::json!({}));
        self.state.metrics.workspaces_active.store(
            self.state.workspaces.list().await.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn list_workspaces(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<WorkspaceList>, Status> {
        let workspaces = self.state.workspaces.list().await;
        let infos = workspaces
            .into_iter()
            .map(|ws| WorkspaceInfo {
                path: ws.root().to_string_lossy().to_string(),
                locked: true,
            })
            .collect();
        Ok(Response::new(WorkspaceList {
            workspaces: infos,
        }))
    }
}
