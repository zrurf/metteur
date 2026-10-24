//! Supervision receipt inspection never retries intake or calls a model.
use super::super::proto::{ResourceRequest, ResourceResponse};
use super::{DaemonService, resources::response};
use crate::oversight::conversation;
use crate::storage::resources::fault;
use serde::Deserialize;
use tonic::{Request, Response, Status};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    Capabilities,
    ReconcilePending,
    Read {
        run: Uuid,
        conversation: Uuid,
        id: Uuid,
    },
    ReadOrRetire {
        run: Uuid,
        conversation: Uuid,
        id: Uuid,
    },
}
impl DaemonService {
    pub(crate) async fn concierge_delivery(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 4096 {
            return Ok(response(Err(fault("invalid_request", "Unsupported supervision query"))));
        }
        let query: Query = match serde_json::from_str(&req.payload_json) {
            Ok(query) => query,
            Err(_) => {
                return Ok(response(Err(fault("invalid_request", "Invalid supervision query"))));
            }
        };
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace not open"))?;
        if matches!(query, Query::Capabilities) {
            return Ok(response(Ok(
                serde_json::json!({"schema":1,"durable_intake":true,"read_or_retire":true,"pending_reconciliation":true,"automatic_replay":false,"max_inputs":8192,"bytes":16*1024*1024}),
            )));
        }
        if !self
            .state
            .acl_store
            .read()
            .unwrap()
            .is_allowed(&subject, "/metteur.Daemon/GetConciergeState")
        {
            return Ok(response(Err(fault(
                "permission_denied",
                "Current supervision read permission is required",
            ))));
        }
        if matches!(query, Query::ReconcilePending) {
            let result = (|| -> crate::storage::resources::Result<serde_json::Value> {
                let recovery = crate::storage::recovery::RecoveryStore::new(
                    ws.db.clone(),
                    &format!("project:{}", ws.root().to_string_lossy()),
                    &subject,
                )?;
                let mut items = Vec::new();
                for (run, id) in recovery.pending_supervision()? {
                    items.push(
                        conversation::input_receipt(&ws.db, &subject, run, run, id, true).map_err(
                            |_| {
                                fault(
                                    "input_unconfirmed",
                                    "A pending supervision identity needs review",
                                )
                            },
                        )?,
                    );
                }
                Ok(serde_json::json!({"items":items,"raw_copies_unchanged":true}))
            })();
            return Ok(response(result));
        }
        let (run, conversation, id, retire) = match query {
            Query::Read {
                run,
                conversation,
                id,
            } => (run, conversation, id, false),
            Query::ReadOrRetire {
                run,
                conversation,
                id,
            } => (run, conversation, id, true),
            Query::Capabilities | Query::ReconcilePending => unreachable!(),
        };
        let result = conversation::input_receipt(&ws.db, &subject, run, conversation, id, retire)
            .map_err(|_| {
                fault(
                    "input_unconfirmed",
                    "Supervision input needs reconciliation; keep its identity",
                )
            })
            .and_then(|receipt| {
                let source_available = crate::execution::DbCheckpointSink::load(&ws.db, run)
                    .map_err(|_| {
                        fault("storage_unavailable", "Supervision source could not be checked")
                    })?
                    .is_some();
                Ok(serde_json::json!({"receipt":receipt,"source_available":source_available}))
            });
        Ok(response(result))
    }
}
