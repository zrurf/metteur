//! Shared ReAct loop kernel.
//!
//! [`run_react`] owns the iteration loop previously embedded in the Call LLM
//! node: cancellation/pause handling, interrupt injection, emergency racing,
//! tool invocation with transaction/audit records, anonymization of tool
//! traffic, billing-enriched usage audits and optional context compression.
//! It is reused by the CallLLM node, the SpawnSubAgent tool and the Abstract
//! node planner.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use metteur_shared::Usage;
use metteur_shared::config::{LlmConfig, LlmModelConfig};
use metteur_shared::llm::{
    ContentBlock, ContextManager, GenerationParams, Message, ReasoningEffort, Role, ToolCall,
    ToolDefinition, ToolResult,
};

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::execution::interrupt::InterruptBus;
use crate::execution::interrupt::InterruptPriority;
use crate::execution::jobs::{JobManager, JobSnapshot};
use crate::llm::{
    LlmClient, LlmProviderConfig, LlmResponse, MockClient, MockStep, ProviderKind, StreamDelta,
};
use crate::observability::anon::Anonymizer;
use crate::registry::tools::result::truncate_result;

/// The default maximum number of ReAct iterations.
pub const DEFAULT_MAX_ITERATIONS: usize = 10;

/// Consecutive tool failures tolerated before a run is aborted.
pub const DEFAULT_TOOL_ERROR_LIMIT: u32 = 5;

/// Identical tool invocations tolerated before a run is aborted.
pub const DEFAULT_REPEAT_CALL_LIMIT: u32 = 3;

/// Tool results retained in the context before eviction kicks in.
pub const DEFAULT_MAX_TOOL_RESULTS: usize = 24;

/// Options controlling a single ReAct run.
#[derive(Debug, Clone)]
pub struct ReactOptions {
    /// Provider key (`openai-chat`, `anthropic`, `openai-responses` or `mock`).
    pub provider: String,
    /// Explicit model override; falls back to configured defaults.
    pub model: Option<String>,
    /// Explicit base URL override.
    pub base_url: Option<String>,
    /// Explicit API key override.
    pub api_key: Option<String>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Nucleus sampling probability mass.
    pub top_p: Option<f64>,
    /// Maximum number of tokens to generate.
    pub max_tokens: Option<u32>,
    /// Sequences at which generation stops.
    pub stop: Vec<String>,
    /// Reasoning effort for reasoning-capable models.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Seed for reproducible sampling.
    pub seed: Option<i64>,
    /// Presence penalty.
    pub presence_penalty: Option<f64>,
    /// Frequency penalty.
    pub frequency_penalty: Option<f64>,
    /// Maximum number of LLM iterations before giving up.
    pub max_iterations: usize,
    /// Tools callable in this run; `None` allows every registered tool.
    pub allowed_tools: Option<HashSet<String>>,
    /// Compress the context when it exceeds this many messages.
    pub compress_after_messages: Option<usize>,
    /// Human-readable label used in logs and audit details.
    pub label: String,
    /// Scripted text for the mock provider.
    pub mock_text: Option<String>,
    /// Scripted steps for the mock provider; overrides `mock_text` when set.
    ///
    /// A tool turn cannot be scripted with a single text answer, and the chat
    /// RPC has no other way to reach one without a provider.
    pub mock_steps: Option<Vec<MockStep>>,
    /// Artificial delay before each mock response.
    pub mock_delay_ms: Option<u64>,
    /// Consecutive tool failures tolerated before giving up (`0` disables).
    pub tool_error_limit: u32,
    /// Identical `(name, arguments)` invocations tolerated (`0` disables).
    pub repeat_call_limit: u32,
    /// Tool results retained in the context (`0` disables eviction).
    pub max_tool_results: usize,
    /// Run read-only tool calls of one turn concurrently.
    pub parallel_read_tools: bool,
    /// Replace results of files modified afterwards with a stale marker.
    pub stale_result_placeholders: bool,
    /// Replace an earlier full read of the same paths when a file is read
    /// again.
    pub dedup_reads: bool,
    /// Fallback byte cap for tool results that declare none of their own.
    pub max_tool_result_bytes: usize,
    /// Anonymize thinking text; drops its signature (see `outbound_context`).
    pub anonymize_thinking: bool,
}

impl Default for ReactOptions {
    fn default() -> Self {
        Self {
            provider: "openai-chat".to_string(),
            model: None,
            base_url: None,
            api_key: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop: Vec::new(),
            reasoning_effort: None,
            seed: None,
            presence_penalty: None,
            frequency_penalty: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            allowed_tools: None,
            compress_after_messages: None,
            label: String::new(),
            mock_text: None,
            mock_steps: None,
            mock_delay_ms: None,
            tool_error_limit: DEFAULT_TOOL_ERROR_LIMIT,
            repeat_call_limit: DEFAULT_REPEAT_CALL_LIMIT,
            max_tool_results: DEFAULT_MAX_TOOL_RESULTS,
            parallel_read_tools: true,
            stale_result_placeholders: true,
            dedup_reads: true,
            max_tool_result_bytes: crate::registry::tool::DEFAULT_TOOL_RESULT_BYTES,
            anonymize_thinking: false,
        }
    }
}

/// A step event emitted by the streaming chat variant of the ReAct loop.
#[derive(Debug, Clone)]
pub enum ReactEvent {
    /// A completed assistant turn that answered without tool calls.
    Assistant {
        /// The answer text.
        text: String,
        /// The reasoning that preceded the answer, when the model produced any.
        reasoning: String,
    },
    /// Text the assistant produced in a turn that also called tools.
    ///
    /// The live client already streamed this text, so it is not re-sent for
    /// display; a transcript needs it to replay the conversation faithfully.
    AssistantText {
        /// The text of the turn (never empty).
        text: String,
    },
    /// A tool call that is about to run.
    ///
    /// Emitted before the tool starts so the UI can show what is happening
    /// while a slow command or a large edit is still in flight.
    ToolStart {
        /// Provider-assigned id of the call.
        call_id: String,
        /// The tool name.
        name: String,
        /// One-line summary of the arguments (may be empty).
        summary: String,
    },
    /// Incremental output of a still-running tool.
    ToolProgress {
        /// Provider-assigned id of the call.
        call_id: String,
        /// Trailing output lines, already trimmed.
        tail: String,
    },
    /// A tool invocation with its (still anonymized) textual result.
    Tool {
        /// Provider-assigned id of the call.
        call_id: String,
        /// The tool name.
        name: String,
        /// Whether the call succeeded.
        ok: bool,
        /// Wall-clock duration of the call in milliseconds.
        elapsed_ms: u64,
        /// The tool's textual result.
        content: String,
    },
}

/// The result of a completed ReAct run.
#[derive(Debug, Clone)]
pub struct ReactOutcome {
    /// The final assistant text (still anonymized).
    pub text: String,
    /// The mutated conversation context after the run.
    pub context: ContextManager,
    /// Token usage accumulated across all completions of the run.
    pub usage: Usage,
}

/// Runs the ReAct loop until the model answers without tool calls.
///
/// Returns the final text together with the mutated context and accumulated
/// usage. The text remains anonymized so callers decide when to restore.
pub async fn run_react(
    ctx: &mut ExecutionContext,
    context: ContextManager,
    opts: &ReactOptions,
) -> DaemonResult<ReactOutcome> {
    react_loop(ctx, context, opts, None, None).await.map_err(|(err, _)| err)
}

/// Streaming variant of [`run_react`] used by the ReAct chat RPC.
///
/// Emits one [`ReactEvent`] per assistant turn and tool call. When `on_delta`
/// is provided the LLM call is streamed and each delta is forwarded, allowing
/// token-level output. On error the partially mutated context is returned so
/// callers can persist an interrupted session.
pub async fn run_react_streaming(
    ctx: &mut ExecutionContext,
    context: ContextManager,
    opts: &ReactOptions,
    on_delta: Option<&mut (dyn FnMut(StreamDelta) + Send)>,
    on_event: &mut (dyn FnMut(ReactEvent) + Send),
) -> Result<ReactOutcome, (DaemonError, ContextManager)> {
    react_loop(ctx, context, opts, on_delta, Some(on_event)).await
}

/// The shared ReAct iteration loop behind [`run_react`] and
/// [`run_react_streaming`].
///
/// Errors carry the context mutated so far so streaming callers can persist a
/// partial session; [`run_react`] discards it to keep the plain signature.
async fn react_loop(
    ctx: &mut ExecutionContext,
    context: ContextManager,
    opts: &ReactOptions,
    on_delta: Option<&mut (dyn FnMut(StreamDelta) + Send)>,
    on_event: Option<&mut (dyn FnMut(ReactEvent) + Send)>,
) -> Result<ReactOutcome, (DaemonError, ContextManager)> {
    // Tools that only queue context operations need to know a loop will drain
    // them; the mark is cleared on the way out so a following blueprint node
    // does not inherit it.
    ctx.in_react_loop = true;
    let outcome = react_loop_inner(ctx, context, opts, on_delta, on_event).await;
    ctx.in_react_loop = false;
    outcome
}

