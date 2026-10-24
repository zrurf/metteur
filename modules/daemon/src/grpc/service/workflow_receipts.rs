//! Queries/fences for existing narrow control RPCs; no control executor here.
use super::{DaemonService, RunningExecution};
use crate::{
    execution::interrupt::{Interrupt, InterruptPriority, OwnedInterrupt},
    storage::{
        approval_delivery::Target,
        resources::{Result, fault},
        workflow_receipts::{Identity, Phase, Receipt, Store},
    },
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Query {
    Capabilities,
    Describe {
        target: Target,
    },
    Prepare {
        identity: Identity,
        target: Target,
        action: String,
        body: Option<String>,
    },
    Read {
        id: Uuid,
    },
    Fence {
        id: Uuid,
        target: Target,
        action: String,
        body: Option<String>,
    },
    List {
        after: Option<Uuid>,
    },
}
pub(super) fn error(e: impl std::fmt::Display) -> tonic::Status {
    tonic::Status::failed_precondition(e.to_string())
}
pub(super) fn identity(
    raw: &str,
    allow_legacy_stop: bool,
) -> std::result::Result<Identity, tonic::Status> {
    if raw.is_empty() && allow_legacy_stop {
        return Ok(Identity {
            id: Uuid::new_v4(),
            revision: 0,
        });
    }
    serde_json::from_str(raw).map_err(|_| {
        tonic::Status::failed_precondition(
            "Receipt-capable client required; retain this control/input identity",
        )
    })
}
pub(super) fn blueprint_target(run: Uuid) -> Target {
    Target {
        kind: "blueprint".into(),
        run: Some(run),
        session: None,
        turn: None,
    }
}
impl DaemonService {
    pub(super) async fn workflow_query(
        &self,
        ws: &crate::workspace::manager::Workspace,
        owner: &str,
        query: Query,
    ) -> Result<serde_json::Value> {
        let store = Store::new(ws.db.clone(), owner)?;
        let normalize = |record: Receipt, live: bool| -> Result<Receipt> {
            if live {
                return Ok(record);
            }
            match record.phase {
                Phase::Queued => store.transition(record.id, Phase::Unconsumed),
                Phase::Claimed => store.transition(record.id, Phase::Unknown),
                _ => Ok(record),
            }
        };
        let active = self.control_target(ws.root()).await;
        let live = |record: &Receipt| match record.target.kind.as_str() {
            "blueprint" => active.run == record.target.run && active.state == "blueprint",
            "chat" => {
                active.session == record.target.session
                    && active.turn == record.target.turn
                    && active.state == "chat"
            }
            _ => false,
        };
        match query {
            Query::Capabilities => Ok(
                serde_json::json!({"schema":1,"durable_controls":true,"durable_blueprint_input":true,"automatic_replay":false,"max_receipts":8192,"body_bytes":16384}),
            ),
            Query::Describe {
                target,
            } => {
                if !target.valid() {
                    return Err(fault("invalid_target", "A concrete target is required"));
                }
                Ok(serde_json::json!({"revision":store.revision(&target)?,"target":target}))
            }
            Query::Prepare {
                identity,
                target,
                action,
                body,
            } => Ok(
                serde_json::json!({"receipt":store.prepare(&identity,&target,&action,body.as_deref())?}),
            ),
            Query::Read {
                id,
            } => {
                let record = store
                    .read(id)?
                    .map(|r| {
                        let current = live(&r);
                        normalize(r, current)
                    })
                    .transpose()?;
                Ok(serde_json::json!({"receipt":record}))
            }
            Query::Fence {
                id,
                target,
                action,
                body,
            } => {
                let record = store.fence(id, &target, &action, body.as_deref())?;
                let current = live(&record);
                Ok(serde_json::json!({"receipt":normalize(record,current)?}))
            }
            Query::List {
                after,
            } => {
                let (records, next) = store.list(after)?;
                let items = records
                    .into_iter()
                    .map(|record| {
                        let current = live(&record);
                        normalize(record, current)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(serde_json::json!({"items":items,"next":next}))
            }
        }
    }
    pub(super) fn blueprint_control(
        &self,
        ws: &crate::workspace::manager::Workspace,
        owner: &str,
        entry: &RunningExecution,
        action: &str,
        raw: &str,
    ) -> std::result::Result<(), tonic::Status> {
        let identity = identity(raw, action == "stop")?;
        let store = Store::new(ws.db.clone(), owner).map_err(error)?;
        let record = store.read(identity.id).map_err(error)?;
        if action != "stop"
            && record.is_none()
            && entry.cancel_requested.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(tonic::Status::failed_precondition(
                "Run is stopping; pause/resume cannot reverse Stop",
            ));
        }
        store
            .control(&identity, &blueprint_target(entry.run_id), action, || {
                if action == "stop" {
                    entry.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(broker) = &entry.approvals {
                        broker.close();
                    }
                } else {
                    entry
                        .pause_requested
                        .store(action == "pause", std::sync::atomic::Ordering::SeqCst);
                }
            })
            .map_err(error)?;
        if action == "stop" {
            crate::oversight::recovery::mark_direct_cancel(&ws.db, entry.run_id)
                .map_err(super::to_status)?;
        }
        ws.db.notify_changes();
        Ok(())
    }
}
struct InputOwner {
    store: Store,
    id: Uuid,
}
impl OwnedInterrupt for InputOwner {
    fn claim(&self) -> crate::error::DaemonResult<()> {
        self.store
            .transition(self.id, Phase::Claimed)
            .map(|_| ())
            .map_err(|e| crate::DaemonError::Persistence(e.to_string()))
    }
    fn unconsumed(&self) -> crate::error::DaemonResult<()> {
        self.store
            .transition(self.id, Phase::Unconsumed)
            .map(|_| ())
            .map_err(|e| crate::DaemonError::Persistence(e.to_string()))
    }
    fn applied(&self, persisted: bool) -> crate::error::DaemonResult<()> {
        self.store
            .transition(
                self.id,
                if persisted {
                    Phase::Applied
                } else {
                    Phase::Unknown
                },
            )
            .map(|_| ())
            .map_err(|e| crate::DaemonError::Persistence(e.to_string()))
    }
}
pub(super) fn temporary_input(
    ws: &crate::workspace::manager::Workspace,
    owner: &str,
    entry: &RunningExecution,
    raw: &str,
    body: String,
) -> std::result::Result<(), tonic::Status> {
    let identity = identity(raw, false)?;
    let store = Store::new(ws.db.clone(), owner).map_err(error)?;
    let target = blueprint_target(entry.run_id);
    if let Some(record) = store.read(identity.id).map_err(error)? {
        if record.target != target
            || record.body.as_deref() != Some(body.as_str())
            || record.operation != "temporary_input"
            || record.phase == Phase::Retired
        {
            return Err(tonic::Status::failed_precondition(
                "Input identity is closed or has different content",
            ));
        }
        if record.phase != Phase::Prepared {
            return Ok(());
        }
    }
    if entry.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(tonic::Status::failed_precondition(
            "Targeted run is stopping; retain the input",
        ));
    }
    let bus = entry
        .interrupt_bus
        .as_ref()
        .ok_or_else(|| tonic::Status::failed_precondition("Target cannot receive input"))?;
    let owner = Arc::new(InputOwner {
        store: store.clone(),
        id: identity.id,
    });
    bus.send_owned(
        Interrupt {
            priority: InterruptPriority::Urgent,
            message: body.clone(),
        },
        owner,
        || {
            store
                .queue(&identity, &target, &body)
                .map_err(|e| crate::DaemonError::Persistence(e.to_string()))
        },
    )
    .map_err(super::to_status)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc::proto::{
        CancelRequest, InterruptRequest, PauseRequest, ResourceRequest, ResumeRequest,
        daemon_server::Daemon,
    };
    use crate::{Registry, WorkspaceManager};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tonic::Request;
    async fn setup() -> (
        DaemonService,
        Arc<super::super::AppState>,
        Arc<crate::workspace::manager::Workspace>,
        RunningExecution,
    ) {
        let root = std::env::temp_dir().join(format!("dw-workflow-rpc-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = Arc::new(super::super::AppState::new(
            WorkspaceManager::new().with_global_config_path(root.join(".metteur/global.toml")),
            Arc::new(Registry::with_builtins()),
            metteur_shared::config::Config::default(),
        ));
        let ws = state.workspaces.open(&root).await.unwrap();
        let run = RunningExecution {
            approval_details: Arc::new(parking_lot::Mutex::new(Default::default())),
            blueprint_id: Uuid::new_v4(),
            continuation_task: None,
            run_id: Uuid::new_v4(),
            interrupt_bus: Some(crate::execution::interrupt::InterruptBus::new()),
            pause_requested: Arc::new(AtomicBool::new(false)),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            approvals: None,
        };
        state.running.write().await.insert(ws.root().into(), run.clone());
        (DaemonService::new(state.clone()), state, ws, run)
    }
    fn receipt(id: Uuid, revision: u64) -> String {
        serde_json::json!({"id":id,"revision":revision}).to_string()
    }
    async fn query(
        service: &DaemonService,
        ws: &crate::workspace::manager::Workspace,
        action: serde_json::Value,
    ) -> serde_json::Value {
        let response = Daemon::chat_delivery(
            service,
            Request::new(ResourceRequest {
                workspace_path: ws.root().to_string_lossy().into(),
                schema_version: 1,
                payload_json: serde_json::json!({"operation":"workflow","action":action})
                    .to_string(),
                ..Default::default()
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(response.error_json.is_empty(), "{}", response.error_json);
        serde_json::from_str(&response.payload_json).unwrap()
    }
    #[tokio::test]
    async fn narrow_controls_deduplicate_order_and_stop_without_waiting_for_admission() {
        let (service, _, ws, run) = setup().await;
        let path = ws.root().to_string_lossy().to_string();
        let id = Uuid::new_v4();
        let _busy = ws.admit().await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            query(
                &service,
                &ws,
                serde_json::json!({"operation":"describe","target":blueprint_target(run.run_id)}),
            ),
        )
        .await
        .unwrap();
        Daemon::pause_execution(
            &service,
            Request::new(PauseRequest {
                workspace_path: path.clone(),
                run_id: run.run_id.to_string(),
                receipt_json: receipt(id, 0),
            }),
        )
        .await
        .unwrap();
        assert!(run.pause_requested.load(Ordering::SeqCst));
        Daemon::resume_execution(
            &service,
            Request::new(ResumeRequest {
                workspace_path: path.clone(),
                run_id: run.run_id.to_string(),
                receipt_json: receipt(Uuid::new_v4(), 1),
            }),
        )
        .await
        .unwrap();
        Daemon::pause_execution(
            &service,
            Request::new(PauseRequest {
                workspace_path: path.clone(),
                run_id: run.run_id.to_string(),
                receipt_json: receipt(id, 0),
            }),
        )
        .await
        .unwrap();
        assert!(!run.pause_requested.load(Ordering::SeqCst));
        assert!(
            Daemon::pause_execution(
                &service,
                Request::new(PauseRequest {
                    workspace_path: path.clone(),
                    run_id: run.run_id.to_string(),
                    receipt_json: receipt(Uuid::new_v4(), 0)
                })
            )
            .await
            .is_err()
        );
        Daemon::cancel_execution(
            &service,
            Request::new(CancelRequest {
                workspace_path: path.clone(),
                run_id: run.run_id.to_string(),
                receipt_json: receipt(Uuid::new_v4(), 0),
            }),
        )
        .await
        .unwrap();
        assert!(run.cancel_requested.load(Ordering::SeqCst));
        assert!(
            Daemon::resume_execution(
                &service,
                Request::new(ResumeRequest {
                    workspace_path: path,
                    run_id: run.run_id.to_string(),
                    receipt_json: receipt(Uuid::new_v4(), 3)
                })
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn temporary_rpc_has_durable_body_node_consumption_and_end_disposition() {
        let (service, state, ws, run) = setup().await;
        let id = Uuid::new_v4();
        let body = "  original\r\n中文  ";
        let request = InterruptRequest {
            workspace_path: ws.root().to_string_lossy().into(),
            priority: "Urgent".into(),
            message: body.into(),
            target_kind: "blueprint".into(),
            run_id: run.run_id.to_string(),
            receipt_json: receipt(id, 0),
            ..Default::default()
        };
        Daemon::send_interrupt(&service, Request::new(request.clone())).await.unwrap();
        Daemon::send_interrupt(&service, Request::new(request)).await.unwrap();
        let read = query(&service, &ws, serde_json::json!({"operation":"read","id":id})).await;
        assert_eq!(read["receipt"]["body"], body);
        assert_eq!(read["receipt"]["phase"], "queued");
        let bus = run.interrupt_bus.as_ref().unwrap();
        let node = Uuid::new_v4();
        assert_eq!(bus.drain_owned_for(InterruptPriority::Urgent, Some(node)).unwrap(), vec![body]);
        bus.finish_consumer(Some(node), true).unwrap();
        assert_eq!(
            query(&service, &ws, serde_json::json!({"operation":"read","id":id})).await["receipt"]
                ["phase"],
            "applied"
        );
        let pending = Uuid::new_v4();
        let store = Store::new(ws.db.clone(), "local").unwrap();
        store
            .queue(
                &Identity {
                    id: pending,
                    revision: 0,
                },
                &blueprint_target(run.run_id),
                "unconsumed",
            )
            .unwrap();
        state.running.write().await.remove(ws.root());
        assert_eq!(
            query(&service, &ws, serde_json::json!({"operation":"read","id":pending})).await["receipt"]
                ["phase"],
            "unconsumed"
        );
    }
    #[tokio::test]
    async fn failed_stop_persistence_still_cancels_but_receipt_stays_unknown() {
        let (service, _, ws, run) = setup().await;
        let id = Uuid::new_v4();
        ws.db.fail_delivery_write.store(true, Ordering::SeqCst);
        assert!(
            Daemon::cancel_execution(
                &service,
                Request::new(CancelRequest {
                    workspace_path: ws.root().to_string_lossy().into(),
                    run_id: run.run_id.to_string(),
                    receipt_json: receipt(id, 0)
                })
            )
            .await
            .is_err()
        );
        assert!(run.cancel_requested.load(Ordering::SeqCst));
        assert_eq!(query(&service,&ws,serde_json::json!({"operation":"fence","id":id,"target":blueprint_target(run.run_id),"action":"stop","body":null})).await["receipt"]["phase"],"unknown");
    }
}
