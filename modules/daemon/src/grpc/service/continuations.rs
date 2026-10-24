//! Authenticated task controls and host event dispatch. No model-facing event API.
use super::super::proto::{ResourceRequest, ResourceResponse, SendChatRequest};
use super::{AppState, DaemonService, to_status};
use crate::{
    continuation::{
        state::{TaskState, Terminal},
        store::{ContinuationStore, TaskHandle},
    },
    error::DaemonError,
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    PrepareRebind { id: Uuid },
    List {
        after: Option<Uuid>,
    },
    Read {
        id: Uuid,
    },
    Result {
        id: Uuid,
        source: String,
        offset: usize,
    },
    CaptureResult {
        reference: super::result_references::ResultReference,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    id: Uuid,
    action: String,
    #[serde(default)]
    rebind_digest: Option<String>,
}
fn response(value: serde_json::Value) -> Response<ResourceResponse> {
    Response::new(ResourceResponse {
        schema_version: 1,
        payload_json: value.to_string(),
        error_json: String::new(),
    })
}
fn decode<T: serde::de::DeserializeOwned>(req: &ResourceRequest) -> Result<T, Status> {
    if req.schema_version != 1 || req.payload_json.len() > 32768 {
        return Err(Status::invalid_argument("Invalid continuation envelope"));
    }
    serde_json::from_str(&req.payload_json)
        .map_err(|_| Status::invalid_argument("Invalid continuation fields"))
}
fn projection(r: &crate::continuation::store::TaskRecord) -> serde_json::Value {
    serde_json::json!({"revision":r.revision,"task":r.task,"policy":r.policy,"schedules":r.schedules,"has_saved_context":r.saved_context.is_some(),"launch_intent":r.launch_intent,"rebind_receipt":r.rebind_receipt})
}
impl DaemonService {
    pub(crate) async fn query_continuations(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        let store = ContinuationStore::inspect(ws.db.clone(), ws.root()).map_err(to_status)?;
        let live = self.state.chats.read().await.get(ws.root()).cloned();
        let project =
            |record: &crate::continuation::store::TaskRecord| -> Result<serde_json::Value, Status> {
                let mut value = projection(record);
                let current_revision =
                    ws.db.chat_revision(record.task.session.as_bytes()).map_err(to_status)?;
                let may_send = self
                    .state
                    .acl_store
                    .read()
                    .unwrap()
                    .is_allowed(&subject, "/metteur.Daemon/SendChat");
                if let Some(chat) = &live {
                    if chat.session_id == Some(record.task.session) {
                        if let Some(broker) = &chat.approvals {
                            let details = chat.approval_details.lock();
                            value["pending_approvals"] = serde_json::json!(
                            broker
                                .pending_ids()
                                .into_iter()
                                .filter_map(|id| details.get(&id).map(
                                    |detail| serde_json::json!({"request_id":id,"detail":detail,"content_digest":crate::storage::approval_delivery::ApprovalDelivery::content_digest(detail),"target":{"kind":"chat","session":chat.session_id,"turn":chat.turn_id,"run":null}})
                                ))
                                .collect::<Vec<_>>()
                        );
                        }
                    }
                }
                value["workflow"] = crate::continuation::presentation::workflow_with_approval(
                    record,
                    current_revision,
                    may_send,
                    now_ms(),
                    value["pending_approvals"].as_array().is_some_and(|items| !items.is_empty()),
                );
                Ok(value)
            };
        let value = match decode::<Query>(&req)? {
            Query::PrepareRebind { id } => {
                let _gate = ws.admit().await.map_err(to_status)?;
                if self.state.chats.read().await.contains_key(ws.root()) || self.state.running.read().await.contains_key(ws.root()) {
                    return Err(Status::failed_precondition("Finish the foreground execution before reviewing a new binding"));
                }
                let r = store.get(id).map_err(to_status)?.ok_or_else(|| Status::not_found("Task not found"))?;
                if r.owner_subject != subject { return Err(Status::permission_denied("Task owner mismatch")); }
                let context = crate::chat::session::load_thread(&ws.db, &r.task.session).map_err(to_status)?
                    .ok_or_else(|| Status::not_found("Original conversation is unavailable"))?;
                let registry = self.state.registry_for(Some(ws.root()), true).await?;
                let config = ws.config.read().await;
                let digest = crate::continuation::policy::binding_digest(&config, r.model_options.get("model").and_then(|v| v.as_str()), &registry);
                crate::continuation::rebind::proposal(&r, ws.db.chat_revision(r.task.session.as_bytes()).map_err(to_status)?, &context, &digest,
                    self.state.acl_store.read().unwrap().is_allowed(&subject, "/metteur.Daemon/SendChat"), now_ms()).map_err(to_status)?
            }
            Query::List {
                after,
            } => {
                let page = store.list(after).map_err(to_status)?;
                let next = if page.len() == 50 {
                    page.last().map(|r| r.task.id)
                } else {
                    None
                };
                let items = page
                    .iter()
                    .filter(|r| r.owner_subject == subject)
                    .map(&project)
                    .collect::<Result<Vec<_>, _>>()?;
                serde_json::json!({"items":items,"next":next})
            }
            Query::Read {
                id,
            } => {
                let r = store
                    .get(id)
                    .map_err(to_status)?
                    .ok_or_else(|| Status::not_found("Task not found"))?;
                if r.owner_subject != subject {
                    return Err(Status::permission_denied("Task owner mismatch"));
                }
                project(&r)?
            }
            Query::Result {
                id,
                source,
                offset,
            } => {
                let r = store
                    .get(id)
                    .map_err(to_status)?
                    .ok_or_else(|| Status::not_found("Task not found"))?;
                if r.owner_subject != subject {
                    return Err(Status::permission_denied("Task owner mismatch"));
                }
                store.read_result_artifact(id, &source, offset).map_err(to_status)?
            }
            Query::CaptureResult {
                reference,
            } => {
                let (reference, content) = self.result_document(&ws, &subject, &reference, true)?;
                serde_json::json!({"reference":reference,"content":content,"origin":"source_data"})
            }
        };
        Ok(response(value))
    }
    pub(crate) async fn control_continuation(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let input: Control = decode(&req)?;
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        // Policy controls only mutate the existing task ledger. Stop must not
        // wait behind the execution it is meant to cancel.
        let store = ContinuationStore::new(ws.db.clone(), ws.root()).map_err(to_status)?;
        if input.action == "rebind" {
            let _gate = ws.admit().await.map_err(to_status)?;
            let request_id = Uuid::parse_str(&req.request_id).map_err(|_| Status::invalid_argument("Rebind request UUID required"))?;
            if request_id.is_nil() { return Err(Status::invalid_argument("Rebind request UUID required")); }
            let digest = input.rebind_digest.as_deref().filter(|v| v.len() == 64)
                .ok_or_else(|| Status::invalid_argument("Review a concrete rebind proposal first"))?;
            let record = store.get(input.id).map_err(to_status)?.ok_or_else(|| Status::not_found("Task not found"))?;
            if record.owner_subject != subject { return Err(Status::permission_denied("Task owner mismatch")); }
            if let Some((id, previous)) = &record.rebind_receipt {
                if *id == request_id {
                    if previous != digest { return Err(Status::failed_precondition("Rebind request identity conflict")); }
                    return Ok(response(projection(&record)));
                }
            }
            if self.state.chats.read().await.contains_key(ws.root()) || self.state.running.read().await.contains_key(ws.root())
                || super::foreground::unresolved_writes(&ws.db, ws.root()).map_err(to_status)? {
                return Err(Status::failed_precondition("Workspace is occupied or has unresolved write effects"));
            }
            let context = crate::chat::session::load_thread(&ws.db, &record.task.session).map_err(to_status)?
                .ok_or_else(|| Status::not_found("Original conversation is unavailable"))?;
            let registry = self.state.registry_for(Some(ws.root()), true).await?;
            let config = ws.config.read().await;
            let config_digest = crate::continuation::policy::binding_digest(&config, record.model_options.get("model").and_then(|v| v.as_str()), &registry);
            let r = store.update(input.id, req.expected_revision, |r| {
                let current = ws.db.chat_revision(r.task.session.as_bytes())?;
                let proposal = crate::continuation::rebind::proposal(r, current, &context, &config_digest,
                    self.state.acl_store.read().unwrap().is_allowed(&subject, "/metteur.Daemon/SendChat"), now_ms())?;
                if proposal["digest"].as_str() != Some(digest) {
                    return Err(DaemonError::Execution("Rebind proposal changed; review it again".into()));
                }
                crate::continuation::rebind::apply(r, current, context, now_ms());
                r.rebind_receipt = Some((request_id, digest.to_owned()));
                Ok(())
            }).map_err(to_status)?;
            let mut value = projection(&r);
            value["workflow"] = crate::continuation::presentation::workflow(&r, r.task.expected_revision, true, now_ms());
            return Ok(response(value));
        }
        let r = store
            .update(input.id, req.expected_revision, |r| {
                if r.owner_subject != subject {
                    return Err(DaemonError::PermissionDenied("Task owner mismatch".into()));
                }
                match input.action.as_str() {
                    "stop" => {
                        r.task
                            .cancel_timer_sources("task_stopped", now_ms())
                            .map_err(|e| DaemonError::Execution(e.into()))?;
                        if r.task.state != TaskState::Stopped {
                            r.task.stop();
                        }
                        for s in &mut r.schedules {
                            s.cancel();
                        }
                    }
                    "cancel_schedule" => {
                        r.task
                            .cancel_timer_sources("schedule_cancelled", now_ms())
                            .map_err(|e| DaemonError::Execution(e.into()))?;
                        for schedule in &mut r.schedules {
                            schedule.cancel();
                        }
                        if !matches!(
                            r.task.state,
                            TaskState::Active | TaskState::Stopped | TaskState::Completed
                        ) {
                            r.task.wait = None;
                            r.task.state = if r.task.dispatch.as_ref().is_some_and(|d| {
                                matches!(
                                    d.state,
                                    crate::continuation::state::DispatchState::Started
                                        | crate::continuation::state::DispatchState::InDoubt
                                )
                            }) {
                                TaskState::NeedsAttention
                            } else {
                                TaskState::Completed
                            };
                        }
                    }
                    "pause" => {
                        if matches!(
                            r.task.state,
                            TaskState::Stopped | TaskState::Completed | TaskState::NeedsAttention
                        ) {
                            return Err(DaemonError::Execution(
                                "Task has ended or requires explicit recovery".into(),
                            ));
                        }
                        r.task.paused = true;
                        if r.task.state != TaskState::Active {
                            r.task.state = TaskState::Paused;
                        }
                    }
                    "resume" => {
                        let current = ws.db.chat_revision(r.task.session.as_bytes())?;
                        let may_send = self
                            .state
                            .acl_store
                            .read()
                            .unwrap()
                            .is_allowed(&subject, "/metteur.Daemon/SendChat");
                        let facts = crate::continuation::presentation::workflow(
                            r,
                            current,
                            may_send,
                            now_ms(),
                        );
                        if !facts["allowed_actions"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v == "resume"))
                        {
                            return Err(DaemonError::Execution(format!(
                                "Automatic continuation remains blocked: {}",
                                facts["reason"]
                            )));
                        }
                        if matches!(
                            r.task.state,
                            TaskState::Stopped | TaskState::Completed | TaskState::NeedsAttention
                        ) {
                            return Err(DaemonError::Execution(
                                "Task requires a new explicit plan".into(),
                            ));
                        }
                        if ws.db.chat_revision(r.task.session.as_bytes())?
                            != r.task.expected_revision
                        {
                            return Err(DaemonError::Execution(
                                "Session changed; automatic continuation remains blocked".into(),
                            ));
                        }
                        r.task.paused = false;
                        r.task.reconcile(now_ms());
                    }
                    _ => return Err(DaemonError::Execution("Unknown task control".into())),
                }
                Ok(())
            })
            .map_err(to_status)?;
        if input.action == "stop" {
            ws.jobs.kill_task(input.id);
            if let Some(run) = self.state.running.read().await.get(ws.root()) {
                if run.continuation_task == Some(input.id) {
                    run.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(broker) = &run.approvals {
                        broker.close();
                    }
                }
            }

            if let Some(chat) = self.state.chats.read().await.get(ws.root()) {
                if chat.continuation_task == Some(input.id)
                    && chat.session_id == Some(r.task.session)
                {
                    chat.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                    if let Some(b) = &chat.approvals {
                        b.close();
                    }
                    if let Some(bus) = &chat.interrupt_bus {
                        bus.send(crate::execution::interrupt::Interrupt {
                            priority: crate::execution::interrupt::InterruptPriority::Emergency,
                            message: "Task stopped by user".into(),
                        });
                    }
                }
            }
        }
        let mut value = projection(&r);
        value["workflow"] = crate::continuation::presentation::workflow(
            &r,
            ws.db.chat_revision(r.task.session.as_bytes()).map_err(to_status)?,
            self.state.acl_store.read().unwrap().is_allowed(&subject, "/metteur.Daemon/SendChat"),
            now_ms(),
        );
        Ok(response(value))
    }
}
pub(super) fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