/// The body of [`react_loop`], without the loop-lifetime bookkeeping.
async fn react_loop_inner(
    ctx: &mut ExecutionContext,
    mut context: ContextManager,
    opts: &ReactOptions,
    mut on_delta: Option<&mut (dyn FnMut(StreamDelta) + Send)>,
    mut on_event: Option<&mut (dyn FnMut(ReactEvent) + Send)>,
) -> Result<ReactOutcome, (DaemonError, ContextManager)> {
    let llm_defaults = llm_default_config(ctx).await;
    let mut client = match build_client(ctx, opts, &llm_defaults) {
        Ok(client) => client,
        Err(err) => return Err((err, context)),
    };
    let params = build_params(opts, &llm_defaults);
    let tools = tool_definitions(&ctx.registry, opts.allowed_tools.as_ref());
    let anonymizer = build_anonymizer(ctx).await;
    let billing = billing_config(ctx).await;
    let policy = crate::llm::RetryPolicy::from_config(&llm_defaults);

    let mut total_usage = Usage::default();
    let mut final_text = String::new();
    // Set when the model finished on its own, which is what separates "answered
    // with nothing to say" from "ran out of turns".
    let mut answered = false;
    // Consecutive tool failures and repeated `(name, arguments)` signatures.
    let mut consecutive_errors: u32 = 0;
    let mut repeat_counts: HashMap<String, u32> = HashMap::new();
    let mut notices = BudgetNotices::new(&llm_defaults, opts.max_iterations);
    let execution = execution_config(ctx).await;
    let auto_wake = execution.job_auto_wake;
    // Job ids whose completion this conversation already reported.
    let mut announced: Vec<String> = Vec::new();
    if !ctx.todos.is_empty() {
        // A resumed run keeps its plan: the tool result that produced it may
        // have been evicted, and the model must not lose track of the work.
        let rendered = metteur_shared::llm::render_todos(&ctx.todos);
        context
            .push_message(Message::text(Role::User, format!("[engine] current plan:\n{rendered}")));
    }

    let budget = opts.max_iterations + notices.wrap_up_turns();
    for iteration in 0..budget {
        let wrapping = notices.is_wrap_up(iteration, opts.max_iterations);
        // Honor cancellation and pause requests. A cancelled turn hands the
        // context back so the caller can persist it.
        if let Err(err) = crate::execution::control::gate(ctx).await {
            return Err((err, context));
        }

        // Inject deferred normal interrupts and any urgent/emergency
        // interrupts queued before this LLM call.
        while let Some(msg) = ctx.pending_normal.pop_front() {
            context.push_message(Message::text(Role::User, msg));
        }
        if let Some(bus) = &ctx.interrupts {
            // A queued (normal) message from a client lands at the next LLM
            // call, which is the same point the interpreter injects it. Without
            // this drain a message typed into a running chat would be lost.
            for msg in bus.drain(InterruptPriority::Normal) {
                context.push_message(Message::text(Role::User, msg));
            }
            for msg in bus.drain(InterruptPriority::Urgent) {
                context.push_message(Message::text(Role::User, msg));
            }
            for msg in bus.drain(InterruptPriority::Emergency) {
                context.push_message(Message::text(Role::User, msg));
            }
        }

        // Tell the model how much room is left before it runs out of turns;
        // a wrap-up turn runs without tools so it has to answer.
        if let Some(notice) = notices.notice_for(iteration, opts.max_iterations) {
            context.push_message(Message::text(Role::User, notice));
        }
        let turn_tools: &[ToolDefinition] = if wrapping {
            &[]
        } else {
            &tools
        };

        // Compress long contexts before issuing the next request.
        compress_if_needed(ctx, &client, &mut context, &params, opts, &billing, &mut total_usage)
            .await;

        // Race the LLM call against an emergency interrupt so that an
        // emergency message can abort an in-flight request. The streamed
        // variant forwards deltas through `on_delta` as they arrive.
        //
        // The model sees an anonymized shadow copy; stored context keeps the
        // original text so chat restore and audit remain readable.
        let response = {
            let outbound = outbound_context(&context, &anonymizer, opts.anonymize_thinking).await;
            match request_with_retry(
                ctx,
                &mut client,
                opts,
                &llm_defaults,
                &policy,
                &outbound,
                &params,
                turn_tools,
                &mut on_delta,
            )
            .await
            {
                Ok(LlmRace::Response(resp)) => resp,
                Ok(LlmRace::Emergency(msg)) => {
                    context.push_message(Message::text(Role::User, msg));
                    continue;
                }
                Err(err) => return Err((err, context)),
            }
        };

        record_usage(ctx, client.as_ref(), &response.usage, &billing).await;
        total_usage.input_tokens += response.usage.input_tokens;
        total_usage.output_tokens += response.usage.output_tokens;
        total_usage.reasoning_tokens += response.usage.reasoning_tokens;
        total_usage.total_tokens += response.usage.total_tokens;

        if response.tool_calls.is_empty() {
            // The model wants to finish. If it still has background jobs, park
            // here and let the engine wake it when one completes — that is what
            // removes the "sleep a little and check again" pattern.
            if auto_wake {
                let finished = match collect_finished_job(ctx, &mut announced).await {
                    Ok(finished) => finished,
                    Err(err) => return Err((err, context)),
                };
                if let Some(finished) = finished {
                    // Keep the model's "I will wait" text, then hand it the
                    // result of the job that just ended.
                    if !response.text.is_empty() {
                        context.push_message(Message::text(Role::Assistant, response.text.clone()));
                    }
                    // The `job` execution event already tells the UI; the
                    // notice here is what the model reads.
                    let notice =
                        job_notice(&finished, &ctx.jobs, execution.job_tail_lines as usize);
                    notify_job(ctx, &finished);
                    context.push_message(Message::text(Role::User, notice));
                    continue;
                }
            }
            final_text = response.text.clone();
            answered = true;
            if let Some(cb) = on_event.as_deref_mut() {
                cb(ReactEvent::Assistant {
                    text: final_text.clone(),
                    reasoning: thinking_text(&response),
                });
            }
            break;
        }
        if wrapping {
            // The wrap-up turn had no tools to call; asking for one means the
            // model cannot produce an answer and the run must fail loudly.
            return Err((
                DaemonError::Execution(
                    "iteration budget exhausted without a final answer".to_string(),
                ),
                context,
            ));
        }

        // Record the assistant turn with its tool calls.
        //
        // Any text the model produced belongs to *this* message: a separate
        // assistant message before the results would break the provider
        // contract (every tool result must directly follow the call that
        // requested it), and the next request would be rejected.
        context.push_message(Message {
            role: Role::Assistant,
            content: thinking_blocks(&response),
            tool_calls: response.tool_calls.clone(),
            tool_call_id: None,
        });

        // Execute each tool call and mix the results into the context.
        //
        // Failures are reported to the model as result text instead of
        // aborting the run: a single rejected edit must not discard a whole
        // turn. Consecutive failures are bounded so a broken loop cannot spin.
        //
        // The conversation is offered to tools that ask for it (SubAgent
        // inheritance); the clone happens only for those calls.
        if response.tool_calls.iter().any(|call| call.name == "SpawnSubAgent") {
            ctx.parent_context = Some(context.clone());
        }
        // Announce every call before running it: a slow command or a large edit
        // would otherwise leave the UI with nothing to show for minutes.
        if let Some(cb) = on_event.as_deref_mut() {
            // Text that accompanies tool calls is part of the conversation and
            // belongs in a transcript, even though the live client has already
            // displayed it via the delta stream.
            let text = response.text.trim();
            if !text.is_empty() {
                cb(ReactEvent::AssistantText {
                    text: response.text.clone(),
                });
            }
            for call in &response.tool_calls {
                cb(ReactEvent::ToolStart {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    summary: crate::registry::tool::summarize_call(&call.arguments),
                });
            }
        }
        let outcomes =
            execute_tool_calls(ctx, &response.tool_calls, opts, &anonymizer, &mut on_event).await;
        ctx.parent_context = None;
        if let Err(error) = ctx.transaction_log.ensure_healthy() {
            return Err((error, context));
        }

        // Context operations requested by the calls above apply to the results
        // already in the context, never to the batch that requested them.
        apply_context_ops(ctx, &mut context);

        for (call, outcome) in response.tool_calls.iter().zip(outcomes) {
            let ToolOutcome {
                result: outcome,
                read_paths: paths,
                full_reads,
                elapsed_ms,
            } = outcome;
            let outcome = match outcome {
                Err(error @ DaemonError::Persistence(_)) => return Err((error, context)),
                other => other,
            };
            // A complete read replaces an earlier complete read of the same
            // paths: keeping both would duplicate content the model already
            // has. The check runs per call so the paths are attributed to the
            // tool that produced them.
            if opts.dedup_reads && !full_reads.is_empty() {
                let superseded = context.supersede_reads(&call.name, &full_reads);
                if superseded > 0 {
                    ctx.audit("context.supersede", serde_json::json!({ "results": superseded }));
                }
            }
            let (text, ok) = match outcome {
                Ok(text) => {
                    consecutive_errors = 0;
                    (text, true)
                }
                Err(err) => {
                    consecutive_errors += 1;
                    let message = format!("Error: {err}");
                    ctx.audit(
                        "tool.error",
                        serde_json::json!({ "name": call.name, "error": err.to_string() }),
                    );
                    if opts.tool_error_limit > 0 && consecutive_errors >= opts.tool_error_limit {
                        return Err((
                            DaemonError::Execution(format!(
                                "{consecutive_errors} consecutive tool failures; last error from \
                                 '{}': {err}",
                                call.name
                            )),
                            context,
                        ));
                    }
                    (message, false)
                }
            };

            // Warn the model when it repeats an identical call, and abort once
            // the repetition budget is exhausted.
            let signature = format!("{}\u{0}{}", call.name, call.arguments);
            let repeats = repeat_counts.entry(signature).or_insert(0);
            *repeats += 1;
            let repeat_note = if *repeats > 1 {
                if opts.repeat_call_limit > 0 && *repeats > opts.repeat_call_limit {
                    return Err((
                        DaemonError::Execution(format!(
                            "tool '{}' called {} times with identical arguments",
                            call.name, repeats
                        )),
                        context,
                    ));
                }
                format!(
                    "\n[note: identical {} call #{}; re-read the result or change the approach]",
                    call.name, repeats
                )
            } else {
                String::new()
            };

            let capped = truncate_result(&text, tool_result_budget(ctx, &call.name, opts).await);
            let displayed = format!("{capped}{repeat_note}");

            if let Some(cb) = on_event.as_deref_mut() {
                cb(ReactEvent::Tool {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    ok,
                    elapsed_ms,
                    content: displayed.clone(),
                });
            }
            let mixed = anonymizer.anonymize(&displayed).await;
            // Retention comes from the tool's own declaration so reads survive
            // across turns while one-shot mutations can be evicted early.
            let lifetime = ctx.registry.tool(&call.name).map(|t| t.lifetime()).unwrap_or_default();
            context.mix_in_tool_result(ToolResult {
                tool_call_id: call.id.clone(),
                tool: call.name.clone(),
                content: mixed,
                timestamp: now_millis(),
                lifetime,
                paths,
            });
        }

        // Expire reads of files mutated by the calls above (see
        // `ContextManager::invalidate_paths`), then bound the retained results.
        let mutated = drain_mutated_paths(ctx);
        if opts.stale_result_placeholders && !mutated.is_empty() {
            let invalidated = context.invalidate_paths(&mutated);
            if invalidated > 0 {
                ctx.audit(
                    "context.stale",
                    serde_json::json!({ "paths": mutated.len(), "results": invalidated }),
                );
            }
        }
        if opts.max_tool_results > 0 {
            context.evict(metteur_shared::EvictionPolicy::Default, opts.max_tool_results);
        }
    }

    if !answered {
        // Running out of turns is a failure; the wrap-up turn exists exactly so
        // this does not happen, and its absence means the model never stopped
        // asking for tools.
        return Err((
            DaemonError::Execution("iteration budget exhausted without a final answer".to_string()),
            context,
        ));
    }
    Ok(ReactOutcome {
        text: final_text,
        context,
        usage: total_usage,
    })
}

