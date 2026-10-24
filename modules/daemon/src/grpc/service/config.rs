//! Configuration RPCs: get/set config and the audit log.

use std::path::PathBuf;

use metteur_shared::config::ConfigLayer;
use tonic::{Request, Response, Status};

use crate::error::DaemonError;
use crate::observability::audit::AuditWriter;

use super::super::acl::subject_from_request;
use super::super::proto::{
    AuditEntry as ProtoAuditEntry, AuditLogList, Config as ProtoConfig, Empty, GetConfigRequest,
    ListAuditLogRequest, SetConfigRequest,
};
use super::*;

impl DaemonService {
    pub(crate) async fn get_config(
        &self,
        request: Request<GetConfigRequest>,
    ) -> Result<Response<ProtoConfig>, Status> {
        let req = request.into_inner();
        let _guard = self.state.config_gate.lock().await;
        let is_workspace = !req.workspace_path.is_empty();
        let (path, effective) = if !is_workspace {
            (
                self.state.workspaces.global_config_path().map_err(to_status)?,
                self.state.global_config.read().await.clone(),
            )
        } else {
            let ws = self
                .state
                .workspaces
                .get(&PathBuf::from(&req.workspace_path))
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            let effective = ws.config.read().await.clone();
            (ws.root().join(crate::config::CONFIG_DIR).join(crate::config::CONFIG_FILE), effective)
        };
        let layer = crate::config::load_config_layer(&path).map_err(to_status)?;
        let overrides = layer
            .compatible_overrides(is_workspace)
            .map_err(|e| Status::internal(e.to_string()))?;
        // Unknown legacy extensions may have whole-section semantics that a
        // presence overlay cannot represent. Keep raw TOML editable, but do not
        // offer a form migration that changes the effective configuration.
        let compatible = if is_workspace {
            overrides
                .merge(&*self.state.global_config.read().await)
                .map_err(|e| Status::internal(e.to_string()))?
                == effective
        } else {
            overrides.effective().map_err(|e| Status::internal(e.to_string()))? == effective
        };
        Ok(Response::new(ProtoConfig {
            config_json: serde_json::to_string(&layer)
                .map_err(|e| Status::internal(e.to_string()))?,
            overrides_json: if compatible {
                serde_json::to_string(&overrides).map_err(|e| Status::internal(e.to_string()))?
            } else {
                String::new()
            },
            effective_json: serde_json::to_string(&effective)
                .map_err(|e| Status::internal(e.to_string()))?,
            legacy_format: layer.config_version.is_none(),
            defaults_json: serde_json::to_string(&metteur_shared::config::Config::default())
                .map_err(|e| Status::internal(e.to_string()))?,
        }))
    }