pub(super) async fn start_monitor(state: Arc<AppState>, root: PathBuf) {
    let token = Uuid::new_v4();
    {
        let mut workers = state.continuation_workers.lock().await;
        if workers.contains_key(&root) {
            return;
        }
        workers.insert(root.clone(), token);
    }
    let Some(workspace) = state.workspaces.get(&root).await else {
        return;
    };
    let mut changes = workspace.db.subscribe_changes();
    drop(workspace);
    tokio::spawn(async move {
        let mut relative_clocks: std::collections::HashMap<
            Uuid,
            (u64, std::time::Instant, std::time::Duration),
        > = std::collections::HashMap::new();
        let mut next_deadline = Some(now_ms());
        loop {
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { break; },
                _ = async {
                    if let Some(deadline) = next_deadline {
                        tokio::time::sleep(std::time::Duration::from_millis(deadline.saturating_sub(now_ms()))).await;
                    } else { std::future::pending::<()>().await; }
                } => {},
            }
            changes.borrow_and_update();
            next_deadline = None;
            if state.continuation_workers.lock().await.get(&root) != Some(&token) {
                break;
            }
            let Some(ws) = state.workspaces.get(&root).await else {
                break;
            };
            let store = match ContinuationStore::new(ws.db.clone(), ws.root()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Continuation store unavailable: {e}");
                    break;
                }
            };
            let mut after = None;
            loop {
                let page = match store.list(after) {
                    Ok(p) => p,
                    Err(_) => break,
                };
                if page.is_empty() {
                    break;
                }
                for record in page {
                    after = Some(record.task.id);
                    let now = now_ms();
                    let candidates = record
                        .policy
                        .iter()
                        .map(|p| p.expires_ms)
                        .chain(record.task.wait.iter().map(|w| w.deadline_ms))
                        .chain(record.schedules.iter().filter_map(|s| {
                            s.next_due.map(|d| d.timestamp_millis().max(0) as u64)
                        }));
                    for deadline in candidates.filter(|deadline| *deadline > now) {
                        next_deadline = Some(
                            next_deadline.map_or(deadline, |previous: u64| previous.min(deadline)),
                        );
                    }
                    if record.task.state == TaskState::Stopped {
                        ws.jobs.kill_task(record.task.id);
                        continue;
                    }
                    if !matches!(
                        record.task.state,
                        TaskState::Waiting | TaskState::Ready | TaskState::Paused
                    ) {
                        continue;
                    }
                    match super::launches::poll_launch(&state, &ws, &record).await {
                        Ok(true) => continue,
                        Err(error) => {
                            tracing::warn!("Launch reconciliation failed: {error}");
                            continue;
                        }
                        Ok(false) => {}
                    }
                    let result=store.update(record.task.id,record.revision,|r|{
      if r.task.budget.unknown_usage || r.task.budget.used_turns >= r.task.budget.max_turns || r.task.budget.consumed_tokens.saturating_add(r.task.budget.reserved_tokens) >= r.task.budget.token_limit {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Continuation budget exhausted or usage requires reconciliation".into());return Ok(());}
      if r.policy.as_ref().is_none_or(|p|p.expires_ms<=now_ms()) {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Continuation policy expired".into());return Ok(());}
      if ws.db.chat_revision(r.task.session.as_bytes())?!=r.task.expected_revision {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("A newer session revision superseded this wait".into());return Ok(());}
      if !state.acl_store.read().unwrap().is_allowed(&r.owner_subject,"/metteur.Daemon/SendChat") {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Continuation permission revoked".into());return Ok(());}
      // Reconcile persisted job identities; no PID attachment or process replay.
      let ids:Vec<_>=r.task.sources.values().filter(|s|s.kind=="job"&&s.terminal.is_none()).map(|s|s.id.clone()).collect();
      for id in ids {
       if ws.jobs.snapshot(&id).is_none() {r.task.finish_source(&id,Terminal::Indeterminate,serde_json::json!({"reason":"host_process_unavailable"}),now_ms()).map_err(|e|DaemonError::Execution(e.into()))?;}
      }
      if !r.task.paused {
       for schedule in &mut r.schedules {
        if schedule.task_generation != r.task.generation {continue;}
        let source_id=format!("timer:{}:{}:{}",schedule.id,schedule.generation,schedule.occurrences);
        if !r.task.sources.contains_key(&source_id){continue;}
        let mut effective_now=chrono::Utc::now();
        if let crate::continuation::timer::Rule::After{deadline}=&schedule.rule {
            let clock=relative_clocks.entry(schedule.id).or_insert_with(||(schedule.generation,std::time::Instant::now(),deadline.signed_duration_since(effective_now).to_std().unwrap_or_default()));
            if clock.0!=schedule.generation{*clock=(schedule.generation,std::time::Instant::now(),deadline.signed_duration_since(effective_now).to_std().unwrap_or_default());}
            if clock.1.elapsed()<clock.2 {continue;}
            effective_now=effective_now.max(*deadline);
        }
        if let Some(occurrence)=schedule.advance(effective_now).map_err(|e|DaemonError::Execution(e.into()))? {
         r.task.finish_source(&source_id,Terminal::Succeeded,serde_json::json!({"occurrence_id":occurrence.id,"due":occurrence.due,"missed_count":occurrence.missed_count}),now_ms()).map_err(|e|DaemonError::Execution(e.into()))?;
        }
       }
      }
      if r.task.sources.values().any(|s|s.may_write && matches!(s.terminal,Some(Terminal::Indeterminate|Terminal::Interrupted|Terminal::Cancelled))) {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Unresolved background write effects require review".into());}
      else {r.task.reconcile(now_ms());}Ok(())
     });
                    let Ok(record) = result else {
                        continue;
                    };
                    if record.task.state != TaskState::Ready {
                        continue;
                    }
                    if state.chats.read().await.contains_key(&root)
                        || state.running.read().await.contains_key(&root)
                    {
                        continue;
                    }
                    let service = DaemonService::new(state.clone());
                    let mut options = record.model_options.clone();
                    if let Some(o) = options.as_object_mut() {
                        o.insert("permission_mode".into(), serde_json::json!("ask"));
                    }
                    let request = SendChatRequest {
                        workspace_path: root.to_string_lossy().into_owned(),
                        session_id: record.task.session.to_string(),
                        message: String::new(),
                        history_json: String::new(),
                        options_json: options.to_string(),
                    };
                    let handle = TaskHandle {
                        store: store.clone(),
                        id: record.task.id,
                        generation: record.task.generation,
                    };
                    match service
                        .start_chat_turn(request, record.owner_subject.clone(), Some(handle))
                        .await
                    {
                        Ok(response) => {
                            let mut stream = response.into_inner();
                            tokio::spawn(async move {
                                use tokio_stream::StreamExt;
                                while stream.next().await.is_some() {}
                            });
                        }
                        Err(error) => {
                            tracing::debug!("Continuation admission deferred: {error}");
                        }
                    }
                }
            }
        }
        let mut workers = state.continuation_workers.lock().await;
        if workers.get(&root) == Some(&token) {
            workers.remove(&root);
        }
    });
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ScheduleInput {
    task_id: Uuid,
    id: Uuid,
    rule: crate::continuation::timer::Rule,
    max_occurrences: u32,
    expires_at: chrono::DateTime<chrono::Utc>,
    misfire: crate::continuation::timer::Misfire,
    params: serde_json::Value,
    confirmed_digest: Option<String>,
}
impl DaemonService {
    pub(crate) async fn write_schedule(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let subject =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let mut input: ScheduleInput = decode(&req)?;
        let ws = self
            .state
            .workspaces
            .get(Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace is not open"))?;
        let _gate = ws.admit().await.map_err(to_status)?;
        if self.state.chats.read().await.contains_key(ws.root())
            || self.state.running.read().await.contains_key(ws.root())
        {
            return Err(Status::failed_precondition(
                "Wait for the current foreground turn before editing a schedule",
            ));
        }
        let confirmation = input.confirmed_digest.take();
        let digest = crate::storage::resources::digest(
            &serde_json::to_vec(&(&input, req.expected_revision, &subject))
                .map_err(|_| Status::invalid_argument("Invalid plan"))?,
        );
        let store = ContinuationStore::new(ws.db.clone(), ws.root()).map_err(to_status)?;
        let record = store
            .get(input.task_id)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("Task not found"))?;
        if record.owner_subject != subject {
            return Err(Status::permission_denied("Task owner mismatch"));
        }
        if record.revision != req.expected_revision {
            return Err(Status::failed_precondition("Task revision changed"));
        }
        if !matches!(record.task.state, TaskState::Waiting | TaskState::Paused | TaskState::Ready)
            || record.saved_context.is_none()
        {
            return Err(Status::failed_precondition("Schedule requires a saved waiting task"));
        }
        if crate::execution::blackboard::safe_value(&input.params, 0) != input.params {
            return Err(Status::invalid_argument(
                "Credentials are not permitted in schedule parameters",
            ));
        }
        let now = chrono::Utc::now();
        let next = input
            .rule
            .next_after(now)
            .map_err(Status::invalid_argument)?
            .ok_or_else(|| Status::invalid_argument("Schedule has no future occurrence"))?;
        if input.expires_at.timestamp_millis().max(0) as u64
            > record.policy.as_ref().map_or(0, |p| p.expires_ms)
        {
            return Err(Status::invalid_argument("Schedule exceeds task authorization expiry"));
        }
        if input.max_occurrences
            > record.task.budget.max_turns.saturating_sub(record.task.budget.used_turns)
        {
            return Err(Status::invalid_argument(
                "Occurrences exceed the remaining authorized turns",
            ));
        }
        let old = record.schedules.iter().find(|s| s.id == input.id);
        let schedule = crate::continuation::timer::Schedule {
            id: input.id,
            generation: old.map_or(1, |s| s.generation.saturating_add(1)),
            task: record.task.id,
            task_generation: record.task.generation,
            rule: input.rule.clone(),
            next_due: Some(next),
            last_emitted: None,
            max_occurrences: input.max_occurrences,
            occurrences: 0,
            expires_at: input.expires_at,
            paused: false,
            cancelled: false,
            confirmed_digest: digest.clone(),
            misfire: input.misfire,
            params: input.params.clone(),
        };
        schedule.validate().map_err(Status::invalid_argument)?;
        if confirmation.as_deref() != Some(&digest) {
            return Ok(response(
                serde_json::json!({"confirmation_required":true,"digest":digest,"plan":schedule,"replaces_wait":true}),
            ));
        }
        let result = store
            .update(record.task.id, record.revision, |r| {
                if r.schedules.iter().any(|s| s.id != input.id && !s.cancelled) {
                    return Err(DaemonError::Execution(
                        "Only one active schedule per task is supported".into(),
                    ));
                }
                r.task
                    .cancel_timer_sources("schedule_replaced", now_ms())
                    .map_err(|e| DaemonError::Execution(e.into()))?;
                r.schedules.retain(|s| s.id != input.id);
                r.schedules.push(schedule.clone());
                let id = format!(
                    "timer:{}:{}:{}",
                    schedule.id, schedule.generation, schedule.occurrences
                );
                r.task.sources.insert(
                    id.clone(),
                    crate::continuation::state::Source {
                        id: id.clone(),
                        kind: "timer".into(),
                        task: r.task.id,
                        generation: r.task.generation,
                        may_write: false,
                        terminal: None,
                        result: serde_json::Value::Null,
                    },
                );
                r.task.wait = Some(crate::chat::wait::WaitIntent {
                    sources: vec![id],
                    mode: crate::chat::wait::MatchMode::Any,
                    deadline_ms: input.expires_at.timestamp_millis().max(0) as u64,
                    params: input.params.clone(),
                });
                r.task.state = if r.task.paused {
                    TaskState::Paused
                } else {
                    TaskState::Waiting
                };
                Ok(())
            })
            .map_err(to_status)?;
        Ok(response(projection(&result)))
    }
}