/// Reads the `[execution]` section of the merged workspace configuration.
async fn execution_config(ctx: &ExecutionContext) -> metteur_shared::config::ExecutionConfig {
    match &ctx.config {
        Some(config) => config.read().await.execution.clone(),
        None => metteur_shared::config::ExecutionConfig::default(),
    }
}

/// Waits for the next background job of this run to finish.
///
/// Returns `None` when the run has already collected every finished job — the
/// caller then ends the turn. Cancellation kills the run's jobs and aborts the
/// wait, so a parked turn never outlives a cancel.
async fn collect_finished_job(
    ctx: &ExecutionContext,
    announced: &mut Vec<String>,
) -> DaemonResult<Option<JobSnapshot>> {
    if let Some(finished) = ctx.jobs.first_finished(ctx.run_id, announced) {
        announced.push(finished.id.clone());
        return Ok(Some(finished));
    }
    if !ctx.jobs.any_running(ctx.run_id) {
        return Ok(None);
    }
    notify(
        ctx,
        format!(
            "[engine] waiting for {} background job(s) to finish; you will be woken when one              completes",
            ctx.jobs.list(Some(ctx.run_id)).iter().filter(|job| job.state.is_running()).count()
        ),
    );
    let jobs = Arc::clone(&ctx.jobs);
    let owner = ctx.run_id;
    let known = announced.clone();
    let cancel = Arc::clone(&ctx.cancel_requested);
    let finished = tokio::select! {
        finished = jobs.wait_any_finished(owner, &known) => finished,
        _ = crate::execution::jobs::wait_for_cancel(cancel) => {
            ctx.jobs.kill_owned_by(owner);
            return Err(DaemonError::Interrupted(
                "cancelled while waiting for a background command".to_string(),
            ));
        }
    };
    if let Some(job) = &finished {
        announced.push(job.id.clone());
    }
    Ok(finished)
}

/// Renders the engine's wake-up notice for a finished job.
fn job_notice(job: &JobSnapshot, jobs: &JobManager, tail_lines: usize) -> String {
    let tail = jobs.tail(&job.id, tail_lines.max(1));
    let output = match tail.trim() {
        "" => "(no output)".to_string(),
        text => text.to_string(),
    };
    format!(
        "[engine] background job {} {} after {:.1}s. Output (tail):
{output}",
        job.id,
        job.state_label(),
        job.duration_ms() as f64 / 1000.0,
    )
}

/// Announces a finished job to the event stream.
fn notify_job(ctx: &ExecutionContext, job: &JobSnapshot) {
    if let Some(tx) = &ctx.events {
        let _ = tx.send(crate::execution::ExecutionEvent::Job {
            node_id: ctx.current_node,
            job_id: job.id.clone(),
            state: "finished".to_string(),
            summary: job.summary(),
        });
    }
}

/// Applies the context operations queued by tool calls.
fn apply_context_ops(ctx: &mut ExecutionContext, context: &mut ContextManager) {
    let ops = std::mem::take(&mut ctx.context_ops);
    for op in ops {
        match op {
            crate::execution::context::ContextOp::Release(query) => {
                let report = context.release(&query);
                ctx.audit(
                    "context.release",
                    serde_json::json!({
                        "released": report.released,
                        "retained": report.retained,
                        "freed_tokens": report.estimated_tokens,
                        "by_tool": report.by_tool,
                    }),
                );
                context.push_message(Message::text(Role::User, report.notice()));
            }
        }
    }
}

