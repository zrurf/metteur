//! Authenticated input recovery, with no model/tool/approval side effects.
use super::{
    super::proto::{ResourceRequest, ResourceResponse},
    DaemonService,
    resources::response,
};
use crate::storage::{
    recovery::{DraftInput, MAX_PAGE, RecoveryStore},
    resources::{Result, fault},
};
use base64::Engine;
use serde::Deserialize;
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    scope: String,
    action: Action,
}
#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Capabilities,
    LatestView,
    List {
        after: Option<Uuid>,
        limit: Option<usize>,
    },
    Read {
        id: Uuid,
    },
    Export {
        id: Uuid,
    },
    Backup {
        input: Box<DraftInput>,
    },
    Discard {
        id: Uuid,
        saved_sequence: u64,
    },
}

fn validate_target(root: &std::path::Path, scope: &str, input: &mut DraftInput) -> Result<()> {
    if input.kind == "text"
        && ((scope == "personal" && input.entity == "metteur://config/user.toml")
            || (scope == "project" && input.entity == "metteur://config/workspace.toml"))
    {
        return Ok(());
    }
    if input.entity.starts_with("@prompt/") {
        let parts: Vec<_> = input.entity.split('/').collect();
        if parts.len() != 3
            || parts[1] != scope
            || !parts[2].ends_with(".md")
            || Uuid::parse_str(parts[2].trim_end_matches(".md")).is_err()
            || input.kind != "text"
        {
            return Err(fault(
                "invalid_target",
                "Prompt recovery requires the matching host scope and resource identity",
            ));
        }
        return Ok(());
    }
    if scope != "project" {
        return Err(fault(
            "invalid_scope",
            "Personal recovery is limited to personal Prompt inputs",
        ));
    }
    match input.kind.as_str() {
        "text" | "canvas" => {
            let path = super::resolve_ws_path(root, &input.entity).map_err(|_| {
                fault("invalid_target", "Recovery path must remain inside the workspace")
            })?;
            if path == root {
                return Err(fault("invalid_target", "Choose a file inside the workspace"));
            }
            input.entity = path
                .strip_prefix(root)
                .map_err(|_| fault("invalid_target", "Recovery path is outside the workspace"))?
                .to_string_lossy()
                .replace('\\', "/");
        }
        "chat" | "new_chat" | "supervision" | "workflow" | "workflow_review" => {
            Uuid::parse_str(&input.entity)
                .map_err(|_| fault("invalid_target", "Recovery target must have a stable UUID"))?;
        }
        "view_state" if input.entity == "workspace" => {}
        _ => return Err(fault("invalid_target", "Unknown recovery target")),
    }
    Ok(())
}

impl DaemonService {
    pub(crate) async fn recovery(
        &self,
        request: Request<ResourceRequest>,
    ) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 2 * 1024 * 1024 {
            return Ok(response(Err(fault(
                "unsupported_request",
                "Unsupported recovery envelope or size; input was not backed up",
            ))));
        }
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        let _admission = ws.admit().await.map_err(super::to_status)?;
        let result = (|| -> Result<serde_json::Value> {
            let envelope: Envelope = serde_json::from_str(&req.payload_json)
                .map_err(|_| fault("invalid_request", "Invalid recovery fields"))?;
            let (db, identity) = match envelope.scope.as_str() {
                "project" => (ws.db.clone(), format!("project:{}", ws.root().to_string_lossy())),
                "personal" => (
                    self.state.global_db.clone().ok_or_else(|| {
                        fault("unavailable", "Personal recovery requires user storage")
                    })?,
                    "personal".into(),
                ),
                _ => return Err(fault("invalid_scope", "Scope must be project or personal")),
            };
            let store = RecoveryStore::new(db, &identity, &subject)?;
            match envelope.action {
                Action::Export {
                    id,
                } => Ok(
                    serde_json::json!({"id":id,"encoding":"base64","raw":base64::engine::general_purpose::STANDARD.encode(store.export(id)?)}),
                ),
                Action::Capabilities => Ok(
                    serde_json::json!({"schema":1,"durable_ack":true,"creation_epoch":store.creation_epoch()?,"tombstone_compaction":true,"latest_view":true,"record_bytes":crate::storage::recovery::MAX_RECORD_BYTES,"scope_bytes":crate::storage::recovery::MAX_SCOPE_BYTES,"max_active":crate::storage::recovery::MAX_ACTIVE,"max_records":crate::storage::recovery::MAX_RECORDS,"max_page":MAX_PAGE,"restores_permissions":false}),
                ),
                Action::LatestView => Ok(
                    serde_json::json!({"record":store.latest_view()?,"restores_permissions":false}),
                ),
                Action::Read {
                    id,
                } => {
                    let record = store
                        .get(id)?
                        .ok_or_else(|| fault("not_found", "Recovery copy not found"))?;
                    let source_available = if record.input.kind == "chat" {
                        Uuid::parse_str(&record.input.entity)
                            .ok()
                            .and_then(|id| {
                                crate::chat::session::load_thread(&ws.db, &id).ok().flatten()
                            })
                            .is_some()
                    } else if matches!(record.input.kind.as_str(), "text" | "canvas")
                        && !record.input.entity.starts_with("@prompt/")
                    {
                        super::resolve_ws_path(ws.root(), &record.input.entity)
                            .is_ok_and(|path| path.is_file())
                    } else {
                        false
                    };
                    Ok(
                        serde_json::json!({"record":record,"source_available":source_available,"requires_current_validation":true,"restores_permissions":false}),
                    )
                }
                Action::List {
                    after,
                    limit,
                } => {
                    let limit = limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE);
                    let items = store.list(after, limit)?;
                    let next = if items.len() == limit {
                        items.last().map(|item| item.id)
                    } else {
                        None
                    };
                    Ok(serde_json::json!({"items":items,"next":next}))
                }
                Action::Backup {
                    mut input,
                } => {
                    validate_target(ws.root(), &envelope.scope, &mut input)?;
                    let request = Uuid::parse_str(&req.request_id).map_err(|_| {
                        fault("invalid_request", "A recovery request UUID is required")
                    })?;
                    let record = store.backup(
                        *input,
                        req.expected_revision,
                        request,
                        chrono::Utc::now().timestamp_millis() as u64,
                    )?;
                    serde_json::to_value(record).map_err(|_| {
                        fault("storage_unavailable", "Cannot encode recovery acknowledgment")
                    })
                }
                Action::Discard {
                    id,
                    saved_sequence,
                } => {
                    let request = Uuid::parse_str(&req.request_id).map_err(|_| {
                        fault("invalid_request", "A recovery request UUID is required")
                    })?;
                    serde_json::to_value(store.discard(
                        id,
                        req.expected_revision,
                        saved_sequence,
                        request,
                        chrono::Utc::now().timestamp_millis() as u64,
                    )?)
                    .map_err(|_| {
                        fault("storage_unavailable", "Cannot encode recovery acknowledgment")
                    })
                }
            }
        })();
        Ok(response(result))
    }
}
