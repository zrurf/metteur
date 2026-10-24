//! The `ExecuteCommand` tool running shell commands through the sandbox.

use async_trait::async_trait;
use metteur_shared::Value;

use crate::error::{DaemonError, DaemonResult};
use crate::execution::context::ExecutionContext;
use crate::execution::jobs::{JobSnapshot, JobState, wait_for_cancel};

use super::Args;
use crate::registry::tool::Tool;

/// Default seconds before a blocking command is killed (`0` = no limit).
const DEFAULT_TIMEOUT_CONFIG: u64 = 0;

/// Default retained output per job when the configuration is absent.
const DEFAULT_OUTPUT_CAP: usize = 256 * 1024;

/// Default number of trailing output lines shown to the model.
const DEFAULT_TAIL_LINES: usize = 80;

/// Maximum compressed output size returned to the caller.
const OUTPUT_LIMIT_BYTES: usize = 8192;

/// How often a running command's trailing output is reported to clients.
const PROGRESS_INTERVAL_MS: u64 = 1000;

/// Output lines included in one progress report.
const PROGRESS_TAIL_LINES: usize = 12;

/// Runs a shell command and waits for it to finish.
///
/// The command runs as a job (see [`crate::execution::jobs`]) even though the
/// tool blocks, so it can be terminated by a cancel and its output stays
/// inspectable from the job list. There is no timeout by default: the engine
/// wakes the model when the command ends, which is what makes sleeping and
/// polling unnecessary.
pub struct ExecuteCommand;

#[async_trait]
impl Tool for ExecuteCommand {
    fn name(&self) -> &str {
        "ExecuteCommand"
    }

    fn description(&self) -> &str {
        "Runs a shell command inside the workspace and returns its exit code and \
         output once it finishes. There is no default timeout: long builds and \
         test runs are expected. The command can be aborted by cancelling the \
         run. Risky commands require user approval and may be denied. For a \
         command that should keep running while you continue working, use \
         StartCommand and then WaitJob."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command line to run." },
                "cwd": { "type": "string", "description": "Working directory relative to the workspace root." },
                "timeout_secs": { "type": "integer", "description": "Kill and report a timeout after this many seconds (default: no limit)." }
            },
            "required": ["command"]
        })
    }

    fn max_result_bytes(&self) -> usize {
        OUTPUT_LIMIT_BYTES
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let a = Args::new(args);
        let command = a.string("command", 0).ok_or_else(|| {
            DaemonError::Execution("ExecuteCommand requires a command".to_string())
        })?;
        let cwd_arg = a.string("cwd", 1);
        let explicit_timeout =
            a.int("timeout_secs", 2).filter(|value| *value > 0).map(|v| v as u64);

        let settings = JobSettings::from_config(ctx).await;
        let cwd = ctx.jobs.resolve_cwd(cwd_arg.as_deref())?;
        let job_id = start_command(ctx, &command, &cwd, settings.output_cap).await?;
        let timeout = explicit_timeout.unwrap_or(settings.timeout_secs);

        let snapshot = wait_for_job(ctx, &job_id, timeout).await?;
        let tail = ctx.jobs.tail(&job_id, settings.tail_lines);
        ctx.jobs.mark_result_delivered(&job_id);
        Ok(Value::Json(result_payload(&snapshot, &tail)))
    }
}