/// Renders the reasoning text of a response as one string.
fn thinking_text(response: &LlmResponse) -> String {
    response
        .thinking
        .iter()
        .map(|block| block.text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts a response's reasoning into content blocks for the context.
///
/// Thinking has to travel with the turn it belongs to: Anthropic rejects a
/// replayed turn whose thinking was stripped while its tool calls were kept,
/// and a redacted block must be passed back verbatim. Providers that forbid
/// replayed reasoning (DeepSeek, OpenAI) ignore these blocks when serializing.
///
/// Empty blocks are dropped — the API rejects empty text and thinking blocks.
fn thinking_blocks(response: &LlmResponse) -> Vec<ContentBlock> {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    for block in &response.thinking {
        if let Some(redacted) = block.redacted.clone().filter(|data| !data.is_empty()) {
            blocks.push(ContentBlock::RedactedThinking {
                data: redacted,
            });
        } else if !block.text.is_empty() {
            blocks.push(ContentBlock::Thinking {
                text: block.text.clone(),
                signature: block.signature.clone(),
            });
        }
    }
    if !response.text.is_empty() {
        blocks.push(ContentBlock::Text(response.text.clone()));
    }
    blocks
}

/// Iteration-budget notices for one run.
///
/// The model is told when the budget runs low and again when it is exhausted;
/// each notice is emitted once so a retry loop cannot repeat them.
struct BudgetNotices {
    /// Whether the low-budget notice is enabled.
    enabled: bool,
    /// Turns granted after the budget is spent, without tools.
    wrap_up: usize,
    /// Whether the low-budget notice was already emitted.
    warned: bool,
    /// Whether the exhausted-budget notice was already emitted.
    exhausted: bool,
}

impl BudgetNotices {
    fn new(config: &LlmConfig, max_iterations: usize) -> Self {
        Self {
            enabled: config.budget_notice,
            wrap_up: if max_iterations == 0 {
                0
            } else {
                config.wrap_up_iterations as usize
            },
            warned: false,
            exhausted: false,
        }
    }

    /// Extra tool-free turns granted past the budget.
    fn wrap_up_turns(&self) -> usize {
        self.wrap_up
    }

    /// Whether the turn at `iteration` is a wrap-up turn.
    fn is_wrap_up(&self, iteration: usize, max_iterations: usize) -> bool {
        iteration >= max_iterations
    }

    /// The notice to append before the turn at `iteration`, if any.
    fn notice_for(&mut self, iteration: usize, max_iterations: usize) -> Option<String> {
        if iteration >= max_iterations {
            if self.exhausted {
                return None;
            }
            self.exhausted = true;
            return Some(
                "[engine] iteration budget exhausted. Answer now with what is done and what \
                 remains; no more tool calls will run."
                    .to_string(),
            );
        }
        if !self.enabled || self.warned {
            return None;
        }
        let remaining = max_iterations - iteration;
        let threshold = (max_iterations / 5).max(1);
        if remaining > threshold {
            return None;
        }
        self.warned = true;
        Some(format!(
            "[engine] iteration budget: {remaining} of {max_iterations} turns remaining. Finish \
             the current step, then answer with what is done and what remains."
        ))
    }
}

/// Sends one model request, retrying transient failures and switching to a
/// fallback model once the retry budget is spent.
///
/// A streamed call is only retried while no delta has been forwarded: replaying
/// a request whose text already reached the client would duplicate output.
#[allow(clippy::too_many_arguments)]
async fn request_with_retry(
    ctx: &mut ExecutionContext,
    client: &mut Arc<dyn LlmClient>,
    opts: &ReactOptions,
    defaults: &LlmConfig,
    policy: &crate::llm::RetryPolicy,
    context: &ContextManager,
    params: &GenerationParams,
    tools: &[ToolDefinition],
    delta: &mut Option<&mut (dyn FnMut(StreamDelta) + Send)>,
) -> DaemonResult<LlmRace> {
    let mut attempts: u32 = 0;
    // Models already attempted for this request: a fallback chain must never
    // revisit one, or two failing models would bounce a request forever.
    let mut tried: Vec<String> = Vec::new();
    if let Some(key) = opts.model.clone().or_else(|| defaults.default_model.clone()) {
        tried.push(key);
    }
    loop {
        ctx.audit("llm.request", serde_json::json!({"run_id": ctx.run_id.to_string(), "model": client.model()}));
        let mut emitted = false;
        let result = match delta.as_deref_mut() {
            Some(cb) => {
                let mut tracked = |d: StreamDelta| {
                    emitted = true;
                    cb(d);
                };
                race_call(
                    client.as_ref(),
                    context,
                    params,
                    tools,
                    &ctx.interrupts,
                    Some(&mut tracked),
                )
                .await
            }
            None => race_call(client.as_ref(), context, params, tools, &ctx.interrupts, None).await,
        };
        match result {
            Ok(race) => {
                if matches!(race, LlmRace::Emergency(_)) { record_unavailable_usage(ctx, client.as_ref()); }
                return Ok(race);
            }
            Err(err) => {
                record_unavailable_usage(ctx, client.as_ref());
                if !crate::llm::RetryPolicy::retryable(&err) || emitted {
                    return Err(err);
                }
                if attempts < policy.max_retries {
                    attempts += 1;
                    let delay = policy.delay_for(attempts - 1);
                    ctx.audit(
                        "llm.retry",
                        serde_json::json!({
                            "attempt": attempts,
                            "max_retries": policy.max_retries,
                            "delay_ms": delay.as_millis() as u64,
                            "error": err.to_string(),
                        }),
                    );
                    notify(
                        ctx,
                        format!(
                            "[engine] provider error ({err}); retrying in {:.1}s (attempt \
                             {attempts}/{})",
                            delay.as_secs_f64(),
                            policy.max_retries
                        ),
                    );
                    tokio::time::sleep(delay).await;
                    if ctx.cancel_requested.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(DaemonError::Interrupted("cancelled by user".to_string()));
                    }
                    continue;
                }
                // Retries are spent: fall back to the next usable model. A
                // candidate that cannot even be built (bad endpoint, unknown
                // api_type) is skipped rather than masking the real error.
                let (next, fallback) = loop {
                    let Some(candidate) = next_fallback(policy, defaults, &tried) else {
                        return Err(err);
                    };
                    tried.push(candidate.clone());
                    let mut fallback_opts = opts.clone();
                    fallback_opts.model = Some(candidate.clone());
                    match build_client(ctx, &fallback_opts, defaults) {
                        Ok(client) => break (candidate, client),
                        Err(build_error) => {
                            tracing::warn!(
                                "[{}] fallback model '{candidate}' is unusable: {build_error}",
                                opts.label
                            );
                        }
                    }
                };
                ctx.audit(
                    "llm.fallback",
                    serde_json::json!({ "to": next, "error": err.to_string() }),
                );
                notify(ctx, format!("[engine] switching to fallback model '{next}' after: {err}"));
                *client = fallback;
                attempts = 0;
            }
        }
    }
}

/// Returns the next model in the fallback chain that has not been tried yet.
///
/// The configured default model is part of the chain: an explicit per-call
/// model that fails should be able to fall back to the default one.
fn next_fallback(
    policy: &crate::llm::RetryPolicy,
    defaults: &LlmConfig,
    tried: &[String],
) -> Option<String> {
    std::iter::once(defaults.default_model.clone())
        .chain(policy.fallback_models.iter().cloned().map(Some))
        .flatten()
        .find(|candidate| !tried.iter().any(|name| name == candidate))
}

/// Emits an engine notice as an audit entry and a live event.
fn notify(ctx: &ExecutionContext, message: String) {
    ctx.audit("llm.notice", serde_json::json!({ "message": message }));
    if let Some(tx) = &ctx.events {
        let _ = tx.send(crate::execution::ExecutionEvent::Message {
            node_id: ctx.current_node,
            message,
        });
    }
}

/// Builds the outbound snapshot sent to the model.
///
/// System fragments, message text and tool-call arguments are anonymized
/// through `anonymizer` (stable tokens, prefix-cache friendly). The stored
/// context is untouched: chat restore and audit keep the original text.
///
/// Thinking blocks are passed through verbatim by default. Providers that
/// verify them (Anthropic) reject a replayed block whose text was rewritten,
/// so anonymizing reasoning text is opt-in and drops the signature.
async fn outbound_context(
    context: &ContextManager,
    anonymizer: &Anonymizer,
    anonymize_thinking: bool,
) -> ContextManager {
    if !anonymizer.is_enabled() {
        return context.clone();
    }
    let mut outbound = context.clone();
    for fragment in &mut outbound.system_fragments {
        fragment.content = anonymizer.anonymize(&fragment.content).await;
    }
    for message in &mut outbound.messages {
        for block in &mut message.content {
            match block {
                ContentBlock::Text(text) => {
                    *text = anonymizer.anonymize(text).await;
                }
                ContentBlock::Thinking {
                    text,
                    signature,
                } => {
                    if anonymize_thinking {
                        *text = anonymizer.anonymize(text).await;
                        *signature = None;
                    }
                }
                ContentBlock::RedactedThinking {
                    ..
                } => {}
            }
        }
        for call in &mut message.tool_calls {
            let serialized = anonymizer.anonymize(&call.arguments.to_string()).await;
            if let Ok(arguments) = serde_json::from_str(&serialized) {
                call.arguments = arguments;
            }
        }
    }
    outbound
}

/// The outcome of the raced LLM call.
enum LlmRace {
    /// The model responded before any emergency interrupt arrived.
    Response(LlmResponse),
    /// An emergency interrupt cancelled the call; its message must be injected.
    Emergency(String),
}

/// Calls the model, racing the request against an emergency interrupt so the
/// interrupt can abort an in-flight call. The race is isolated here to keep the
/// loop's borrows simple; `on_delta` selects the streaming transport.
async fn race_call(
    client: &dyn LlmClient,
    context: &ContextManager,
    params: &GenerationParams,
    tools: &[ToolDefinition],
    bus: &Option<InterruptBus>,
    on_delta: Option<&mut (dyn FnMut(StreamDelta) + Send)>,
) -> DaemonResult<LlmRace> {
    match bus {
        Some(bus) => {
            let bus = bus.clone();
            tokio::select! {
                resp = dispatch(client, context, params, tools, on_delta) => {
                    resp.map(LlmRace::Response)
                }
                msg = bus.wait_emergency() => Ok(LlmRace::Emergency(msg.unwrap_or_default())),
            }
        }
        None => dispatch(client, context, params, tools, on_delta).await.map(LlmRace::Response),
    }
}

/// Sends the request over the streamed or plain transport.
async fn dispatch(
    client: &dyn LlmClient,
    context: &ContextManager,
    params: &GenerationParams,
    tools: &[ToolDefinition],
    on_delta: Option<&mut (dyn FnMut(StreamDelta) + Send)>,
) -> DaemonResult<LlmResponse> {
    match on_delta {
        Some(delta) => client.stream(context, params, tools, delta).await,
        None => client.complete(context, params, tools).await,
    }
}

/// Returns the LLM defaults from the merged workspace configuration.
async fn llm_default_config(ctx: &ExecutionContext) -> LlmConfig {
    match &ctx.config {
        Some(config) => config.read().await.llm.clone(),
        None => LlmConfig::default(),
    }
}

/// Billing context threaded through usage recording: currency, reporting
/// time zone (peak-window evaluation) and per-model configuration table
/// (prices live in `llm.models`).
pub(crate) type BillingCtx = (String, String, HashMap<String, LlmModelConfig>);

/// Reads the billing section + model table of the merged workspace config.
///
/// The table is indexed by config key *and* by `model_id`, because the client
/// reports the provider id (`model_id`) while configuration is keyed by the
/// user-facing model name.
async fn billing_config(ctx: &ExecutionContext) -> Option<BillingCtx> {
    match &ctx.config {
        Some(config) => {
            let cfg = config.read().await;
            let mut models = cfg.llm.models.clone();
            for (key, model) in &cfg.llm.models {
                if !model.model_id.is_empty() && model.model_id != *key {
                    models.entry(model.model_id.clone()).or_insert_with(|| model.clone());
                }
            }
            Some((cfg.billing.currency.clone(), cfg.billing.timezone.clone(), models))
        }
        None => None,
    }
}

/// Builds an anonymizer from the anonymization configuration.
pub(crate) async fn build_anonymizer(ctx: &ExecutionContext) -> Anonymizer {
    match &ctx.config {
        Some(config) => {
            let anonymize = config.read().await.anonymize.clone();
            Anonymizer::from_config(&anonymize)
        }
        None => Anonymizer::disabled(),
    }
}

/// Builds an LLM client from the run options, resolving the model table.
///
/// The configured model entry (`llm.models[<key>]`) is authoritative for the
/// connection: its `api_type`, `api_endpoint`, `api_key` and `model_id` drive
/// the request, so a caller only has to name the model. Explicit per-call
/// `base_url`/`api_key` overrides still win, and an unconfigured model falls
/// back to the plain provider defaults.
fn build_client(
    ctx: &ExecutionContext,
    opts: &ReactOptions,
    defaults: &LlmConfig,
) -> DaemonResult<Arc<dyn LlmClient>> {
    if opts.provider == "mock" {
        let delay_ms = std::time::Duration::from_millis(opts.mock_delay_ms.unwrap_or(0));
        let steps = match &opts.mock_steps {
            Some(steps) if !steps.is_empty() => steps.clone(),
            _ => {
                let text = opts.mock_text.clone().unwrap_or_else(|| "mock response".to_string());
                vec![MockStep::Text(text)]
            }
        };
        return Ok(Arc::new(MockClient::new_delayed(steps, delay_ms)));
    }

    let model_key = opts
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .or_else(|| defaults.default_model.clone())
        .filter(|m| !m.is_empty())
        .or_else(|| {
            (defaults.models.len() == 1)
                .then(|| defaults.models.keys().next().cloned())
                .flatten()
        });
    let model_cfg = model_key.as_ref().and_then(|key| defaults.models.get(key));

    // A configured `api_type` selects the provider; the option value is the
    // fallback for callers that drive a bare provider without a model entry.
    let provider = model_cfg
        .map(|cfg| cfg.api_type.as_str())
        .filter(|api_type| !api_type.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| opts.provider.clone());
    let kind = match provider.as_str() {
        "openai-chat" | "openai" => ProviderKind::OpenAiChat,
        "anthropic" => ProviderKind::Anthropic,
        "openai-responses" => ProviderKind::OpenAiResponses,
        other => return Err(DaemonError::Execution(format!("unknown llm provider '{other}'"))),
    };

    let model_key = model_key.ok_or_else(|| {
        let mut available: Vec<&String> = defaults.models.keys().collect();
        available.sort();
        DaemonError::Execution(if available.is_empty() {
            "no model configured; add one under Settings > LLM & Models".to_string()
        } else {
            format!(
                "no model selected and no default is set; configured models: {}",
                available.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            )
        })
    })?;
    // The id sent to the provider usually differs from the config key.
    let model =
        model_cfg.map(|cfg| cfg.model_id.clone()).filter(|id| !id.is_empty()).unwrap_or(model_key);
    let base_url = opts
        .base_url
        .clone()
        .filter(|url| !url.is_empty())
        .or_else(|| model_cfg.map(|cfg| cfg.api_endpoint.clone()).filter(|url| !url.is_empty()))
        .unwrap_or_else(|| default_base_url(kind).to_string());
    let api_key = opts
        .api_key
        .clone()
        .filter(|key| !key.is_empty())
        .or_else(|| model_cfg.map(|cfg| cfg.api_key.clone()).filter(|key| !key.is_empty()))
        .unwrap_or_default();

    let config = LlmProviderConfig::new(kind, base_url, api_key, model)
        .with_model_settings(defaults, model_cfg);
    ctx.llm_factory.create(&config).map_err(|e| DaemonError::Llm(e.to_string()))
}

fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiChat | ProviderKind::OpenAiResponses => "https://api.openai.com/v1",
        ProviderKind::Anthropic => "https://api.anthropic.com/v1",
    }
}

