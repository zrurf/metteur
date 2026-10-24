//! Receipt queries do not resume an execution or restore old permissions.
use super::{
    super::proto::{ResourceRequest, ResourceResponse},
    DaemonService,
    resources::response,
};
use crate::storage::{
    delivery::{DeliveryReceipt, DeliveryStore, MAX_LEDGER_BYTES, MAX_RECEIPTS},
    resources::{Result, fault},
};
use serde::Deserialize;
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    Workflow {
        action: super::workflow_receipts::Query,
    },
    Admission,
    ControlTarget,
    Capabilities,
    ReconcilePending,
    LiveSubmit(super::live_input::LiveInput),
    LiveBody {
        id: Uuid,
    },
    LiveReceipts {
        ids: Vec<Uuid>,
    },
    Read {
        id: Uuid,
    },
    Retire {
        id: Uuid,
    },
    List {
        after: Option<Uuid>,
    },
    SessionRevision {
        session: Uuid,
    },
}

impl DaemonService {
    pub(crate) async fn chat_delivery(
        &self,
        request: Request<ResourceRequest>,
    ) -> std::result::Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 1024 * 1024 + 16 * 1024 {
            return Ok(response(Err(fault("unsupported_request", "Unsupported delivery query"))));
        }
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace not open"))?;
        let query: Query = match serde_json::from_str(&req.payload_json) {
            Ok(query) => query,
            Err(_) => {
                return Ok(response(Err(fault(
                    "invalid_request",
                    "Invalid delivery query fields",
                ))));
            }
        };
        // Feature discovery must not wait behind a file write/admission RPC
        // before a concrete Stop can be sent. It reads no ledger or content.
        if matches!(query, Query::Capabilities) {
            return Ok(response(Ok(
                serde_json::json!({"schema":1,"durable_manual_send":true,"pending_reconciliation":true,"conditional_files":true,"frozen_file_references":true,"frozen_result_references":true,"targeted_controls":true,"durable_queue":true,"durable_queue_channel":"chat","max_receipts":MAX_RECEIPTS,"ledger_bytes":MAX_LEDGER_BYTES,"ordinary_disconnect":"cancel","automatic_replay":false}),
            )));
        }
        if matches!(query, Query::ControlTarget) {
            return Ok(response(
                serde_json::to_value(self.control_target(ws.root()).await).map_err(|_| {
                    fault("invalid_state", "Live control target could not be encoded")
                }),
            ));
        }
        if let Query::Workflow {
            action,
        } = query
        {
            return Ok(response(self.workflow_query(&ws, &subject, action).await));
        }
        let _guard = ws.admit().await.map_err(super::to_status)?;
        if let Query::LiveSubmit(input) = query {
            return Ok(response(self.receive_live_input(&ws, &subject, input).await.and_then(
                |receipt| {
                    serde_json::to_value(receipt)
                        .map_err(|_| fault("invalid_state", "Receipt could not be encoded"))
                },
            )));
        }
        if req.payload_json.len() > 4096 {
            return Ok(response(Err(fault("invalid_request", "Query is too large"))));
        }
        if matches!(query, Query::Admission) {
            return Ok(response(self.foreground(ws.root(), &ws.db).await.and_then(|value| {
                serde_json::to_value(value)
                    .map_err(|_| fault("invalid_state", "Foreground state could not be encoded"))
            })));
        }
        let chats = self.state.chats.read().await;
        let current = chats.get(ws.root());
        let result = (|| -> Result<serde_json::Value> {
            let store = DeliveryStore::new(ws.db.clone(), &ws.root().to_string_lossy(), &subject)?;
            let receipt_payload = |receipt: Option<DeliveryReceipt>| -> Result<serde_json::Value> {
                let receipt = receipt
                    .map(|receipt| super::live_input::normalize(&store, receipt, current))
                    .transpose()?;
                let readable = self
                    .state
                    .acl_store
                    .read()
                    .unwrap()
                    .is_allowed(&subject, "/metteur.Daemon/GetChatSession");
                let source_available = if readable {
                    if let Some(receipt) = &receipt {
                        if receipt.session.is_nil() {
                            false
                        } else {
                            ws.db.get(crate::storage::persistence::cf::CHAT_THREADS,receipt.session.as_bytes()).map_err(|_|fault("storage_unavailable","Input source could not be checked"))?.is_some_and(|raw|serde_json::from_slice::<crate::chat::session::ChatSessionRecord>(&raw).is_ok_and(|record|record.session_id==receipt.session))
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };
                Ok(serde_json::json!({"receipt":receipt,"source_available":source_available}))
            };
            match query {
                Query::LiveSubmit(_)
                | Query::Workflow {
                    ..
                } => unreachable!("handled asynchronously"),
                Query::LiveBody {
                    id,
                } => Ok(serde_json::json!({"id":id,"body":store.live_body(id)?})),
                Query::LiveReceipts {
                    ids,
                } => {
                    if ids.len() > 64 {
                        return Err(fault("invalid_request", "Receipt batch exceeds its bound"));
                    }
                    let items = ids
                        .into_iter()
                        .map(|id| receipt_payload(store.get(id)?))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(serde_json::json!({"items":items}))
                }
                Query::Admission | Query::ControlTarget => {
                    unreachable!("Handled before synchronous receipt access")
                }
                Query::Capabilities => unreachable!("Handled without workspace admission"),
                Query::ReconcilePending => {
                    let recovery = crate::storage::recovery::RecoveryStore::new(
                        ws.db.clone(),
                        &format!("project:{}", ws.root().to_string_lossy()),
                        &subject,
                    )?;
                    let now = chrono::Utc::now().timestamp_millis() as u64;
                    let mut items = Vec::new();
                    for id in recovery.pending_submissions()? {
                        items.push(store.retire(id, now)?);
                    }
                    Ok(serde_json::json!({"items":items,"raw_copies_unchanged":true}))
                }
                Query::Read {
                    id,
                } => receipt_payload(store.get(id)?),
                Query::Retire {
                    id,
                } => receipt_payload(Some(
                    store.retire(id, chrono::Utc::now().timestamp_millis() as u64)?,
                )),
                Query::List {
                    after,
                } => {
                    let items = store
                        .list(after)?
                        .into_iter()
                        .map(|receipt| super::live_input::normalize(&store, receipt, current))
                        .collect::<Result<Vec<_>>>()?;
                    let next = if items.len() == 50 {
                        items.last().map(|r| r.id)
                    } else {
                        None
                    };
                    Ok(serde_json::json!({"items":items,"next":next}))
                }
                Query::SessionRevision {
                    session,
                } => {
                    if !self
                        .state
                        .acl_store
                        .read()
                        .unwrap()
                        .is_allowed(&subject, "/metteur.Daemon/GetChatSession")
                    {
                        return Err(fault(
                            "permission_denied",
                            "Current conversation read permission is required",
                        ));
                    }
                    let exists = crate::chat::session::load_thread(&ws.db, &session)
                        .map_err(|_| {
                            fault("storage_unavailable", "Conversation could not be read")
                        })?
                        .is_some();
                    if !exists {
                        return Err(fault(
                            "not_found",
                            "Conversation is missing; it will not be recreated",
                        ));
                    }
                    Ok(
                        serde_json::json!({"session":session,"revision":ws.db.chat_revision(session.as_bytes()).map_err(|_|fault("storage_unavailable","Conversation revision could not be read"))?}),
                    )
                }
            }
        })();
        Ok(response(result))
    }
}
