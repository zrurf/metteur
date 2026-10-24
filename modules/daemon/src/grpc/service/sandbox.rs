//! Narrow approval decisions and their durable, owner-bound receipts.
use super::super::proto::{ApprovalDecisionRequest, Empty, ResourceRequest, ResourceResponse};
use super::{DaemonService, resources::response};
use crate::sandbox::{
    approval::{ApprovalBroker, parse_decision},
    grant::GrantStore,
};
use crate::storage::{
    approval_delivery::{ApprovalDelivery, Phase, Target},
    resources::fault,
};
use serde::Deserialize;
use std::{path::Path, sync::Arc};
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    Capabilities,
    Read {
        id: Uuid,
    },
    Describe {
        id: Uuid,
    },
    ReadOrRetire {
        id: Uuid,
        target: Target,
        content_digest: String,
    },
}
impl DaemonService {
    async fn active_approval(
        &self,
        root: &Path,
        id: &str,
    ) -> Option<(Target, Arc<ApprovalBroker>, String)> {
        if let Some(run) = self.state.running.read().await.get(root) {
            let broker = run.approvals.as_ref()?;
            if !broker.pending_ids().iter().any(|pending| pending == id) {
                return None;
            }
            let detail = run.approval_details.lock().get(id)?.clone();
            return Some((
                Target {
                    kind: "blueprint".into(),
                    run: Some(run.run_id),
                    session: None,
                    turn: None,
                },
                broker.clone(),
                ApprovalDelivery::content_digest(&detail),
            ));
        }
        let chats = self.state.chats.read().await;
        let chat = chats.get(root)?;
        let broker = chat.approvals.as_ref()?;
        if !broker.pending_ids().iter().any(|pending| pending == id) {
            return None;
        }
        let detail = chat.approval_details.lock().get(id)?.clone();
        Some((
            Target {
                kind: "chat".into(),
                run: None,
                session: chat.session_id,
                turn: Some(chat.turn_id),
            },
            broker.clone(),
            ApprovalDelivery::content_digest(&detail),
        ))
    }
    pub(crate) async fn approval_delivery(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 4096 {
            return Ok(response(Err(fault(
                "invalid_request",
                "Unsupported approval receipt query",
            ))));
        }
        let query: Query = match serde_json::from_str(&req.payload_json) {
            Ok(query) => query,
            Err(_) => {
                return Ok(response(Err(fault(
                    "invalid_request",
                    "Invalid approval receipt query",
                ))));
            }
        };
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        if !self
            .state
            .acl_store
            .read()
            .unwrap()
            .is_allowed(&subject, "/metteur.Daemon/RespondApproval")
        {
            return Ok(response(Err(fault(
                "permission_denied",
                "Current approval permission is required",
            ))));
        }
        if matches!(query, Query::Capabilities) {
            return Ok(response(Ok(
                serde_json::json!({"schema":1,"durable_decisions":true,"fixed_target_content":true,"read_or_retire":true,"automatic_replay":false,"max_receipts":crate::storage::approval_delivery::MAX_RECEIPTS,"bytes":crate::storage::approval_delivery::MAX_BYTES}),
            )));
        }
        let store = match ApprovalDelivery::new(ws.db.clone(), &subject) {
            Ok(store) => store,
            Err(error) => return Ok(response(Err(error))),
        };
        if let Query::Describe {
            id,
        } = query
        {
            let receipt = match store.read(id) {
                Ok(receipt) => receipt,
                Err(error) => return Ok(response(Err(error))),
            };
            let pending = if receipt.is_none() {
                self.active_approval(ws.root(), &id.to_string()).await.map(|(target, _, content_digest)| serde_json::json!({"id":id,"target":target,"content_digest":content_digest}))
            } else {
                None
            };
            return Ok(response(Ok(
                serde_json::json!({"receipt":receipt,"pending":pending,"restores_permissions":false}),
            )));
        }
        let result = match query {
            Query::Read { id } => store.read(id),
            Query::ReadOrRetire { id, target, content_digest } => {
                if let Some((actual, _, digest)) = self.active_approval(ws.root(), &id.to_string()).await
                    && (actual != target || digest != content_digest) {
                    return Ok(response(Err(fault("identity_conflict", "Approval reconciliation target or content changed; the pending request was kept"))))
                }
                let record = store.read_or_retire(id, &target, &content_digest);
                if let Ok(receipt) = &record
                    && receipt.phase == Phase::Retired
                    && let Some((actual, broker, digest)) = self.active_approval(ws.root(), &id.to_string()).await
                    && actual == target && digest == content_digest {
                    broker.withdraw(&id.to_string()); ws.db.notify_changes();
                }
                record.map(Some)
            },
            Query::Capabilities | Query::Describe { .. } => unreachable!(),
        }.map(|receipt| serde_json::json!({"receipt":receipt,"restores_permissions":false}));
        Ok(response(result))
    }
    pub(crate) async fn respond_approval(
        &self,
        request: Request<ApprovalDecisionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let (decision, scope) = parse_decision(&req.decision)
            .ok_or_else(|| Status::invalid_argument("invalid approval decision"))?;
        if !self
            .state
            .acl_store
            .read()
            .unwrap()
            .is_allowed(&subject, "/metteur.Daemon/RespondApproval")
        {
            return Err(Status::permission_denied("Current approval permission is required"));
        }
        let id = Uuid::parse_str(&req.request_id)
            .map_err(|_| Status::invalid_argument("A host approval UUID is required"))?;
        let protected = !req.target_json.is_empty() || !req.content_digest.is_empty();
        let expected: Option<Target> = if protected {
            Some(
                serde_json::from_str(&req.target_json)
                    .map_err(|_| Status::invalid_argument("Invalid approval target"))?,
            )
        } else {
            None
        };
        if expected.as_ref().is_some_and(|target| !target.valid())
            || (protected && req.content_digest.len() != 64)
        {
            return Err(Status::invalid_argument(
                "Exact approval target and content digest are required",
            ));
        }
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let store = ApprovalDelivery::new(ws.db.clone(), &subject).map_err(receipt_error)?;
        if let Some(record) = store.read(id).map_err(receipt_error)? {
            if record.phase == Phase::Retired {
                return Err(Status::failed_precondition("Approval identity is retired"));
            }
            if record.decision.as_deref() != Some(req.decision.as_str())
                || expected.as_ref().is_some_and(|target| *target != record.target)
                || (protected && req.content_digest != record.content_digest)
            {
                return Err(Status::failed_precondition(
                    "Approval identity has a different decision, target or content",
                ));
            }
            return if record.phase == Phase::Accepted {
                Ok(Response::new(Empty {}))
            } else {
                Err(Status::failed_precondition(
                    "Approval outcome is unconfirmed; query the original receipt without replaying",
                ))
            };
        }
        let (target, broker, content_digest) = self
            .active_approval(ws.root(), &req.request_id)
            .await
            .ok_or_else(|| Status::not_found("Approval request is no longer pending"))?;
        if expected.as_ref().is_some_and(|expected| *expected != target)
            || (protected && req.content_digest != content_digest)
        {
            return Err(Status::failed_precondition(
                "Approval target or content changed; no decision was applied",
            ));
        }
        let (record, fresh) =
            store.claim(id, &target, &content_digest, &req.decision).map_err(receipt_error)?;
        if !fresh {
            return if record.phase == Phase::Accepted {
                Ok(Response::new(Empty {}))
            } else {
                Err(Status::failed_precondition(
                    "Approval already claimed; inspect its original receipt",
                ))
            };
        }
        let grants = GrantStore::new(Some(ws.db.clone()), self.state.global_db.clone());
        let outcome = broker.respond(&req.request_id, decision, scope, &grants);
        store.finish(id, outcome.is_ok()).map_err(receipt_error)?;
        ws.db.notify_changes();
        outcome.map_err(super::to_status)?;
        Ok(Response::new(Empty {}))
    }
}
fn receipt_error(error: metteur_shared::resources::ResourceError) -> Status {
    Status::failed_precondition(error.to_string())
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