/// Builds generation parameters from the options over configured defaults.
fn build_params(opts: &ReactOptions, defaults: &LlmConfig) -> GenerationParams {
    GenerationParams {
        temperature: opts.temperature.or(defaults.temperature),
        top_p: opts.top_p,
        max_tokens: opts.max_tokens,
        stop: opts.stop.clone(),
        reasoning_effort: opts.reasoning_effort,
        seed: opts.seed,
        presence_penalty: opts.presence_penalty,
        frequency_penalty: opts.frequency_penalty,
    }
}

/// Builds tool definitions from the registry, restricted by `allowed`.
///
/// Definitions are name-sorted: `Registry::tools` iterates a hash map, so
/// without the sort the tools array would reorder between requests and break
/// the provider's prefix cache on every call.
fn tool_definitions(
    registry: &crate::registry::Registry,
    allowed: Option<&HashSet<String>>,
) -> Vec<ToolDefinition> {
    let mut tools: Vec<_> = registry
        .tools()
        .into_iter()
        .filter(|t| allowed.map(|set| set.contains(t.name())).unwrap_or(true))
        .collect();
    tools.sort_by(|a, b| a.name().cmp(b.name()));
    tools
        .into_iter()
        .map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters(),
        })
        .collect()
}

/// Resolves the byte budget for one tool result.
async fn tool_result_budget(ctx: &ExecutionContext, name: &str, opts: &ReactOptions) -> usize {
    let declared = ctx.registry.tool(name).map(|t| t.max_result_bytes()).unwrap_or(0);
    if declared > 0 {
        return declared;
    }
    match &ctx.config {
        Some(config) => {
            let configured = config.read().await.llm.max_tool_result_bytes;
            if configured > 0 {
                configured as usize
            } else {
                opts.max_tool_result_bytes
            }
        }
        None => opts.max_tool_result_bytes,
    }
}

/// Drains the paths mutated by tool calls since the last drain.
fn drain_mutated_paths(ctx: &mut ExecutionContext) -> Vec<std::path::PathBuf> {
    std::mem::take(&mut ctx.mutated_paths)
}

/// The result of one tool call, with the read state it declared.
struct ToolOutcome {
    /// The textual result, or the failure the model is told about.
    result: DaemonResult<String>,
    /// Workspace paths the call read.
    read_paths: Vec<std::path::PathBuf>,
    /// Workspace paths the call read *in full*.
    full_reads: Vec<std::path::PathBuf>,
    /// Wall-clock duration of the call, in milliseconds.
    elapsed_ms: u64,
}