/// Starts a command as a job after sandbox authorization.
async fn start_command(
    ctx: &ExecutionContext,
    command: &str,
    cwd: &std::path::Path,
    output_cap: usize,
) -> DaemonResult<String> {
    if !crate::sandbox::authorize(ctx, command).await? {
        return Err(DaemonError::Sandbox(format!("command denied by sandbox: {command}")));
    }
    ctx.audit(
        "sandbox.execute",
        serde_json::json!({
            "command": command,
            "cwd": cwd.to_string_lossy(),
        }),
    );
    let handle = ctx.continuation.clone();
    let mut registered = None;
    let started = ctx.jobs.start_registered(command, cwd, ctx.run_id, output_cap, |id| {
        if let Some(handle) = &handle {
            if ctx.handoff_ref.is_some() && ctx.blueprint.is_some() {
                handle.register_run_job(id, ctx.run_id)?;
            } else {
                handle.register(id, "job", true)?;
            }
            registered = Some(id.to_owned());
        }
        Ok(())
    });
    let job_id = match started {
        Ok(id) => id,
        Err(error) => {
            if let (Some(handle), Some(id)) = (&handle, registered) {
                handle.finish(
                    &id,
                    crate::continuation::state::Terminal::Failed,
                    serde_json::json!({"started":false,"reason":"process_spawn_failed"}),
                    chrono::Utc::now().timestamp_millis().max(0) as u64,
                )?;
            }
            return Err(error);
        }
    };
    if let Some(handle) = handle {
        let jobs = ctx.jobs.clone();
        let id = job_id.clone();
        ctx.jobs.track_receipt(async move {
            if let Some(job) = jobs.wait(&id).await {
                use crate::continuation::state::Terminal;
                let status = match &job.state {
                    JobState::Exited { code: 0 } => Terminal::Succeeded,
                    JobState::Exited { .. } | JobState::Failed { .. } => Terminal::Failed,
                    JobState::Killed => Terminal::Cancelled,
                    JobState::Running => Terminal::Indeterminate,
                };
                // Only bounded output; source data never becomes a permission grant.
                let raw = crate::execution::blackboard::safe_value(&serde_json::json!({"output": jobs.tail(&id, 40), "state": job.state.label()}), 0);
                let safe = crate::observability::anon::Anonymizer::new(&[]).anonymize(&raw.to_string()).await;
                let data = handle.store.result_artifact(handle.id, &id, &safe).unwrap_or_else(|error|serde_json::json!({"summary": crate::registry::tools::result::truncate_result(&safe, 8192), "artifact_error":error.to_string(),"truncated":safe.len()>8192}));
                if let Err(error) = handle.finish(&id, status, data, chrono::Utc::now().timestamp_millis().max(0) as u64) {
                    tracing::warn!("Could not persist job terminal receipt: {error}");
                }
            }
        });
    }
    if let Some(tx) = &ctx.events {
        let summary = ctx.jobs.snapshot(&job_id).map(|job| job.summary()).unwrap_or_default();
        let _ = tx.send(crate::execution::ExecutionEvent::Job {
            node_id: ctx.current_node,
            job_id: job_id.clone(),
            state: "started".to_string(),
            summary,
        });
    }
    Ok(job_id)
}

/// Waits for a job to finish, honoring cancellation and an optional timeout.
///
/// A timeout kills the process: leaving it running would leak a build the
/// caller has already given up on.
///
/// While waiting, the job's trailing output is reported to the caller's
/// progress sink: a build can run for minutes, and the client shows its tail
/// instead of a silent spinner. Reports are sent only when the tail changed.
async fn wait_for_job(
    ctx: &ExecutionContext,
    job_id: &str,
    timeout_secs: u64,
) -> DaemonResult<JobSnapshot> {
    let jobs = std::sync::Arc::clone(&ctx.jobs);
    let cancel = std::sync::Arc::clone(&ctx.cancel_requested);
    let waiting = async {
        match timeout_secs {
            0 => jobs.wait(job_id).await,
            secs => {
                match tokio::time::timeout(std::time::Duration::from_secs(secs), jobs.wait(job_id))
                    .await
                {
                    Ok(snapshot) => snapshot,
                    Err(_) => {
                        jobs.kill(job_id);
                        None
                    }
                }
            }
        }
    };
    let progress = report_progress(ctx, job_id);
    tokio::pin!(progress);
    let outcome = tokio::select! {
        snapshot = waiting => snapshot,
        _ = wait_for_cancel(cancel) => {
            ctx.jobs.kill(job_id);
            return Err(DaemonError::Interrupted(format!("command {job_id} cancelled")));
        }
        _ = &mut progress => None,
    };
    outcome.ok_or_else(|| {
        if timeout_secs == 0 {
            DaemonError::Execution(format!("command job {job_id} not found"))
        } else {
            DaemonError::Execution(format!(
                "command timed out after {timeout_secs}s and was terminated: {job_id}"
            ))
        }
    })
}

