//! Launch admission is a domain operation, never a self-RPC under activity_gate.
use super::{
    super::proto::{ResourceRequest, ResourceResponse},
    DaemonService, to_status,
};
use crate::{
    AppState, DaemonError, DaemonResult, Registry,
    continuation::{
        handoff::HandoffStore,
        launch::{LaunchIntent, LaunchState, LaunchStore},
        state::{TaskState, Terminal},
        store::{TaskHandle, TaskRecord},
    },
    execution::{DbCheckpointSink, RunStatus},
    workspace::manager::Workspace,
};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;
#[derive(Clone)]
pub(crate) struct LaunchBinding {
    pub task: TaskHandle,
    pub intent: LaunchIntent,
}
fn now() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}
fn fail(s: impl Into<String>) -> DaemonError {
    DaemonError::Execution(s.into())
}
fn store(ws: &Workspace) -> DaemonResult<LaunchStore> {
    LaunchStore::new(ws.db.clone(), ws.root())
}
pub(super) fn probe(
    state: &AppState,
    ws: &Workspace,
    registry: Arc<Registry>,
    owner: &str,
) -> crate::execution::context::ExecutionContext {
    let mut ctx = crate::execution::context::ExecutionContext::new(
        registry,
        state.llm_factory.clone(),
        ws.root().to_path_buf(),
    )
    .with_user(owner.to_owned())
    .with_config(ws.config.clone());
    ctx.workspace_db = Some(ws.db.clone());
    ctx.global_db = state.global_db.clone();
    ctx.version_manager = Some(ws.version_manager.clone());
    ctx
}
pub(crate) async fn admit_launch(
    state: &AppState,
    ws: &Workspace,
    registry: &Arc<Registry>,
    binding: &LaunchBinding,
    run: Uuid,
) -> Result<(), Status> {
    let record = binding
        .task
        .store
        .get(binding.task.id)
        .map_err(to_status)?
        .ok_or_else(|| Status::not_found("Parent task missing"))?;
    let launches = store(ws).map_err(to_status)?;
    let (_, intent) = launches.get(binding.intent.id, &binding.intent.owner).map_err(to_status)?;
    if intent.state != LaunchState::YieldCommitted
        || record.task.generation != intent.task_generation
        || record.task.paused
        || !record.task.policy_active
        || record.launch_intent != Some(intent.id)
        || record.saved_context.is_none()
        || !matches!(record.task.state, TaskState::Waiting | TaskState::Ready)
    {
        return Err(Status::failed_precondition(
            "Launch was paused, revoked or has no committed handoff",
        ));
    }
    if !state
        .acl_store
        .read()
        .unwrap()
        .is_allowed(&intent.owner, "/metteur.Daemon/ExecuteBlueprint")
    {
        return Err(Status::permission_denied("Blueprint permission revoked"));
    }
    if intent.expires_ms <= now() || record.policy.as_ref().is_none_or(|p| p.expires_ms <= now()) {
        return Err(Status::failed_precondition("Launch authorization expired"));
    }
    if record.task.budget.unknown_usage
        || record.task.budget.consumed_tokens.saturating_add(record.task.budget.reserved_tokens)
            >= record.task.budget.token_limit
    {
        return Err(Status::failed_precondition("Parent budget exhausted or usage unknown"));
    }
    if ws.db.chat_revision(intent.session.as_bytes()).map_err(to_status)?
        != record.task.expected_revision
    {
        return Err(Status::failed_precondition("Parent session changed"));
    }
    let ctx = probe(state, ws, registry.clone(), &intent.owner);
    let fresh =
        crate::registry::tools::launch::manifest(&ctx, intent.blueprint.id).map_err(to_status)?.2;
    if fresh != intent.manifest {
        launches
            .transition(
                intent.id,
                &intent.owner,
                LaunchState::YieldCommitted,
                LaunchState::Stale,
                None,
                Some("Blueprint or dependency changed after confirmation".into()),
            )
            .map_err(to_status)?;
        return Err(Status::failed_precondition("Confirmed dependency versions are stale"));
    }
    let handoff = HandoffStore::new(ws.db.clone(), ws.root()).map_err(to_status)?;
    if handoff.get(intent.handoff, &intent.owner).map_err(to_status)?.allow_source_read {
        handoff.read_source(intent.handoff, &intent.owner, 0).map_err(to_status)?;
    }
    crate::prompt_snapshots::SelectionStore::new(ws.db.clone(), ws.root())
        .and_then(|s| s.freeze_run(run, &intent.prompts))
        .map_err(|e| Status::failed_precondition(e.to_string()))?;
    launches
        .transition(
            intent.id,
            &intent.owner,
            LaunchState::YieldCommitted,
            LaunchState::Starting,
            Some(run),
            None,
        )
        .map_err(to_status)?;
    binding
        .task
        .change(|r| {
            let source = r
                .task
                .sources
                .get_mut(&intent.source_id())
                .ok_or_else(|| fail("Launch source missing"))?;
            source.may_write = true;
            Ok(())
        })
        .map_err(to_status)?;
    launches
        .transition(
            intent.id,
            &intent.owner,
            LaunchState::Starting,
            LaunchState::Running,
            Some(run),
            None,
        )
        .map_err(to_status)?;
    Ok(())
}
/// Called only after the interpreter persisted its final checkpoint and began
/// cleanup. Unresolved child effects never masquerade as a settled success.
pub(crate) async fn finalize_launch(
    binding: &LaunchBinding,
    db: &crate::storage::persistence::Db,
    run: Uuid,
    unresolved_jobs: bool,
) -> DaemonResult<()> {
    let checkpoint = DbCheckpointSink::load(db, run)?;
    let terminal = if unresolved_jobs || checkpoint.as_ref().is_some_and(|c| c.in_flight.is_some())
    {
        Terminal::Indeterminate
    } else {
        match checkpoint.as_ref().map(|c| c.status) {
            Some(RunStatus::Completed) => Terminal::Succeeded,
            Some(RunStatus::Failed) => Terminal::Failed,
            Some(RunStatus::Cancelled) => Terminal::Cancelled,
            _ => Terminal::Indeterminate,
        }
    };
    let result = serde_json::json!({"run_id":run,"blueprint_id":binding.intent.blueprint.id,"status":terminal,"checkpoint_status":checkpoint.as_ref().map(|c|format!("{:?}",c.status)),"blueprint_version":checkpoint.as_ref().and_then(|c|c.blueprint_version.clone()),"executed_nodes":checkpoint.as_ref().map(|c|c.executed.len()),"unresolved_background_effects":unresolved_jobs,"validation_claim":"Consult the run's recorded checks; terminal success alone is not user acceptance"});
    fn safe_output(value: &metteur_shared::Value, depth: usize) -> serde_json::Value {
        if depth > 8 {
            return serde_json::json!("[depth limit]");
        }
        match value {
            metteur_shared::Value::Context(_) => {
                serde_json::json!("[context retained in run, not injected]")
            }
            metteur_shared::Value::List(xs) => serde_json::Value::Array(
                xs.iter().take(100).map(|v| safe_output(v, depth + 1)).collect(),
            ),
            other => crate::execution::nodes::value_to_json(other),
        }
    }
    let outputs = checkpoint.as_ref().map(|c| {
        let ordered: std::collections::BTreeMap<_, _> =
            c.data_values.iter().map(|(id, v)| (id.to_string(), safe_output(v, 0))).collect();
        ordered.into_iter().take(100).collect::<std::collections::BTreeMap<_, _>>()
    });
    let checks=checkpoint.as_ref().map(|c|c.view.invocations.iter().filter(|v|v.check.is_some()).map(|v|serde_json::json!({"node_id":v.node_id,"current":v.current,"check":v.check,"status":v.status,"version":v.version})).take(100).collect::<Vec<_>>());
    let artifact = crate::execution::blackboard::safe_value(
        &serde_json::json!({"run":result,"outputs":outputs,"recorded_checks":checks,"source":"persisted_run_checkpoint"}),
        0,
    );
    let redacted =
        crate::observability::anon::Anonymizer::new(&[]).anonymize(&artifact.to_string()).await;
    let mut result = binding.task.store.result_artifact(
        binding.task.id,
        &binding.intent.source_id(),
        &redacted,
    )?;
    result["run_id"] = serde_json::json!(run);
    result["status"] = serde_json::json!(terminal);
    let root = binding
        .task
        .store
        .get(binding.task.id)?
        .ok_or_else(|| fail("Parent task missing"))?
        .workspace;
    let launches = LaunchStore::new(db.clone(), std::path::Path::new(&root))?;
    let intent =
        launches.finish(binding.intent.id, &binding.intent.owner, run, terminal, result)?;
    deliver_terminal(&binding.task, &intent)
}
fn deliver_terminal(task: &TaskHandle, intent: &LaunchIntent) -> DaemonResult<()> {
    let terminal = intent.terminal.ok_or_else(|| fail("Terminal status missing"))?;
    task.finish(&intent.source_id(), terminal, intent.result.clone(), now())?;
    // Preserve Stop. A cancelled run never grants automatic restart or summary.
    for _ in 0..4 {
        let r = task.store.get(task.id)?.ok_or_else(|| fail("Parent task missing"))?;
        if r.task.state == TaskState::Stopped {
            return Ok(());
        }
        let updated=task.store.update(task.id,r.revision,|r|{if matches!(terminal,Terminal::Cancelled|Terminal::Indeterminate|Terminal::Interrupted){r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Blueprint stopped or its effects require review; automatic continuation blocked".into());}else if !intent.resume_on_completion {r.task.state=TaskState::Completed;r.task.attention=Some("Blueprint result retained; automatic completion continuation was not authorized".into());}Ok(())});
        if updated.is_ok() {
            return Ok(());
        }
    }
    Err(fail("Concurrent completion reconciliation; retry scan"))
}
/// Returns true while launch processing owns this waiting task. The ordinary
/// event dispatcher must not steal a foreground slot before its run settles.
pub(crate) async fn poll_launch(
    state: &Arc<AppState>,
    ws: &Arc<Workspace>,
    record: &TaskRecord,
) -> DaemonResult<bool> {
    let Some(id) = record.launch_intent else {
        return Ok(false);
    };
    let launches = store(ws)?;
    let (_, mut intent) = launches.get(id, &record.owner_subject)?;
    let handle = TaskHandle {
        store: crate::continuation::store::ContinuationStore::new(ws.db.clone(), ws.root())?,
        id: record.task.id,
        generation: intent.task_generation,
    };
    if record.task.state == TaskState::Stopped {
        return Ok(true);
    }
    if intent.state == LaunchState::Terminal {
        deliver_terminal(&handle, &intent)?;
        return Ok(!intent.resume_on_completion
            || matches!(
                intent.terminal,
                Some(Terminal::Cancelled | Terminal::Indeterminate | Terminal::Interrupted)
            ));
    }
    if intent.state == LaunchState::Confirmed {
        if record.saved_context.is_none()
            || record.task.wait.as_ref().is_none_or(|w| !w.sources.contains(&intent.source_id()))
        {
            return Ok(true);
        }
        intent = launches.transition(
            id,
            &intent.owner,
            LaunchState::Confirmed,
            LaunchState::YieldCommitted,
            None,
            None,
        )?;
    }
    if matches!(intent.state, LaunchState::Starting | LaunchState::Running) {
        let running =
            state.running.read().await.get(ws.root()).is_some_and(|r| Some(r.run_id) == intent.run);
        if !running {
            if let Some(run) = intent.run {
                if DbCheckpointSink::load(&ws.db, run)?.is_some_and(|c| {
                    matches!(
                        c.status,
                        RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
                    )
                }) {
                    finalize_launch(
                        &LaunchBinding {
                            task: handle.clone(),
                            intent: intent.clone(),
                        },
                        &ws.db,
                        run,
                        false,
                    )
                    .await?;
                    return Ok(true);
                }
            }
            launches.transition(
                id,
                &intent.owner,
                intent.state,
                LaunchState::NeedsAttention,
                intent.run,
                Some("Run ownership is unknown; no automatic replay".into()),
            )?;
            handle.change(|r| {
                r.task.state = TaskState::NeedsAttention;
                r.task.attention = Some("Blueprint startup or execution requires recovery".into());
                Ok(())
            })?;
        }
        return Ok(true);
    }
    if matches!(
        intent.state,
        LaunchState::Stale | LaunchState::NeedsAttention | LaunchState::Declined
    ) {
        handle.change(|r| {
            r.task.state = TaskState::NeedsAttention;
            r.task.attention =
                Some("Blueprint proposal cannot run; review its recorded state".into());
            Ok(())
        })?;
        return Ok(true);
    }
    if record.task.paused || intent.state != LaunchState::YieldCommitted {
        return Ok(true);
    }
    if state.chats.read().await.contains_key(ws.root())
        || state.running.read().await.contains_key(ws.root())
    {
        return Ok(true);
    }
    let service = DaemonService::new(state.clone());
    match service.start_blueprint_launch(handle.clone(), intent.clone()).await {
        Ok(mut stream) => {
            tokio::spawn(async move {
                use tokio_stream::StreamExt;
                while stream.next().await.is_some() {}
            });
        }
        Err(error) => {
            if !error.message().contains("active")
                && !error.message().contains("running chat")
                && !error.message().contains("background write")
            {
                let (_, latest) = launches.get(id, &intent.owner)?;
                let _ = launches.transition(
                    id,
                    &intent.owner,
                    latest.state,
                    LaunchState::NeedsAttention,
                    latest.run,
                    Some(error.to_string()),
                );
                handle.change(|r| {
                    r.task.state = TaskState::NeedsAttention;
                    r.task.attention = Some(error.to_string());
                    Ok(())
                })?;
            }
        }
    }
    Ok(true)
}
impl DaemonService {
    async fn start_blueprint_launch(
        &self,
        task: TaskHandle,
        intent: LaunchIntent,
    ) -> Result<
        tokio_stream::wrappers::ReceiverStream<Result<super::super::proto::ExecutionEvent, Status>>,
        Status,
    > {
        let record = task
            .store
            .get(task.id)
            .map_err(to_status)?
            .ok_or_else(|| Status::not_found("Parent task missing"))?;
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&record.workspace))
            .await
            .ok_or_else(|| Status::not_found("Workspace closed"))?;
        let registry = self.state.registry_for(Some(ws.root()), true).await?;
        let addon_fragments = registry.addon_fragments.values().flatten().cloned().collect();
        let run = Uuid::new_v4();
        super::spawn_execution(
            &self.state,
            ws.root().to_path_buf(),
            ws.db.clone(),
            ws.config.clone(),
            ws.root().to_path_buf(),
            registry,
            self.state.llm_factory.clone(),
            crate::observability::audit::AuditWriter::new(ws.db.clone()),
            intent.owner.clone(),
            Arc::new(DbCheckpointSink::new(ws.db.clone(), run)),
            intent.blueprint.clone(),
            None,
            crate::execution::interrupt::InterruptBus::new(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ws.lsp(),
            addon_fragments,
            Some(ws.version_manager.clone()),
            ws.jobs(),
            Some(LaunchBinding {
                task,
                intent,
            }),
        )
        .await
    }
    pub(crate) async fn query_blueprint_launch(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        let owner =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 8192 {
            return Err(Status::invalid_argument("Invalid query envelope"));
        }
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace closed"))?;
        let launches = store(&ws).map_err(to_status)?;
        let input: serde_json::Value = serde_json::from_str(&req.payload_json)
            .map_err(|_| Status::invalid_argument("Invalid launch query"))?;
        let projection = |revision: u64, i: LaunchIntent| serde_json::json!({"revision":revision,"id":i.id,"task_id":i.task,"session_id":i.session,"handoff_id":i.handoff,"blueprint_id":i.blueprint.id,"blueprint_name":i.blueprint.name,"manifest":i.manifest,"state":i.state,"digest":i.confirmation_digest,"run_id":i.run,"terminal":i.terminal,"result":i.result,"attention":i.attention,"resume_on_completion":i.resume_on_completion});
        let value = if let Some(id) = input.get("id").and_then(|v| v.as_str()) {
            let (r, i) = launches
                .get(
                    Uuid::parse_str(id)
                        .map_err(|_| Status::invalid_argument("Invalid intent ID"))?,
                    &owner,
                )
                .map_err(to_status)?;
            projection(r, i)
        } else {
            let after = input
                .get("after")
                .and_then(|v| v.as_str())
                .map(Uuid::parse_str)
                .transpose()
                .map_err(|_| Status::invalid_argument("Invalid cursor"))?;
            serde_json::json!({"items":launches.list(&owner,after).map_err(to_status)?.into_iter().map(|(r,i)|projection(r,i)).collect::<Vec<_>>()})
        };
        Ok(Response::new(ResourceResponse {
            schema_version: 1,
            payload_json: value.to_string(),
            error_json: String::new(),
        }))
    }
}

impl DaemonService {
    pub(crate) async fn confirm_blueprint_launch(
        &self,
        request: Request<ResourceRequest>,
    ) -> Result<Response<ResourceResponse>, Status> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Choice {
            request_id: String,
            intent_id: Uuid,
            original_digest: String,
            allow: bool,
            resume_on_completion: bool,
            allow_source_read: bool,
        }
        let owner =
            super::super::acl::subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        if req.schema_version != 1 || req.payload_json.len() > 8192 {
            return Err(Status::invalid_argument("Invalid confirmation envelope"));
        }
        let choice: Choice = serde_json::from_str(&req.payload_json)
            .map_err(|_| Status::invalid_argument("Invalid launch decision"))?;
        let ws = self
            .state
            .workspaces
            .get(std::path::Path::new(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("Workspace closed"))?;
        let chat = self
            .state
            .chats
            .read()
            .await
            .get(ws.root())
            .cloned()
            .ok_or_else(|| Status::not_found("Launch approval is no longer pending"))?;
        let broker =
            chat.approvals.clone().ok_or_else(|| Status::not_found("Approval broker closed"))?;
        if !broker.pending_ids().contains(&choice.request_id) {
            return Err(Status::not_found("Approval is no longer pending"));
        }
        let raw = chat
            .approval_details
            .lock()
            .get(&choice.request_id)
            .cloned()
            .ok_or_else(|| Status::not_found("Approval request not found"))?;
        let detail: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|_| Status::invalid_argument("Approval detail unavailable"))?;
        if detail["request_type"] != "blueprint_launch"
            || detail["intent_id"].as_str() != Some(&choice.intent_id.to_string())
            || detail["digest"].as_str() != Some(choice.original_digest.as_str())
        {
            return Err(Status::permission_denied(
                "Decision does not match the pending launch request",
            ));
        }
        let launches = store(&ws).map_err(to_status)?;
        let (_, intent) = launches.get(choice.intent_id, &owner).map_err(to_status)?;
        if chat.session_id != Some(intent.session) {
            return Err(Status::permission_denied("Launch is not owned by this conversation"));
        }
        if choice.allow {
            let handoffs = HandoffStore::new(ws.db.clone(), ws.root()).map_err(to_status)?;
            let mut h = handoffs.get(intent.handoff, &owner).map_err(to_status)?;
            if h.allow_source_read != choice.allow_source_read {
                h.id = Uuid::new_v5(
                    &intent.id,
                    format!("source-access:{}", choice.allow_source_read).as_bytes(),
                );
                h.allow_source_read = choice.allow_source_read;
                handoffs.save(&h).map_err(to_status)?;
            }
            launches
                .revise_choices(
                    intent.id,
                    &owner,
                    &choice.original_digest,
                    choice.resume_on_completion,
                    h.id,
                )
                .map_err(to_status)?;
        }

        broker
            .respond(
                &choice.request_id,
                if choice.allow {
                    crate::sandbox::approval::Decision::Allow
                } else {
                    crate::sandbox::approval::Decision::Deny
                },
                crate::sandbox::approval::Scope::Once,
                &crate::sandbox::grant::GrantStore::new(
                    Some(ws.db.clone()),
                    self.state.global_db.clone(),
                ),
            )
            .map_err(to_status)?;
        Ok(Response::new(ResourceResponse {
            schema_version: 1,
            payload_json: serde_json::json!({"decision_recorded":true,"intent_id":intent.id})
                .to_string(),
            error_json: String::new(),
        }))
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{
        WorkspaceManager,
        continuation::{handoff::DependencyManifest, policy::Policy, store::ContinuationStore},
    };
    use metteur_proto::proto::{OpenWorkspaceRequest, daemon_server::Daemon};
    use std::collections::BTreeMap;
    #[tokio::test]
    async fn lost_run_owner_becomes_attention_without_launching_again() {
        let root = std::env::temp_dir().join(format!("nx-launch-recovery-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let registry = Arc::new(Registry::with_builtins());
        let state = Arc::new(AppState::new(
            WorkspaceManager::new().with_global_config_path(root.join("global.toml")),
            registry.clone(),
            metteur_shared::config::Config::default(),
        ));
        let service = DaemonService::new(state.clone());
        Daemon::open_workspace(
            &service,
            Request::new(OpenWorkspaceRequest {
                path: root.to_string_lossy().into(),
            }),
        )
        .await
        .unwrap();
        let ws = state.workspaces.get(&root).await.unwrap();
        let tasks = ContinuationStore::new(ws.db.clone(), &root).unwrap();
        let policy = Policy {
            detached: true,
            max_turns: 3,
            token_limit: 1000,
            expires_ms: now() + 60000,
            purpose: "Recovery boundary".into(),
        };
        let task = tasks.create(policy.task(Uuid::new_v4(), 0)).unwrap();
        let blueprint = metteur_shared::Blueprint {
            id: Uuid::new_v4(),
            name: "Uncertain prior run".into(),
            nodes: vec![],
            edges: vec![],
            entry_node_id: Uuid::new_v4(),
        };
        let manifest = DependencyManifest::freeze(
            &blueprint,
            crate::storage::versioning::VersionRef {
                snapshot_id: Uuid::new_v4(),
                blueprint_uri: "blueprints/recovery.blueprint".into(),
                blob_hash: "a".repeat(64),
            },
            &registry,
            &BTreeMap::new(),
        )
        .unwrap();
        let launches = store(&ws).unwrap();
        let (revision, intent) = launches
            .propose(LaunchIntent {
                schema_version: 1,
                id: Uuid::new_v4(),
                owner: "local".into(),
                task: task.task.id,
                task_generation: 1,
                session: task.task.session,
                turn: Uuid::new_v4(),
                handoff: Uuid::new_v4(),
                blueprint,
                manifest,
                prompts: BTreeMap::new(),
                resume_on_completion: true,
                expires_ms: policy.expires_ms,
                state: LaunchState::Proposed,
                confirmation_digest: String::new(),
                confirmed_ms: None,
                confirmation_request: None,
                run: None,
                terminal: None,
                result: serde_json::Value::Null,
                attention: None,
            })
            .unwrap();
        launches
            .decide(
                intent.id,
                "local",
                revision,
                true,
                &intent.confirmation_digest,
                &intent.manifest,
                now(),
            )
            .unwrap();
        launches
            .transition(
                intent.id,
                "local",
                LaunchState::Confirmed,
                LaunchState::YieldCommitted,
                None,
                None,
            )
            .unwrap();
        let run = Uuid::new_v4();
        launches
            .transition(
                intent.id,
                "local",
                LaunchState::YieldCommitted,
                LaunchState::Starting,
                Some(run),
                None,
            )
            .unwrap();
        launches
            .transition(
                intent.id,
                "local",
                LaunchState::Starting,
                LaunchState::Running,
                Some(run),
                None,
            )
            .unwrap();
        let handle = TaskHandle {
            store: tasks.clone(),
            id: task.task.id,
            generation: 1,
        };
        handle.register(&intent.source_id(), "blueprint", true).unwrap();
        handle
            .change(|r| {
                r.owner_subject = "local".into();
                r.policy = Some(policy.clone());
                r.launch_intent = Some(intent.id);
                r.task
                    .commit_wait(
                        crate::chat::wait::WaitIntent {
                            sources: vec![intent.source_id()],
                            mode: crate::chat::wait::MatchMode::All,
                            deadline_ms: policy.expires_ms,
                            params: serde_json::json!({}),
                        },
                        0,
                        now(),
                    )
                    .map_err(fail)
            })
            .unwrap();
        tasks.recover().unwrap();
        let record = tasks.get(task.task.id).unwrap().unwrap();
        assert!(poll_launch(&state, &ws, &record).await.unwrap());
        let after = tasks.get(task.task.id).unwrap().unwrap();
        assert_eq!(after.task.state, TaskState::NeedsAttention);
        let recovered = store(&ws).unwrap().get(intent.id, "local").unwrap().1;
        assert_eq!(recovered.state, LaunchState::NeedsAttention);
        assert_eq!(recovered.run, Some(run));
        assert!(state.running.read().await.is_empty());
        assert!(after.task.sources[&intent.source_id()].terminal.is_none());
        assert!(poll_launch(&state, &ws, &after).await.unwrap());
        assert!(state.running.read().await.is_empty());
    }
}