/// Executes the tool calls of one assistant turn, returning each result in
/// call order together with the workspace paths that call read.
///
/// Calls that the run forbids or the registry does not know yield an error
/// string (the model can recover from those). Read-only calls run concurrently
/// when `parallel_read_tools` is set; mutating calls stay sequential so their
/// side effects keep a deterministic order.
///
/// A sequential call runs against a progress channel: whatever the tool reports
/// (a long command's trailing output) is forwarded as [`ReactEvent::ToolProgress`]
/// while the call is still in flight. Concurrent reads are short by nature and
/// report nothing.
async fn execute_tool_calls(
    ctx: &mut ExecutionContext,
    calls: &[ToolCall],
    opts: &ReactOptions,
    anonymizer: &Anonymizer,
    on_event: &mut Option<&mut (dyn FnMut(ReactEvent) + Send)>,
) -> Vec<ToolOutcome> {
    let parallel = opts.parallel_read_tools
        && calls.len() > 1
        && calls.iter().all(|call| is_read_only_tool(ctx, &call.name));
    if !parallel {
        let mut out = Vec::with_capacity(calls.len());
        for call in calls {
            ctx.read_paths.clear();
            ctx.read_paths_full.clear();
            let started = std::time::Instant::now();
            let result = run_with_progress(ctx, call, opts, anonymizer, on_event).await;
            let fatal = matches!(&result, Err(DaemonError::Persistence(_)))
                || ctx.transaction_log.ensure_healthy().is_err();
            out.push(ToolOutcome {
                result,
                read_paths: drain_read_paths(ctx),
                full_reads: std::mem::take(&mut ctx.read_paths_full),
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
            if fatal {
                break;
            }
        }
        return out;
    }
    let started = std::time::Instant::now();
    // Sequential bookkeeping, concurrent execution: audit and transaction
    // records keep call order while the calls themselves overlap. Each call
    // runs on a nested context so its read state stays isolated.
    let mut prepared: Vec<Option<Vec<metteur_shared::Value>>> = Vec::with_capacity(calls.len());
    for call in calls {
        if rejected_reason(ctx, call, opts.allowed_tools.as_ref()).is_some() {
            prepared.push(None);
            continue;
        }
        let args_value = deanonymize_arguments(anonymizer, &call.arguments).await;
        let args = arguments_to_values(&args_value);
        ctx.transaction_log.record_tool_call(call.name.clone(), args.clone());
        ctx.audit("tool.call", serde_json::json!({ "name": call.name, "args": args_value }));
        prepared.push(Some(args));
    }

    let mut handles = Vec::with_capacity(calls.len());
    for (call, args) in calls.iter().zip(prepared) {
        let Some(args) = args else {
            handles.push(None);
            continue;
        };
        let Some(tool) = ctx.registry.tool(&call.name) else {
            handles.push(None);
            continue;
        };
        let mut child = ctx.child_nested();
        handles.push(Some(tokio::spawn(async move {
            let outcome = match tool.timeout() {
                Some(limit) => {
                    match tokio::time::timeout(limit, tool.call(&args, &mut child)).await {
                        Ok(result) => result,
                        Err(_) => {
                            Err(DaemonError::Execution(format!("tool '{}' timed out", tool.name())))
                        }
                    }
                }
                None => tool.call(&args, &mut child).await,
            };
            let text = outcome.map(|value| crate::execution::nodes::value_to_string(&value));
            (text, child.read_paths, child.read_paths_full)
        })));
    }

    let mut out = Vec::with_capacity(calls.len());
    for (call, handle) in calls.iter().zip(handles) {
        // The batch ran concurrently, so its wall-clock time is what each of
        // its calls took.
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let entry = match handle {
            Some(handle) => match handle.await {
                Ok((result, read_paths, full_reads)) => ToolOutcome {
                    result,
                    read_paths,
                    full_reads,
                    elapsed_ms,
                },
                Err(join_error) => ToolOutcome {
                    result: Err(DaemonError::Execution(format!(
                        "tool '{}' task failed: {join_error}",
                        call.name
                    ))),
                    read_paths: Vec::new(),
                    full_reads: Vec::new(),
                    elapsed_ms,
                },
            },
            None => ToolOutcome {
                result: rejected_result(ctx, call, opts.allowed_tools.as_ref()),
                read_paths: Vec::new(),
                full_reads: Vec::new(),
                elapsed_ms,
            },
        };
        out.push(entry);
    }
    out
}

/// Runs one tool call while forwarding its progress reports.
///
/// The call and the progress receiver are polled together, so a tool that
/// reports output every second keeps the UI alive without the tool knowing
/// anything about events: it writes to [`ExecutionContext::progress`] and this
/// function decides what reaches the caller.
async fn run_with_progress(
    ctx: &mut ExecutionContext,
    call: &ToolCall,
    opts: &ReactOptions,
    anonymizer: &Anonymizer,
    on_event: &mut Option<&mut (dyn FnMut(ReactEvent) + Send)>,
) -> DaemonResult<String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    ctx.progress = Some(tx);
    let mut forward = |tail: String| {
        if let Some(cb) = on_event.as_deref_mut() {
            cb(ReactEvent::ToolProgress {
                call_id: call.id.clone(),
                tail,
            });
        }
    };
    let result = {
        let running = invoke_tool(ctx, call, opts.allowed_tools.as_ref(), anonymizer);
        tokio::pin!(running);
        loop {
            tokio::select! {
                outcome = &mut running => break outcome,
                Some(tail) = rx.recv() => forward(tail),
            }
        }
    };
    // The pinned future is gone, so the context may be touched again.
    ctx.progress = None;
    while let Ok(tail) = rx.try_recv() {
        forward(tail);
    }
    result
}

/// Drains the paths read by tool calls since the last drain.
fn drain_read_paths(ctx: &mut ExecutionContext) -> Vec<std::path::PathBuf> {
    std::mem::take(&mut ctx.read_paths)
}

/// Whether a tool may run concurrently with its siblings.
fn is_read_only_tool(ctx: &ExecutionContext, name: &str) -> bool {
    ctx.registry.tool(name).map(|tool| tool.read_only()).unwrap_or(false)
}

/// Returns the refusal text for a call that must not run, if any.
fn rejected_reason(
    ctx: &ExecutionContext,
    call: &ToolCall,
    allowed: Option<&HashSet<String>>,
) -> Option<&'static str> {
    if let Some(set) = allowed
        && !set.contains(&call.name)
    {
        return Some("not allowed in this context");
    }
    if ctx.registry.tool(&call.name).is_none() {
        return Some("not available");
    }
    None
}

/// Builds the refusal result for a rejected call.
fn rejected_result(
    ctx: &ExecutionContext,
    call: &ToolCall,
    allowed: Option<&HashSet<String>>,
) -> DaemonResult<String> {
    Ok(format!(
        "Error: tool '{}' is {}.",
        call.name,
        rejected_reason(ctx, call, allowed).unwrap_or("unavailable")
    ))
}

/// Invokes a tool call and returns the textual result.
///
/// Calls outside `allowed` or unknown to the registry produce an error
/// message as the result so the model can recover. Arguments are
/// deanonymized before execution so tools see real values.
async fn invoke_tool(
    ctx: &mut ExecutionContext,
    call: &ToolCall,
    allowed: Option<&HashSet<String>>,
    anonymizer: &Anonymizer,
) -> DaemonResult<String> {
    if rejected_reason(ctx, call, allowed).is_some() {
        return rejected_result(ctx, call, allowed);
    }
    ctx.transaction_log.ensure_healthy()?;
    let Some(tool) = ctx.registry.tool(&call.name) else {
        return rejected_result(ctx, call, allowed);
    };
    let args_value = deanonymize_arguments(anonymizer, &call.arguments).await;
    let args = arguments_to_values(&args_value);
    ctx.transaction_log.record_tool_call(call.name.clone(), args.clone());
    ctx.audit("tool.call", serde_json::json!({ "name": call.name, "args": args_value }));
    let result = match tool.timeout() {
        Some(limit) => match tokio::time::timeout(limit, tool.call(&args, ctx)).await {
            Ok(result) => result,
            Err(_) => {
                return Err(DaemonError::Execution(format!("tool '{}' timed out", call.name)));
            }
        },
        None => tool.call(&args, ctx).await,
    }?;
    Ok(crate::execution::nodes::value_to_string(&result))
}

/// Restores real secrets inside a tool call's JSON arguments.
async fn deanonymize_arguments(
    anonymizer: &Anonymizer,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    let raw = anonymizer.deanonymize(&arguments.to_string()).await;
    serde_json::from_str(&raw).unwrap_or_else(|_| arguments.clone())
}

/// Converts a tool call's JSON arguments into a value list.
///
/// Object arguments are passed as a single JSON value so tools can resolve
/// arguments by name; array arguments are passed positionally.
fn arguments_to_values(args: &serde_json::Value) -> Vec<metteur_shared::Value> {
    match args {
        serde_json::Value::Array(items) => {
            items.iter().map(crate::execution::nodes::json_to_value).collect()
        }
        serde_json::Value::Object(_) => vec![metteur_shared::Value::Json(args.clone())],
        other => vec![crate::execution::nodes::json_to_value(other)],
    }
}

/// Retains an unmetered outcome so later successes cannot hide a coverage gap.
fn record_unavailable_usage(ctx: &ExecutionContext, client: &dyn LlmClient) {
    ctx.audit("llm.usage", serde_json::json!({
        "run_id": ctx.run_id.to_string(), "provider": client.provider(), "model": client.model(),
        "tokens_reported": false, "cache_read_reported": false,
    }));
}