    pub(crate) async fn set_config(
        &self,
        request: Request<SetConfigRequest>,
    ) -> Result<Response<Empty>, Status> {
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".to_string());
        let req = request.into_inner();
        let _guard = self.state.config_gate.lock().await;
        let mut layer: ConfigLayer = serde_json::from_str(&req.config_json)
            .map_err(|e| Status::invalid_argument(format!("invalid config: {e}")))?;
        let config = layer
            .effective()
            .map_err(|e| Status::invalid_argument(format!("invalid config: {e}")))?;
        layer.discard_legacy_nulls();
        let global_scope = req.workspace_path.is_empty();
        let mut updates = Vec::new();
        let path = if global_scope {
            for ws in self.state.workspaces.list().await {
                let ws_layer = crate::config::load_config_layer(
                    &ws.root().join(crate::config::CONFIG_DIR).join(crate::config::CONFIG_FILE),
                )
                .map_err(to_status)?;
                let merged =
                    ws_layer.merge(&config).map_err(|e| Status::invalid_argument(e.to_string()))?;
                validate_lsp(&merged.lsp)?;
                updates.push((ws, merged));
            }
            self.state.workspaces.global_config_path().map_err(to_status)?
        } else {
            let ws = self
                .state
                .workspaces
                .get(&PathBuf::from(&req.workspace_path))
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            let global = self.state.global_config.read().await;
            let merged =
                layer.merge(&global).map_err(|e| Status::invalid_argument(e.to_string()))?;
            validate_lsp(&merged.lsp)?;
            let path = ws.root().join(crate::config::CONFIG_DIR).join(crate::config::CONFIG_FILE);
            updates.push((ws, merged));
            path
        };
        // Validate every affected workspace before changing the file or live state.
        if global_scope {
            validate_lsp(&config.lsp)?;
        }
        let toml = toml::to_string(&layer).map_err(|e| {
            Status::invalid_argument(format!("configuration cannot be saved as TOML: {e}"))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| to_status(DaemonError::Io(e)))?;
        }
        std::fs::write(&path, toml).map_err(|e| to_status(DaemonError::Io(e)))?;
        let mut pending = Vec::new();
        if global_scope {
            *self.state.global_config.write().await = config.clone();
            match self.state.acl_store.write() {
                Ok(mut acl) => *acl = config.acl.clone(),
                Err(_) => pending.push("ACL store is poisoned; restart the daemon".to_string()),
            }
            let _ = self.state.config_tx.send(config.clone());
            if config.daemon != self.state.startup_config.daemon {
                pending.push("daemon process settings require restart; disabling an existing OS startup registration also requires removing that registration".into());
            }
            if config.addon != self.state.startup_config.addon {
                pending.push("addon host settings require daemon restart".into());
            }
            record_global_audit(
                &self.state,
                &subject,
                "config.set",
                serde_json::json!({"scope":"global"}),
            );
        }
        for (ws, merged) in updates {
            if merged.versioning.auto_snapshot != ws.applied_auto_snapshot {
                pending
                    .push(format!("reopen {} to apply automatic snapshots", ws.root().display()));
            }
            if !global_scope && merged.addon != self.state.startup_config.addon {
                pending.push("addon host settings are global; update the global layer and restart the daemon".into());
            }
            if !global_scope && merged.daemon != self.state.startup_config.daemon {
                pending.push("daemon process settings are global; update the global layer and restart the daemon".into());
            }
            if !global_scope
                && merged.mcp.call_timeout_secs
                    != self.state.global_config.read().await.mcp.call_timeout_secs
            {
                pending.push(
                    "MCP host timeout is global; update the global MCP call_timeout_secs".into(),
                );
            }
            *ws.config.write().await = merged;
            ws.db.notify_changes();
            ws.reload_lsp().await;
            if !global_scope {
                let _ = AuditWriter::new(ws.db.clone()).record(
                    &subject,
                    "config.set",
                    serde_json::json!({"scope":"workspace"}),
                );
            }
        }
        if let Err(error) = self.state.resync_mcp().await {
            pending.push(error.to_string());
        }
        if !pending.is_empty() {
            return Err(Status::failed_precondition(format!(
                "Configuration saved, but not fully applied: {}",
                pending.join("; ")
            )));
        }
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn list_audit_log(
        &self,
        request: Request<ListAuditLogRequest>,
    ) -> Result<Response<AuditLogList>, Status> {
        let req = request.into_inner();
        let entries = if req.workspace_path.is_empty() {
            let writer = self
                .state
                .global_audit
                .clone()
                .ok_or_else(|| Status::not_found("global audit is not enabled"))?;
            writer.list().map_err(to_status)?
        } else {
            let ws = self
                .state
                .workspaces
                .get(&PathBuf::from(&req.workspace_path))
                .await
                .ok_or_else(|| Status::not_found("workspace not open"))?;
            AuditWriter::new(ws.db.clone()).list().map_err(to_status)?
        };
        let entries = entries
            .into_iter()
            .map(|entry| ProtoAuditEntry {
                timestamp: entry.timestamp as i64,
                user_id: entry.user_id,
                operation: entry.operation,
                detail_json: entry.detail.to_string(),
            })
            .collect();
        Ok(Response::new(AuditLogList {
            entries,
        }))
    }
}

fn validate_lsp(config: &metteur_shared::config::LspConfig) -> Result<(), Status> {
    if config.enabled
        && config.languages.iter().any(|l| {
            l.id.trim().is_empty() || l.command.first().is_none_or(|c| c.trim().is_empty())
        })
    {
        return Err(Status::invalid_argument(
            "enabled LSP languages need an id and a nonempty command; processes start on first use",
        ));
    }
    Ok(())
}
