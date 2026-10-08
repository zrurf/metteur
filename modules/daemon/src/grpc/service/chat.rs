//! Chat RPCs: streaming conversation turns, abort and session persistence.

use std::path::PathBuf;
use std::sync::Arc;

use metteur_shared::llm::{ContextManager, Message, Role};
use tonic::{Request, Response, Status};

use crate::chat::session::{
    ChatSessionRecord, delete_thread, display_message_count, list_threads, load_thread,
    save_thread, title_of,
};
use crate::chat::transcript::Transcript;
use crate::execution::context::ExecutionContext;
use crate::execution::interrupt::{Interrupt, InterruptBus, InterruptPriority};
use crate::execution::react::{ReactEvent, run_react_streaming};
use crate::llm::StreamDelta;
use crate::observability::audit::AuditWriter;
use crate::sandbox::approval::ApprovalBroker;

use super::super::acl::subject_from_request;
use super::super::proto::{
    AbortChatRequest, ChatEvent, ChatSessionInfo, ChatSessionList, DeleteChatSessionRequest, Empty,
    GetChatSessionRequest, GetChatSessionResponse, ListChatSessionsRequest, RewindChatRequest,
    SendChatRequest,
};
use super::*;

impl DaemonService {
    pub(crate) async fn send_chat(
        &self,
        request: Request<SendChatRequest>,
    ) -> Result<
        Response<tokio_stream::wrappers::UnboundedReceiverStream<Result<ChatEvent, Status>>>,
        Status,
    > {
        let turn_started = std::time::Instant::now();
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".to_string());
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let run_id = uuid::Uuid::new_v4();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let ws_db = ws.db.clone();
        let _admission = ws.activity_gate.lock().await;

        // A retry is deliberately session-bound. If the client loses the
        // restored session id, fail closed instead of treating the request as
        // a new conversation seeded from display history, which could contain
        // the answer that was just discarded.
        if chat_option_bool(&req.options_json, "retry") && req.session_id.trim().is_empty() {
            return Err(Status::failed_precondition(
                "retry requires the restored persisted chat session",
            ));
        }

        // Resolve the session before claiming the chat slot: an explicitly
        // named session must exist and belong to this workspace; an empty id
        // starts a fresh thread (history_json seeds its context).
        let existing = match req.session_id.as_str() {
            "" => None,
            id => {
                let session_id = uuid::Uuid::parse_str(id)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?;
                match load_thread(&ws_db, &session_id)
                    .map_err(|e| Status::internal(e.to_string()))?
                {
                    Some(record) => Some(record),
                    None => return Err(Status::not_found(format!("chat session {id} not found"))),
                }
            }
        };
        let session_id = existing.as_ref().map(|r| r.session_id).unwrap_or_else(uuid::Uuid::new_v4);
        let created_at = existing.as_ref().map(|r| r.created_at).unwrap_or(now);
        let old_turns = existing.as_ref().map(|r| r.turns).unwrap_or(0);
        let title = existing.as_ref().and_then(|r| r.title.clone());
        let existing_transcript = existing.as_ref().map(|r| r.transcript.clone());
        let existing_todos = existing.as_ref().map(|r| r.todos.clone()).unwrap_or_default();

        // A workspace hosts either one execution or one chat at a time.
        {
            let running = self.state.running.read().await;
            if running.contains_key(&ws_key) {
                return Err(Status::failed_precondition("workspace has a running execution"));
            }
        }
        if self.state.chats.read().await.contains_key(&ws_key) {
            return Err(Status::failed_precondition("workspace already has an active chat"));
        }
        let registry=self.state.registry_for(Some(ws.root()),true).await?;
        ws.reconcile_files().map_err(to_status)?;
        // Capture the exact model state, not a reconstruction from UI messages.
        let before = existing.clone().unwrap_or_else(|| {
            let mut context = ContextManager::default();
            history_messages(&req.history_json, &mut context.messages);
            let mut record = ChatSessionRecord::new(now, context);
            record.session_id = session_id;
            record
        });
        let checkpoint_id = crate::chat::checkpoint::capture(
            &ws_db,
            &ws.version_manager,
            before,
            &format!("Before chat turn {}", old_turns + 1),
        ).map_err(|error| Status::failed_precondition(format!(
            "chat recovery checkpoint unavailable; no new turn started: {error}"
        )))?.to_string();
        let interrupt_bus = InterruptBus::new();
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let broker = Arc::new(ApprovalBroker::new());
        let persist = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.state.chats.write().await.insert(
            ws_key.clone(),
            ChatRun {
                interrupt_bus: Some(interrupt_bus.clone()),
                cancel_requested: cancel_flag.clone(),
                approvals: Some(broker.clone()),
                persist: persist.clone(),
                session_id: Some(session_id),
            },
        );

        let addon_fragments: Vec<_> = registry.addon_fragments.values().flatten().cloned().collect();
        let (event_tx, event_rx) =
            tokio::sync::mpsc::unbounded_channel::<Result<ChatEvent, Status>>();
        let state = self.state.clone();
        let llm_factory = self.state.llm_factory.clone();
        let root = ws.root().to_path_buf();
        let ws_config = ws.config.clone();
        let lsp_source = ws.lsp_manager.clone();
        let version_manager = ws.version_manager.clone();
        let jobs = ws.jobs();
        let user = subject.clone();
        let message = req.message.clone();
        let history_json = req.history_json.clone();
        let options_json = req.options_json.clone();

        tokio::spawn(async move {
            let approval_lifetime = broker.close_on_drop();
            let mut ctx = ExecutionContext::new(registry, llm_factory, root.clone())
                .with_run(run_id, now)
                .with_user(user)
                .with_config(ws_config)
                .with_audit(AuditWriter::new(ws_db.clone()));
            ctx.interrupts = Some(interrupt_bus);
            ctx.cancel_requested = cancel_flag.clone();
            ctx.approvals = Some(broker);
            ctx.workspace_db = Some(ws_db.clone());
            ctx.lsp_source = Some(lsp_source);
            ctx.version_manager = Some(version_manager);
            ctx.jobs = Arc::clone(&jobs);
            ctx.todos = existing_todos;
            if let Some(global_db) = state.global_db.clone() {
                ctx.global_db = Some(global_db);
            }
            ctx.addon_fragments = addon_fragments.clone();
            // The composer's permission selector travels with each turn; an
            // absent value falls back to the workspace configuration.
            if let Some(mode) = chat_option(&options_json, "permission_mode") {
                ctx.permission_mode = crate::sandbox::PermissionMode::parse(&mode);
            } else if let Some(config) = &ctx.config {
                let configured =
                    metteur_shared::config::effective_permission_mode(&config.read().await.sandbox);
                ctx.permission_mode = crate::sandbox::PermissionMode::parse(configured);
            }

            // Compose the context: persisted sessions are authoritative (they
            // retain tool results); new sessions start from history_json.
            let harness = crate::harness::fragments(&ctx).await;
            let mut context = match existing {
                Some(record) => {
                    let mut restored = record.context;
                    // Rebuild the harness sections from the current environment;
                    // identical text leaves the stored prefix untouched.
                    crate::harness::HarnessPrompt::refresh(&mut restored, harness);
                    // Addons installed since the session started still apply
                    // to the next turn; already-injected copies are kept as-is.
                    for fragment in &addon_fragments {
                        let present = restored
                            .system_fragments
                            .iter()
                            .any(|f| f.scope == fragment.scope && f.content == fragment.content);
                        if !present {
                            restored.system_fragments.push(fragment.clone());
                        }
                    }
                    restored
                }
                None => {
                    let mut fresh = ContextManager {
                        system_fragments: harness,
                        ..ContextManager::default()
                    };
                    fresh.system_fragments.extend(addon_fragments);
                    history_messages(&history_json, &mut fresh.messages);
                    fresh
                }
            };
            // Sessions written before the send path was fixed carry their
            // opening turn twice; the repair is a no-op for healthy ones.
            let repaired = crate::chat::session::repair_duplicate_opening(&mut context);
            if repaired > 0 {
                tracing::info!("chat {session_id}: repaired {repaired} duplicated opening turn(s)");
            }
            context.push_message(Message::text(Role::User, message.clone()));

            // The transcript is the display view: it keeps tool calls, notices
            // and the timing the model's context cannot reproduce.
            let mut transcript = Transcript::resuming(existing_transcript.unwrap_or_default());
            transcript.push_user(message.clone(), now);
            transcript.set_user_checkpoint(&checkpoint_id);

            // New sessions take their title from the first user message.
            let title = title.or_else(|| title_of(&context));

            // Announce the session id so the client can resume later turns,
            // together with the context it starts from: without it the client
            // has no size to show until the first turn completes.
            let opening_stats = context_stats(&ctx, &context).await;
            let _ = event_tx.send(Ok(ChatEvent {
                kind: "session".to_string(),
                content: String::new(),
                detail_json: serde_json::json!({
                    "session_id": session_id.to_string(),
                    "created_at": created_at,
                    "checkpoint_id": checkpoint_id,
                    "context": opening_stats,
                })
                .to_string(),
            }));

            // Tool-side engine events are bridged onto the chat stream so a
            // `TodoWrite` (or a retry/fallback notice from the loop) reaches
            // the UI during the turn instead of only when it ends.
            let (engine_tx, mut engine_rx) =
                tokio::sync::mpsc::unbounded_channel::<crate::execution::ExecutionEvent>();
            ctx.events = Some(engine_tx);
            let bridge_tx = event_tx.clone();
            tokio::spawn(async move {
                while let Some(event) = engine_rx.recv().await {
                    let payload = match event {
                        crate::execution::ExecutionEvent::Todos {
                            node_id,
                            todos,
                        } => ChatEvent {
                            kind: "todos".to_string(),
                            content: String::new(),
                            detail_json: serde_json::json!({
                                "node_id": node_id.to_string(),
                                "todos": todos,
                            })
                            .to_string(),
                        },
                        crate::execution::ExecutionEvent::Message {
                            message,
                            ..
                        } => ChatEvent {
                            kind: "notice".to_string(),
                            content: message,
                            detail_json: String::new(),
                        },
                        crate::execution::ExecutionEvent::Job {
                            job_id,
                            state,
                            summary,
                            ..
                        } => ChatEvent {
                            kind: "job".to_string(),
                            content: summary,
                            detail_json: serde_json::json!({
                                "job_id": job_id,
                                "state": state,
                            })
                            .to_string(),
                        },
                        // An approval request must reach the client that can
                        // answer it; dropping it here made a turn that needs a
                        // decision wait for the timeout with nothing shown.
                        crate::execution::ExecutionEvent::ApprovalRequested {
                            request_id,
                            detail,
                            ..
                        } => ChatEvent {
                            kind: "approval".to_string(),
                            content: detail,
                            detail_json: serde_json::json!({ "request_id": request_id })
                                .to_string(),
                        },
                        _ => continue,
                    };
                    if bridge_tx.send(Ok(payload)).is_err() {
                        break;
                    }
                }
            });

            let mut opts = chat_options(&options_json);
            if let Some(config) = &ctx.config {
                let llm = config.read().await.llm.clone();
                opts.tool_error_limit = llm.tool_error_limit;
                opts.repeat_call_limit = llm.repeat_call_limit;
                opts.max_tool_results = llm.max_tool_results as usize;
                opts.parallel_read_tools = llm.parallel_read_tools;
                opts.stale_result_placeholders = llm.stale_result_placeholders;
                opts.dedup_reads = llm.dedup_reads;
                opts.max_tool_result_bytes = llm.max_tool_result_bytes as usize;
                opts.anonymize_thinking = llm.anonymize_thinking;
                if opts.compress_after_messages.is_none() && llm.compress_at_ratio > 0.0 {
                    // Long chats must not grow until the provider rejects the
                    // request; the ReAct loop also enforces the token budget.
                    opts.compress_after_messages = Some(40);
                }
            }
            // The closures below run while the turn streams, so the transcript
            // is shared rather than borrowed.
            let transcript = std::sync::Arc::new(parking_lot::Mutex::new(transcript));
            let delta_tx = event_tx.clone();
            let delta_cancel = cancel_flag.clone();
            let delta_journal = std::sync::Arc::clone(&transcript);
            // Argument-size reports arrive per chunk; one per 400ms is what a
            // reader can use, and a burst would drown the transcript.
            let mut last_args_at = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap_or_else(std::time::Instant::now);
            let mut on_delta = move |delta: StreamDelta| {
                {
                    let mut journal = delta_journal.lock();
                    if matches!(&delta, StreamDelta::Reasoning(text) if !text.is_empty()) {
                        journal.start_reasoning();
                    }
                }
                let event = match delta {
                    StreamDelta::Text(text) => ChatEvent {
                        kind: "assistant_delta".to_string(),
                        content: text,
                        detail_json: String::new(),
                    },
                    StreamDelta::Reasoning(text) => ChatEvent {
                        kind: "reasoning_delta".to_string(),
                        content: text,
                        detail_json: String::new(),
                    },
                    StreamDelta::ToolArgs {
                        name,
                        bytes,
                    } => {
                        let now = std::time::Instant::now();
                        if now.duration_since(last_args_at) < std::time::Duration::from_millis(400)
                        {
                            return;
                        }
                        last_args_at = now;
                        ChatEvent {
                            kind: "tool_args".to_string(),
                            content: String::new(),
                            detail_json: serde_json::json!({ "name": name, "bytes": bytes })
                                .to_string(),
                        }
                    }
                };
                if delta_tx.send(Ok(event)).is_err() {
                    delta_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            };
            let stream_tx = event_tx.clone();
            let journal = std::sync::Arc::clone(&transcript);
            let mut on_event = move |ev: ReactEvent| {
                let reasoning_elapsed_ms = if matches!(&ev, ReactEvent::Assistant { .. }) {
                    journal.lock().finish_reasoning()
                } else {
                    None
                };
                record_react_event(&journal, &ev);
                let event = match ev {
                    ReactEvent::Assistant {
                        text,
                        reasoning,
                    } => ChatEvent {
                        kind: "assistant".to_string(),
                        content: text,
                        detail_json: serde_json::json!({
                            "reasoning": reasoning,
                            "reasoning_elapsed_ms": reasoning_elapsed_ms,
                        })
                        .to_string(),
                    },
                    // The live client already streamed this text with the
                    // deltas, so it is recorded but not re-sent.
                    ReactEvent::AssistantText {
                        ..
                    } => return,
                    ReactEvent::ToolStart {
                        call_id,
                        name,
                        summary,
                    } => ChatEvent {
                        kind: "tool_start".to_string(),
                        content: summary,
                        detail_json: serde_json::json!({ "name": name, "call_id": call_id })
                            .to_string(),
                    },
                    ReactEvent::ToolProgress {
                        call_id,
                        tail,
                    } => ChatEvent {
                        kind: "tool_progress".to_string(),
                        content: tail,
                        detail_json: serde_json::json!({ "call_id": call_id }).to_string(),
                    },
                    ReactEvent::Tool {
                        call_id,
                        name,
                        ok,
                        elapsed_ms,
                        content,
                    } => ChatEvent {
                        kind: "tool".to_string(),
                        content,
                        detail_json: serde_json::json!({
                            "name": name,
                            "call_id": call_id,
                            "ok": ok,
                            "elapsed_ms": elapsed_ms,
                        })
                        .to_string(),
                    },
                };
                if stream_tx.send(Ok(event)).is_err() {
                    cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            };

            let stream_broker = ctx.approvals.as_ref().unwrap().clone();
            let stream_cancel = ctx.cancel_requested.clone();
            let outcome = {
                let run = run_react_streaming(
                    &mut ctx, context, &opts, Some(&mut on_delta), &mut on_event,
                );
                tokio::pin!(run);
                tokio::select! {
                    biased;
                    _ = event_tx.closed() => {
                        stream_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                        stream_broker.close();
                        run.await
                    }
                    outcome = &mut run => outcome,
                }
            };
            drop(approval_lifetime);
            // The task list lives on the execution context; carry it into the
            // persisted record so a resumed session remembers the plan.
            let todos = ctx.todos.clone();
            let turn_elapsed_ms = turn_started.elapsed().as_millis() as u64;
            transcript.lock().finish_turn(turn_elapsed_ms);
            let (mut terminal, saved) = match outcome {
                Ok(outcome) => {
                    // Text-only answers are not appended by the loop; persist
                    // them so a resumed session remembers prior turns.
                    let mut saved_context = outcome.context.clone();
                    if !outcome.text.is_empty() {
                        saved_context
                            .push_message(Message::text(Role::Assistant, outcome.text.clone()));
                    }
                    // Measured before the context moves into the record.
                    let stats = context_stats(&ctx, &saved_context).await;
                    let record = ChatSessionRecord {
                        session_id,
                        created_at,
                        updated_at: now,
                        turns: old_turns + 1,
                        title,
                        context: saved_context,
                        todos: todos.clone(),
                        transcript: finish_transcript(&transcript),
                    };
                    let terminal = Ok(ChatEvent {
                        kind: "done".to_string(),
                        content: String::new(),
                        detail_json: serde_json::json!({
                            "usage": {
                                "input_tokens": outcome.usage.input_tokens,
                                "output_tokens": outcome.usage.output_tokens,
                                "total_tokens": outcome.usage.total_tokens,
                                "cached_input_tokens": outcome.usage.cached_input_tokens,
                                "cache_write_input_tokens":
                                    outcome.usage.cache_write_input_tokens,
                            },
                            // What the conversation now occupies, so the client can
                            // show a capacity meter without a second RPC.
                            "context": stats,
                            "session_id": session_id.to_string(),
                            "turn_elapsed_ms": turn_elapsed_ms,
                        })
                        .to_string(),
                    });
                    (terminal, Some(record))
                }
                Err((err, partial)) => {
                    // Persist the partially mutated context so an aborted or
                    // failed turn can be resumed from where it stopped. The
                    // transcript keeps the failure so a restored conversation
                    // says why the turn stopped.
                    if let Some(mut journal) = transcript.try_lock() {
                        journal.push_error(err.to_string(), now);
                    }
                    let record = ChatSessionRecord {
                        session_id,
                        created_at,
                        updated_at: now,
                        turns: old_turns,
                        title,
                        context: partial,
                        todos: todos.clone(),
                        transcript: finish_transcript(&transcript),
                    };
                    let terminal = Ok(ChatEvent {
                        kind: "error".to_string(),
                        content: err.to_string(),
                        detail_json: serde_json::json!({ "turn_elapsed_ms": turn_elapsed_ms })
                            .to_string(),
                    });
                    (terminal, Some(record))
                }
            };
            if persist.load(std::sync::atomic::Ordering::SeqCst)
                && let Some(record) = saved
                && let Err(err) = save_thread(&ws_db, &record)
            {
                tracing::warn!("[chat] failed to persist session: {err}");
                terminal = Ok(ChatEvent {
                    kind: "error".to_string(),
                    content: format!("chat persistence failed; turn state is not durably recoverable: {err}"),
                    detail_json: serde_json::json!({ "turn_elapsed_ms": turn_elapsed_ms }).to_string(),
                });
            }
            let _ = event_tx.send(terminal);
            // Background commands belong to the turn that started them.
            let killed = jobs.kill_owned_by(run_id);
            if killed > 0 {
                tracing::info!("chat {run_id} ended; terminated {killed} background job(s)");
            }
            state.chats.write().await.remove(&ws_key);
        });

        Ok(Response::new(tokio_stream::wrappers::UnboundedReceiverStream::new(event_rx)))
    }

    pub(crate) async fn abort_chat(
        &self,
        request: Request<AbortChatRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        let chats = self.state.chats.read().await;
        let entry = chats.get(&ws_key).ok_or_else(|| Status::not_found("no active chat"))?;
        entry.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(broker) = &entry.approvals {
            broker.close();
        }
        if let Some(bus) = &entry.interrupt_bus {
            bus.send(Interrupt {
                priority: InterruptPriority::Emergency,
                message: "Aborted by user.".to_string(),
            });
        }
        Ok(Response::new(Empty {}))
    }

    pub(crate) async fn list_chat_sessions(
        &self,
        request: Request<ListChatSessionsRequest>,
    ) -> Result<Response<ChatSessionList>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let sessions = list_threads(&ws.db)
            .map_err(|e| Status::internal(e.to_string()))?
            .into_iter()
            .rev()
            .map(|record| ChatSessionInfo {
                session_id: record.session_id.to_string(),
                created_at: record.created_at as i64,
                updated_at: record.updated_at as i64,
                turns: record.turns,
                title: record.title.clone().unwrap_or_default(),
                message_count: display_message_count(&record) as u64,
            })
            .collect();
        Ok(Response::new(ChatSessionList {
            sessions,
        }))
    }

    pub(crate) async fn get_chat_session(
        &self,
        request: Request<GetChatSessionRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        // An empty id resolves to the most recently updated thread.
        let record = if req.session_id.is_empty() {
            let mut threads = list_threads(&ws.db).map_err(|e| Status::internal(e.to_string()))?;
            threads.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
            threads.into_iter().next()
        } else {
            let session_id = uuid::Uuid::parse_str(&req.session_id)
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            load_thread(&ws.db, &session_id).map_err(|e| Status::internal(e.to_string()))?
        };
        let record = record.ok_or_else(|| Status::not_found("chat session not found"))?;
        Ok(Response::new(GetChatSessionResponse {
            session_id: record.session_id.to_string(),
            created_at: record.created_at as i64,
            history_json: context_history_json(&record.context),
            todos_json: serde_json::to_string(&record.todos).unwrap_or_else(|_| "[]".to_string()),
            transcript_json: crate::chat::transcript::to_json(&record.transcript),
        }))
    }

    /// Rewinds a whole user turn, including everything produced after it.
    pub(crate) async fn rewind_chat(
        &self,
        request: Request<RewindChatRequest>,
    ) -> Result<Response<GetChatSessionResponse>, Status> {
        let subject = subject_from_request(&request).unwrap_or_else(|| "local".into());
        let req = request.into_inner();
        let session_id = uuid::Uuid::parse_str(&req.session_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let snapshot_id = uuid::Uuid::parse_str(&req.snapshot_id)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let _gate = ws.activity_gate.lock().await;
        crate::chat::checkpoint::check_owner(&ws.db, session_id, snapshot_id).map_err(to_status)?;
        if self.state.running.read().await.contains_key(ws.root()) {
            return Err(Status::failed_precondition(
                "stop the blueprint execution before rewinding chat",
            ));
        }
        {
            let chats = self.state.chats.read().await;
            if let Some(run) = chats.get(ws.root()) {
                if run.session_id != Some(session_id) {
                    return Err(Status::failed_precondition("another conversation is running"));
                }
                run.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(broker) = &run.approvals {
                    broker.close();
                }
            }
        }
        // Keep admission blocked until the old task has persisted and killed its
        // jobs; otherwise a late finalizer could overwrite the restored context.
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while self.state.chats.read().await.contains_key(ws.root()) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| Status::failed_precondition("chat is still stopping; retry rewind shortly"))?;
        if ws.jobs.list(None).iter().any(|job| job.state.is_running()) {
            return Err(Status::failed_precondition("stop background jobs before rewinding files"));
        }
        let record =
            crate::chat::checkpoint::restore(&ws.db, &ws.version_manager, session_id, snapshot_id)
                .map_err(to_status)?;
        let _ = AuditWriter::new(ws.db.clone()).record(
            &subject,
            "chat.rewind",
            serde_json::json!({
                "session_id": session_id, "snapshot_id": snapshot_id,
            }),
        );
        Ok(Response::new(GetChatSessionResponse {
            session_id: record.session_id.to_string(),
            created_at: record.created_at as i64,
            history_json: context_history_json(&record.context),
            todos_json: serde_json::to_string(&record.todos).unwrap_or_else(|_| "[]".into()),
            transcript_json: crate::chat::transcript::to_json(&record.transcript),
        }))
    }

    pub(crate) async fn delete_chat_session(
        &self,
        request: Request<DeleteChatSessionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let ws_key = ws.root().to_path_buf();
        // An empty id targets the most recently updated thread.
        let _gate = ws.activity_gate.lock().await;
        let target = if req.session_id.is_empty() {
            let mut threads = list_threads(&ws.db).map_err(|e| Status::internal(e.to_string()))?;
            threads.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
            threads.first().map(|r| r.session_id)
        } else {
            Some(
                uuid::Uuid::parse_str(&req.session_id)
                    .map_err(|e| Status::invalid_argument(e.to_string()))?,
            )
        };
        // Stop a running chat only when it runs on the deleted thread, and
        // forbid its finalize from persisting so a late write cannot resurrect
        // the cleared session. Deleting an idle thread must not abort another
        // thread's in-flight turn.
        if let Some(target) = target {
            let abort = self
                .state
                .chats
                .read()
                .await
                .get(&ws_key)
                .is_some_and(|chat| chat.session_id == Some(target));
            if abort && let Some(chat) = self.state.chats.write().await.get_mut(&ws_key) {
                chat.persist.store(false, std::sync::atomic::Ordering::SeqCst);
                chat.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(broker) = &chat.approvals {
                    broker.close();
                }
                if let Some(bus) = &chat.interrupt_bus {
                    bus.send(Interrupt {
                        priority: InterruptPriority::Emergency,
                        message: "Aborted by user.".to_string(),
                    });
                }
            }
            delete_thread(&ws.db, &target).map_err(|e| Status::internal(e.to_string()))?;
        }
        Ok(Response::new(Empty {}))
    }
}

/// Reads a non-empty string field out of the chat options JSON.
fn chat_option(options_json: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(options_json).ok()?;
    let text = value.get(key)?.as_str()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Reads a boolean option without making malformed options fatal.
fn chat_option_bool(options_json: &str, key: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(options_json)
        .ok()
        .and_then(|value| value.get(key).and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Input window assumed when the model's configuration does not state one.
const DEFAULT_CONTEXT_WINDOW: u64 = 128 * 1024;

/// Summarises what the conversation occupies: tokens, the model's window and
/// the per-region breakdown the capacity popover renders.
///
/// The numbers are estimates (see `llm::token::estimate_tokens`); a client must
/// present them as an indication, not as a billed figure. The window is
/// `null` for models whose configuration does not describe one, and the UI
/// hides the meter rather than inventing a limit.
async fn context_stats(ctx: &ExecutionContext, context: &ContextManager) -> serde_json::Value {
    let llm = match &ctx.config {
        Some(config) => config.read().await.llm.clone(),
        None => metteur_shared::config::LlmConfig::default(),
    };
    let model = llm
        .default_model
        .as_deref()
        .and_then(|key| llm.models.get(key))
        .or_else(|| llm.models.values().next());
    // A configured window wins; otherwise a built-in default for a known model
    // family, and failing both the common modern default so the meter still has
    // a percentage. Only the last case is an *assumption*: the other two are
    // either an operator assertion or a documented default.
    let configured = crate::llm::has_configured_window(model);
    let limit = crate::llm::context_window_budget(model, None).or(Some(DEFAULT_CONTEXT_WINDOW));
    let regions: Vec<serde_json::Value> = context
        .usage_report()
        .into_iter()
        .map(|region| serde_json::json!({ "region": region.region, "tokens": region.tokens }))
        .collect();
    serde_json::json!({
        "tokens": metteur_shared::llm::estimate_context_tokens(context),
        "limit": limit,
        "assumed_limit": !configured,
        "regions": regions,
    })
}

/// Mirrors one ReAct event into the session transcript.
///
/// Only the durable steps are recorded: progress tails belong to a running
/// call and a restored turn shows the result, not every intermediate chunk.
fn record_react_event(
    journal: &std::sync::Arc<parking_lot::Mutex<Transcript>>,
    event: &ReactEvent,
) {
    let at = chrono::Utc::now().timestamp_millis() as u64;
    let mut journal = journal.lock();
    match event {
        ReactEvent::Assistant {
            text,
            reasoning,
        } => journal.push_assistant(text.clone(), reasoning.clone(), at),
        ReactEvent::AssistantText {
            text,
        } => journal.push_assistant(text.clone(), String::new(), at),
        ReactEvent::ToolStart {
            call_id,
            name,
            summary,
        } => journal.push_tool_start(call_id.clone(), name.clone(), summary.clone(), at),
        ReactEvent::Tool {
            call_id,
            name,
            ok,
            elapsed_ms,
            content,
        } => journal.push_tool_result(call_id, name, *ok, *elapsed_ms, content.clone(), at),
        ReactEvent::ToolProgress {
            ..
        } => {}
    }
}

/// Closes the shared transcript, returning the session's entries.
fn finish_transcript(
    journal: &std::sync::Arc<parking_lot::Mutex<Transcript>>,
) -> Vec<crate::chat::transcript::TranscriptEntry> {
    let entries = std::mem::take(&mut *journal.lock());
    entries.finish()
}

/// Serializes a context's user/assistant text as `[{role, content}]`.
fn context_history_json(context: &ContextManager) -> String {
    let entries: Vec<serde_json::Value> = context
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                _ => "assistant",
            };
            serde_json::json!({ "role": role, "content": m.text_content() })
        })
        .collect();
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ContextManager {
        let mut ctx = ContextManager::default();
        ctx.push_message(Message::text(Role::User, "hello"));
        ctx.push_message(Message::text(Role::Assistant, "reply"));
        ctx.push_message(Message {
            role: Role::Tool,
            content: vec![metteur_shared::llm::ContentBlock::Text("tool out".to_string())],
            tool_calls: Vec::new(),
            tool_call_id: Some("call_1".to_string()),
        });
        ctx
    }

    #[test]
    fn history_json_excludes_tool_and_system_messages() {
        let json = context_history_json(&context());
        let entries: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["role"], "user");
        assert_eq!(entries[1]["content"], "reply");
    }

    #[test]
    fn retry_option_is_strict_and_malformed_values_are_ignored() {
        assert!(chat_option_bool(r#"{"retry":true}"#, "retry"));
        assert!(!chat_option_bool(r#"{"retry":"true"}"#, "retry"));
        assert!(!chat_option_bool("not-json", "retry"));
    }
}