/// Reports the running job's tail until the sink closes or the wait ends.
///
/// Returns when the client side of the sink is gone, so a dropped connection
/// stops the reporting instead of spinning.
async fn report_progress(ctx: &ExecutionContext, job_id: &str) {
    let Some(sink) = ctx.progress.clone() else {
        // Without a sink (blueprint runs, non-streaming callers) the future
        // never resolves; the select! above simply ignores it.
        std::future::pending::<()>().await;
        return;
    };
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(PROGRESS_INTERVAL_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = String::new();
    loop {
        ticker.tick().await;
        let tail = ctx.jobs.tail(job_id, PROGRESS_TAIL_LINES);
        if tail.is_empty() || tail == last {
            continue;
        }
        if sink.send(tail.clone()).is_err() {
            return;
        }
        last = tail;
    }
}

/// Renders a job as the tool result payload.
fn result_payload(snapshot: &JobSnapshot, tail: &str) -> serde_json::Value {
    let output = crate::sandbox::compress_output(tail.trim(), OUTPUT_LIMIT_BYTES);
    let exit_code = match &snapshot.state {
        JobState::Exited {
            code,
        } => *code,
        JobState::Killed
        | JobState::Failed {
            ..
        }
        | JobState::Running => -1,
    };
    serde_json::json!({
        "job_id": snapshot.id,
        "exit_code": exit_code,
        "state": snapshot.state.label(),
        "duration_ms": snapshot.duration_ms(),
        "output": output,
    })
}

/// The `[execution]` job settings a tool needs.
struct JobSettings {
    timeout_secs: u64,
    output_cap: usize,
    tail_lines: usize,
}

impl JobSettings {
    async fn from_config(ctx: &ExecutionContext) -> Self {
        let execution = match &ctx.config {
            Some(config) => config.read().await.execution.clone(),
            None => metteur_shared::config::ExecutionConfig::default(),
        };
        Self {
            timeout_secs: if execution.command_timeout_secs == 0 {
                DEFAULT_TIMEOUT_CONFIG
            } else {
                execution.command_timeout_secs
            },
            output_cap: if execution.job_output_max_bytes == 0 {
                DEFAULT_OUTPUT_CAP
            } else {
                execution.job_output_max_bytes as usize
            },
            tail_lines: if execution.job_tail_lines == 0 {
                DEFAULT_TAIL_LINES
            } else {
                execution.job_tail_lines as usize
            },
        }
    }
}

/// Starts a background job and returns its id immediately.
pub struct StartCommand;

#[async_trait]
impl Tool for StartCommand {
    fn name(&self) -> &str {
        "StartCommand"
    }

    fn description(&self) -> &str {
        "Starts a shell command in the background and returns its job id \
         immediately. Use it for commands that take a long time while you keep \
         working, then collect the result with WaitJob (or just finish your turn \
         — the engine wakes you when a job completes). The command can be \
         stopped with KillJob."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command line to run." },
                "cwd": { "type": "string", "description": "Working directory relative to the workspace root." }
            },
            "required": ["command"]
        })
    }

    fn max_result_bytes(&self) -> usize {
        2048
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let a = Args::new(args);
        let command = a
            .string("command", 0)
            .ok_or_else(|| DaemonError::Execution("StartCommand requires a command".to_string()))?;
        let settings = JobSettings::from_config(ctx).await;
        let cwd = ctx.jobs.resolve_cwd(a.string("cwd", 1).as_deref())?;
        let job_id = start_command(ctx, &command, &cwd, settings.output_cap).await?;
        // A JSON payload rather than prose, so a blueprint can wire the result
        // straight into `WaitJob`'s `job_id` pin; the note keeps it usable for
        // the model as well.
        Ok(Value::Json(serde_json::json!({
            "job_id": job_id,
            "state": "running",
            "command": command,
            "note": "It keeps running in the background. Use WaitJob to block on it, \
                     or finish your turn and you will be woken when it completes.",
        })))
    }
}

/// Reports the state of one job, or of every job of this run.
pub struct JobStatus;

#[async_trait]
impl Tool for JobStatus {
    fn name(&self) -> &str {
        "JobStatus"
    }

    fn description(&self) -> &str {
        "Reports the state of background commands without blocking: id, state \
         (running / exit code / killed), elapsed time, output size and command \
         line. Without a job id it reports every job started by this run."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "Job to inspect; omit to list this run's jobs." }
            }
        })
    }

    fn read_only(&self) -> bool {
        true
    }

    fn lifetime(&self) -> metteur_shared::ToolResultLifetime {
        metteur_shared::ToolResultLifetime::OneShot
    }

    fn max_result_bytes(&self) -> usize {
        4096
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let a = Args::new(args);
        match job_id_of_arg(&a) {
            Some(id) => {
                let snapshot = owned_job(ctx, &id)?;
                Ok(Value::String(snapshot.summary()))
            }
            None => {
                let jobs = ctx.jobs.list(Some(ctx.run_id));
                if jobs.is_empty() {
                    return Ok(Value::String("No background jobs.".to_string()));
                }
                let lines: Vec<String> = jobs.iter().map(JobSnapshot::summary).collect();
                Ok(Value::String(lines.join("\n")))
            }
        }
    }
}

/// Blocks until a job finishes (or a timeout elapses).
pub struct WaitJob;

#[async_trait]
impl Tool for WaitJob {
    fn name(&self) -> &str {
        "WaitJob"
    }