/// Audits one completion's usage, extended with run id and estimated cost.
async fn record_usage(
    ctx: &ExecutionContext,
    client: &dyn LlmClient,
    usage: &Usage,
    billing: &Option<BillingCtx>,
) {
    let model = client.model().to_string();
    let mut detail = serde_json::json!({
        "accounting_version": 1,
        "tokens_reported": usage.tokens_reported,
        "cache_read_reported": usage.cache_read_reported,
        "provider": client.provider(),
        "model": model,
        "run_id": ctx.run_id.to_string(),
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "total_tokens": usage.total_tokens,
        "cached_input_tokens": usage.cached_input_tokens,
        "cache_write_input_tokens": usage.cache_write_input_tokens,
    });
    // The hit rate is what makes cache effectiveness observable in the audit
    // UI; it is absent when the provider reports no input at all.
    if let Some(rate) = usage.cache_hit_rate() {
        detail["cache_hit_rate"] = serde_json::json!(rate);
    }
    if let Some((currency, timezone, models)) = billing.as_ref()
        && let Some(model_cfg) = models.get(client.model())
        && let Some(cost) = crate::llm::billing::cost(
            crate::llm::billing::effective_pricing_at(model_cfg, usage.input_tokens, timezone, chrono::Utc::now())
                .as_ref(),
            currency,
            usage,
        )
    {
        detail["cost_micros"] = cost.micros.into();
        detail["currency"] = cost.currency.into();
    }
    ctx.audit("llm.usage", detail);
    if let Some(metrics) = &ctx.metrics {
        use std::sync::atomic::Ordering;
        metrics.llm_calls_total.fetch_add(1, Ordering::Relaxed);
        metrics.llm_input_tokens_total.fetch_add(usage.input_tokens, Ordering::Relaxed);
        metrics
            .llm_output_tokens_total
            .fetch_add(usage.output_tokens, Ordering::Relaxed);
    }
}

/// Compresses the context when it needs it.
///
/// Two triggers coexist: an explicit message-count threshold set on the node
/// (kept for backwards compatibility) and the workspace token budget, which
/// applies whenever the configured model declares a context window. The token
/// trigger is the one that actually protects a long conversation from
/// overflowing the provider's limit.
///
/// Compression failures are logged and skipped; they never abort the run.
async fn compress_if_needed(
    ctx: &mut ExecutionContext,
    client: &Arc<dyn LlmClient>,
    context: &mut ContextManager,
    params: &GenerationParams,
    opts: &ReactOptions,
    billing: &Option<BillingCtx>,
    usage: &mut Usage,
) {
    let by_messages = opts.compress_after_messages.filter(|keep| *keep > 0);
    let by_tokens = window_pressure(ctx, context, opts).await;
    let triggered_by_messages = matches!(by_messages, Some(keep) if context.messages.len() > keep);
    // Window pressure is relieved deterministically first: releasing tool
    // results costs no model call and loses no meaning, while summarizing
    // spends a completion and can drop details. An explicit node-level message
    // threshold still goes straight to compression — that is a deliberate
    // instruction, not pressure.
    if !triggered_by_messages
        && by_tokens.is_some()
        && auto_release(ctx, context).await
        && window_pressure(ctx, context, opts).await.is_none()
    {
        return;
    }
    let keep_recent = match (by_messages, by_tokens) {
        // A node-level setting wins when both trigger.
        (Some(keep), _) if triggered_by_messages => keep,
        (_, Some(keep)) => keep,
        _ => return,
    };
    if keep_recent == 0 || context.messages.len() <= keep_recent {
        return;
    }
    let outcome =
        summarize_and_compress(ctx, client, context, params, opts, billing, usage, keep_recent)
            .await;
    if let Err(err) = outcome {
        tracing::warn!("[{}] skipping context compression: {err}", opts.label);
    }
}

/// Releases the oldest tool results when the automatic release is enabled.
///
/// Returns whether anything was released. The model is told through a notice
/// so it does not keep referring to content that is gone.
async fn auto_release(ctx: &mut ExecutionContext, context: &mut ContextManager) -> bool {
    let keep = match &ctx.config {
        Some(config) => {
            let config = config.read().await;
            if !config.llm.auto_release {
                return false;
            }
            config.llm.auto_release_keep_results as usize
        }
        None => return false,
    };
    if context.tool_results.len() <= keep {
        return false;
    }
    let query = metteur_shared::llm::ReleaseQuery {
        all: true,
        keep_recent: keep,
        ..Default::default()
    };
    let report = context.release(&query);
    if report.released == 0 {
        return false;
    }
    ctx.audit(
        "context.auto_release",
        serde_json::json!({
            "released": report.released,
            "freed_tokens": report.estimated_tokens,
            "by_tool": report.by_tool,
        }),
    );
    context.push_message(Message::text(Role::User, report.notice()));
    true
}

/// Returns the tail size to keep when the context crowds the model window.
///
/// `None` means either the model has no declared window (nothing to guard
/// against) or the context still fits comfortably.
async fn window_pressure(
    ctx: &ExecutionContext,
    context: &ContextManager,
    opts: &ReactOptions,
) -> Option<usize> {
    let config = ctx.config.as_ref()?;
    let config = config.read().await;
    if config.llm.compress_at_ratio <= 0.0 {
        return None;
    }
    let model_key = opts
        .model
        .clone()
        .filter(|key| !key.is_empty())
        .or_else(|| config.llm.default_model.clone())?;
    let model = config.llm.models.get(&model_key)?;
    let budget = crate::llm::context_window_budget(Some(model), opts.max_tokens.map(u64::from))?;
    let estimated = metteur_shared::llm::estimate_context_tokens(context);
    let threshold = (budget as f64 * config.llm.compress_at_ratio) as u64;
    if estimated <= threshold {
        return None;
    }
    // Keep as many trailing messages as the token budget allows, at least one.
    let keep_budget = config.llm.compress_keep_tokens.max(1);
    let mut kept = 0usize;
    let mut tokens = 0u64;
    for message in context.messages.iter().rev() {
        let cost = metteur_shared::llm::estimate_message(message);
        if kept > 0 && tokens + cost > keep_budget {
            break;
        }
        tokens += cost;
        kept += 1;
    }
    Some(kept.max(1))
}

/// Token budget for the transcript handed to the summarizer.
///
/// The transcript is the material that just overflowed the main model's window.
/// Concatenating all of it into one user message would overflow the summarizer
/// too, turning the recovery path into a second failure — so it is capped, and
/// the newest messages win because a summary is mostly about recent work.
const MAX_TRANSCRIPT_TOKENS: u64 = 24_000;

/// Renders the messages to summarize, keeping the newest within a token budget.
///
/// Oldest messages are dropped whole rather than truncated mid-message: a
/// partial record is worth less to the summarizer than a complete one, and the
/// boundary between kept and dropped messages is already arbitrary.
fn render_transcript(older: &[Message]) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(older.len());
    let mut budget = MAX_TRANSCRIPT_TOKENS;
    let mut dropped = 0usize;
    for message in older.iter().rev() {
        let line = format!("{:?}: {}\n", message.role, message.text_content());
        let cost = metteur_shared::llm::estimate_tokens(&line);
        if cost > budget {
            dropped += 1;
            continue;
        }
        budget -= cost;
        lines.push(line);
    }
    lines.reverse();
    let mut transcript = lines.concat();
    if dropped > 0 {
        // Stated in the payload so the summary can acknowledge the gap instead
        // of presenting a truncated record as complete.
        transcript = format!("[{dropped} older message(s) omitted for length]\n{transcript}");
    }
    transcript
}

/// Produces an LLM summary of the older messages and merges it in.
#[allow(clippy::too_many_arguments)]
async fn summarize_and_compress(
    ctx: &mut ExecutionContext,
    client: &Arc<dyn LlmClient>,
    context: &mut ContextManager,
    params: &GenerationParams,
    opts: &ReactOptions,
    billing: &Option<BillingCtx>,
    usage: &mut Usage,
    keep_recent: usize,
) -> DaemonResult<()> {
    let tokens_before = metteur_shared::llm::estimate_context_tokens(context);
    let split_at = context.pair_safe_split(keep_recent);
    let older: Vec<Message> = context.messages[..split_at].to_vec();
    if older.is_empty() {
        return Ok(());
    }
    let transcript = render_transcript(&older);
    let summarizer_context = ContextManager::new_from_prompt(
        vec![crate::harness::HarnessPrompt::compress_fragment()],
        format!("Summarize the following conversation:\n\n{transcript}"),
    );
    // A configured summarizer model keeps the expensive model out of routine
    // housekeeping; failing to build it falls back to the active client.
    let summarizer = summarizer_client(ctx, opts).await.unwrap_or_else(|| Arc::clone(client));
    ctx.audit("llm.request", serde_json::json!({"run_id": ctx.run_id.to_string(), "model": summarizer.model()}));
    let response = summarizer
        .complete(&summarizer_context, params, &[])
        .await
        .map_err(|e| { record_unavailable_usage(ctx, summarizer.as_ref()); DaemonError::Llm(e.to_string()) })?;
    record_usage(ctx, summarizer.as_ref(), &response.usage, billing).await;
    usage.add(&response.usage);
    if response.text.trim().is_empty() {
        return Err(DaemonError::Llm("empty compression summary".to_string()));
    }
    let messages_before = context.messages.len();
    let summary = response.text;
    // `split_at` was computed on the pairing-safe boundary, so the summary
    // absorbs exactly what was summarized.
    context.compress_to(split_at, |_| Some(summary));
    ctx.audit(
        "llm.compress",
        serde_json::json!({
            "label": opts.label,
            "messages_before": messages_before,
            "messages_after": context.messages.len(),
            "tokens_before": tokens_before,
            "tokens_after": metteur_shared::llm::estimate_context_tokens(context),
        }),
    );
    Ok(())
}