    fn description(&self) -> &str {
        "Waits for a background job to finish and returns its exit code and \
         output. Prefer calling it when you need the result before deciding the \
         next step; if you have nothing else to do, just end your turn and the \
         engine wakes you when a job completes."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "Job to wait for." },
                "timeout_secs": { "type": "integer", "description": "Give up after this many seconds (default: no limit). The job keeps running on timeout." }
            },
            "required": ["job_id"]
        })
    }

    fn max_result_bytes(&self) -> usize {
        OUTPUT_LIMIT_BYTES
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let a = Args::new(args);
        let job_id = job_id_of_arg(&a)
            .ok_or_else(|| DaemonError::Execution("WaitJob requires a job_id".to_string()))?;
        owned_job(ctx, &job_id)?;
        let settings = JobSettings::from_config(ctx).await;
        let timeout = a.int("timeout_secs", 1).filter(|value| *value > 0).map(|v| v as u64);

        let jobs = std::sync::Arc::clone(&ctx.jobs);
        let cancel = std::sync::Arc::clone(&ctx.cancel_requested);
        let waiting = async {
            match timeout {
                // A wait timeout returns the current state instead of killing
                // the job: the caller asked to wait, not to stop the work.
                Some(secs) => {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(secs),
                        jobs.wait(&job_id),
                    )
                    .await
                    {
                        Ok(snapshot) => snapshot,
                        Err(_) => jobs.snapshot(&job_id),
                    }
                }
                None => jobs.wait(&job_id).await,
            }
        };
        let snapshot = tokio::select! {
            snapshot = waiting => snapshot,
            _ = wait_for_cancel(cancel) => {
                return Err(DaemonError::Interrupted(format!("wait for job {job_id} cancelled")));
            }
        };
        let snapshot =
            snapshot.ok_or_else(|| DaemonError::Execution(format!("job {job_id} not found")))?;
        let tail = ctx.jobs.tail(&job_id, settings.tail_lines);
        let mut payload = result_payload(&snapshot, &tail);
        if snapshot.state.is_finished() {
            ctx.jobs.mark_result_delivered(&job_id);
        }
        if snapshot.state.is_running() {
            payload["note"] = serde_json::Value::String(
                "still running; wait again or end your turn to be woken on completion".to_string(),
            );
        }
        Ok(Value::Json(payload))
    }
}

/// Terminates one job, or every running job of this run.
pub struct KillJob;

#[async_trait]
impl Tool for KillJob {
    fn name(&self) -> &str {
        "KillJob"
    }

    fn description(&self) -> &str {
        "Terminates a background command started by this run. Without a job id it \
         terminates every job this run started. Jobs that already finished are \
         left alone."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string", "description": "Job to terminate; omit to kill this run's jobs." }
            }
        })
    }

    fn max_result_bytes(&self) -> usize {
        2048
    }

    async fn call(&self, args: &[Value], ctx: &mut ExecutionContext) -> DaemonResult<Value> {
        let a = Args::new(args);
        match job_id_of_arg(&a) {
            Some(id) => {
                owned_job(ctx, &id)?;
                let killed = ctx.jobs.kill(&id);
                Ok(Value::String(if killed {
                    format!("Kill requested for job {id}.")
                } else {
                    format!("Job {id} already finished; nothing to kill.")
                }))
            }
            None => {
                let killed = ctx.jobs.kill_owned_by(ctx.run_id);
                Ok(Value::String(format!("Kill requested for {killed} running job(s).")))
            }
        }
    }
}

/// Resolves a job id argument.
///
/// Accepts a bare id, a `StartCommand` payload object, or that payload rendered
/// as a JSON string — all three reach this tool in practice.
fn job_id_of_arg(a: &Args<'_>) -> Option<String> {
    let raw = a.string("job_id", 0)?;
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return Some(trimmed.to_string());
    }
    serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|value| value.get("job_id").and_then(|id| id.as_str()).map(str::to_string))
        .or_else(|| Some(trimmed.to_string()))
}

/// Resolves a job id and refuses ids that belong to another run.
///
/// A job id is a capability: without this check one workspace's agent could
/// wait on or kill a process started by an unrelated run.
fn owned_job(ctx: &ExecutionContext, id: &str) -> DaemonResult<JobSnapshot> {
    match ctx.jobs.snapshot(id) {
        Some(snapshot) if snapshot.owner == ctx.run_id => Ok(snapshot),
        Some(_) => Err(DaemonError::Execution(format!(
            "job {id} belongs to another run and cannot be controlled from here"
        ))),
        None => Err(DaemonError::Execution(format!(
            "job {id} not found (jobs do not survive a daemon restart)"
        ))),
    }
}