/// Builds the summarizer client when `[llm].compress_model` names another
/// model; `None` means the active client summarizes.
async fn summarizer_client(
    ctx: &ExecutionContext,
    opts: &ReactOptions,
) -> Option<Arc<dyn LlmClient>> {
    let config = ctx.config.as_ref()?;
    let defaults = config.read().await.llm.clone();
    let model = defaults.compress_model.clone().filter(|name| !name.is_empty())?;
    if Some(&model) == opts.model.as_ref() {
        return None;
    }
    let mut summarizer_opts = opts.clone();
    summarizer_opts.model = Some(model);
    build_client(ctx, &summarizer_opts, &defaults).ok()
}

/// Returns the current time in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The transcript handed to the summarizer must fit its budget even when
    /// the source context is far larger: an unbounded transcript would make the
    /// compression path overflow the summarizer exactly when the main model
    /// already has.
    #[test]
    fn transcript_is_bounded_and_keeps_the_newest() {
        let messages: Vec<Message> = (0..400)
            .map(|i| Message::text(Role::User, format!("message {i} {}", "x".repeat(400))))
            .collect();
        let transcript = render_transcript(&messages);
        let cost = metteur_shared::llm::estimate_tokens(&transcript);
        assert!(
            cost <= MAX_TRANSCRIPT_TOKENS * 2,
            "transcript of {cost} tokens blew past its budget"
        );
        // The newest message is the one a summary most needs; the oldest is the
        // first thing worth dropping.
        let newest = format!("message {}", messages.len() - 1);
        let oldest = "message 0";
        assert!(transcript.contains(&newest), "newest message was dropped");
        assert!(!transcript.contains(oldest), "oldest message should be the one omitted");
        assert!(transcript.contains("omitted for length"));
    }

    fn new_ctx() -> ExecutionContext {
        ExecutionContext::new(
            std::sync::Arc::new(crate::registry::Registry::with_builtins()),
            crate::llm::LlmClientFactory::new(),
            std::env::temp_dir(),
        )
    }

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call_1".to_string(),
            name: name.to_string(),
            arguments,
        }
    }

    #[tokio::test]
    async fn forbidden_tool_yields_error_message() {
        let mut ctx = new_ctx();
        let allowed = HashSet::from(["ReadFile".to_string()]);
        let anonymizer = Anonymizer::disabled();
        let result = invoke_tool(
            &mut ctx,
            &call("WriteFile", serde_json::json!({ "path": "x.txt" })),
            Some(&allowed),
            &anonymizer,
        )
        .await
        .unwrap();
        assert!(result.contains("not allowed"));
    }

    #[tokio::test]
    async fn unknown_tool_yields_error_message() {
        let mut ctx = new_ctx();
        let anonymizer = Anonymizer::disabled();
        let result =
            invoke_tool(&mut ctx, &call("NoSuchTool", serde_json::json!({})), None, &anonymizer)
                .await
                .unwrap();
        assert!(result.contains("not available"));
    }

    #[tokio::test]
    async fn mock_options_build_mock_client() {
        let ctx = new_ctx();
        let opts = ReactOptions {
            provider: "mock".to_string(),
            mock_text: Some("scripted".to_string()),
            mock_delay_ms: Some(0),
            ..Default::default()
        };
        let client = build_client(&ctx, &opts, &LlmConfig::default()).unwrap();
        assert_eq!(client.provider(), "mock");
    }

    #[tokio::test]
    async fn unknown_provider_is_rejected() {
        let ctx = new_ctx();
        let opts = ReactOptions {
            provider: "nope".to_string(),
            ..Default::default()
        };
        let err = match build_client(&ctx, &opts, &LlmConfig::default()) {
            Err(err) => err,
            Ok(_) => panic!("expected unknown provider to be rejected"),
        };
        assert!(err.to_string().contains("unknown llm provider"));
    }

    #[tokio::test]
    async fn configured_model_entry_drives_the_client() {
        use metteur_shared::config::LlmModelConfig;
        let ctx = new_ctx();
        let defaults = LlmConfig {
            default_model: Some("deepseek".to_string()),
            models: std::collections::HashMap::from([(
                "deepseek".to_string(),
                LlmModelConfig {
                    api_type: "anthropic".to_string(),
                    api_endpoint: "https://example.test/v1".to_string(),
                    model_id: "deepseek-chat".to_string(),
                    api_key: "sk-test".to_string(),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        // The caller only names the model; provider, id and endpoint come from
        // the configured entry.
        let opts = ReactOptions {
            model: Some("deepseek".to_string()),
            ..Default::default()
        };
        let client = build_client(&ctx, &opts, &defaults).unwrap();
        assert_eq!(client.provider(), "anthropic");
        assert_eq!(client.model(), "deepseek-chat");

        // An unconfigured model with no default is an actionable error.
        let err = build_client(&ctx, &ReactOptions::default(), &LlmConfig::default())
            .err()
            .expect("missing model must be rejected");
        assert!(err.to_string().contains("no model configured"));
    }

    #[tokio::test]
    async fn tool_definitions_respect_allow_list() {
        let registry = crate::registry::Registry::with_builtins();
        let all = tool_definitions(&registry, None);
        let restricted =
            tool_definitions(&registry, Some(&HashSet::from(["ReadFile".to_string()])));
        let none = tool_definitions(&registry, Some(&HashSet::new()));
        assert!(all.len() > 1);
        assert_eq!(restricted.len(), 1);
        assert_eq!(restricted[0].name, "ReadFile");
        assert!(none.is_empty());
    }

    /// The tool array is part of the cached request prefix, so its order must
    /// not depend on the registry's internal (hash) ordering.
    #[tokio::test]
    async fn tool_definitions_are_canonically_ordered() {
        let first = tool_definitions(&crate::registry::Registry::with_builtins(), None);
        let second = tool_definitions(&crate::registry::Registry::with_builtins(), None);
        let names: Vec<&str> = first.iter().map(|t| t.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "tools must be sorted by name");
        assert_eq!(names, second.iter().map(|t| t.name.as_str()).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn streaming_forwards_deltas_then_final_assistant() {
        let mut ctx = new_ctx();
        let context = ContextManager::new_from_prompt(vec![], "hi");
        let opts = ReactOptions {
            provider: "mock".to_string(),
            mock_text: Some("streamed".to_string()),
            ..Default::default()
        };
        let mut deltas = Vec::new();
        let mut events = Vec::new();
        let mut on_delta = |delta: StreamDelta| deltas.push(delta);
        let mut on_event = |ev: ReactEvent| events.push(ev);
        let outcome =
            run_react_streaming(&mut ctx, context, &opts, Some(&mut on_delta), &mut on_event)
                .await
                .unwrap();
        // The mock provider delivers the whole text as a single delta.
        assert_eq!(deltas, vec![StreamDelta::Text("streamed".to_string())]);
        assert!(matches!(&events[0], ReactEvent::Assistant { text, .. } if text == "streamed"));
        assert_eq!(outcome.text, "streamed");
        // The loop keeps the initial user message; the final text is appended
        // by the caller when persisting.
        assert_eq!(outcome.context.messages.len(), 1);
    }

    #[tokio::test]
    async fn cancellation_returns_partial_context() {
        let mut ctx = new_ctx();
        let context = ContextManager::new_from_prompt(vec![], "question");
        ctx.cancel_requested.store(true, std::sync::atomic::Ordering::SeqCst);
        let opts = ReactOptions {
            provider: "mock".to_string(),
            ..Default::default()
        };
        let result = run_react_streaming(&mut ctx, context, &opts, None, &mut |_| {}).await;
        let Err((err, partial)) = result else {
            panic!("expected cancellation error");
        };
        assert!(err.to_string().contains("cancelled"));
        // The pre-run context survived the interrupted loop.
        assert_eq!(partial.messages.len(), 1);
        assert_eq!(partial.messages[0].text_content(), "question");
    }
}
