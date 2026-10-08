//! Background commands (jobs) for one workspace.
//!
//! A job is a shell command that keeps running while the engine does something
//! else. Every command the agent runs goes through this manager, which owns its
//! output buffer, lifecycle state and kill switch, so the engine can report
//! progress, wait for completion, terminate the process and clean up when a run
//! ends.
//!
//! Jobs are processes, not engine state: they do not survive a daemon restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{Notify, oneshot};

use crate::error::{DaemonError, DaemonResult};

/// How often a running job flushes new output to subscribers.
const OUTPUT_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

/// Largest single output chunk sent to subscribers.
const OUTPUT_CHUNK_BYTES: usize = 8 * 1024;

/// How many notices a slow subscriber may fall behind before it is dropped
/// (it can rebuild the view with `list`).
const NOTICE_BUFFER: usize = 512;

/// What a job notice reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobNoticeKind {
    /// The process started.
    Started,
    /// New output arrived.
    Output,
    /// The process reached a terminal state.
    Finished,
}

impl JobNoticeKind {
    /// The wire label (`started` | `output` | `finished`).
    pub fn label(&self) -> &'static str {
        match self {
            JobNoticeKind::Started => "started",
            JobNoticeKind::Output => "output",
            JobNoticeKind::Finished => "finished",
        }
    }
}

/// A job lifecycle or output notice broadcast to subscribers.
#[derive(Debug, Clone)]
pub struct JobNotice {
    /// The job this notice belongs to.
    pub job_id: String,
    /// What happened.
    pub kind: JobNoticeKind,
    /// New output text (empty for lifecycle notices).
    pub chunk: String,
    /// The job's state at the time of the notice.
    pub snapshot: JobSnapshot,
}

/// How often a waiter re-checks state while waiting for a notification.
///
/// `Notify` alone cannot cover the race between a state check and the waiter
/// registering itself; the timeout bounds the worst case without busy-waiting.
const WAIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// How long the waiter waits for the output readers after the process exits.
///
/// Readers end when the pipes close, which is immediate for a normal exit. A
/// terminated process tree may leave a grandchild holding the pipes, so the
/// terminal state is published after this grace period regardless.
const READER_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Lifecycle state of a background command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    /// The process is still running.
    Running,
    /// The process exited with the given code (`-1` when the platform reports
    /// no code, e.g. after a signal).
    Exited {
        /// The process exit code.
        code: i32,
    },
    /// The process could not be started, or its status could not be read.
    Failed {
        /// Why the job failed.
        message: String,
    },
    /// The process was terminated on request.
    Killed,
}

impl JobState {
    /// Whether the job is still running.
    pub fn is_running(&self) -> bool {
        matches!(self, JobState::Running)
    }

    /// Whether the job ended, i.e. it is not running anymore.
    pub fn is_finished(&self) -> bool {
        !self.is_running()
    }

    /// A short label (`running`, `exited`, `failed`, `killed`).
    pub fn label(&self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Exited {
                ..
            } => "exited",
            JobState::Failed {
                ..
            } => "failed",
            JobState::Killed => "killed",
        }
    }
}

/// An immutable view of one job.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    /// The short job id used to address the job.
    pub id: String,
    /// The command line as the agent wrote it.
    pub command: String,
    /// Working directory, relative to the workspace root.
    pub cwd: String,
    /// Current lifecycle state.
    pub state: JobState,
    /// The run that started the job.
    pub owner: uuid::Uuid,
    /// Start time in milliseconds since the Unix epoch.
    pub started_at: u64,
    /// End time in milliseconds since the Unix epoch.
    pub finished_at: Option<u64>,
    /// Total bytes the job has produced (the buffer keeps only the tail).
    pub output_bytes: u64,
}

impl JobSnapshot {
    /// Wall-clock duration of the job in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.finished_at.unwrap_or_else(now_millis).saturating_sub(self.started_at)
    }

    /// Renders the state as `running`, `exit 3`, `killed` or `failed: …`.
    pub fn state_label(&self) -> String {
        match &self.state {
            JobState::Running => "running".to_string(),
            JobState::Exited {
                code,
            } => format!("exit {code}"),
            JobState::Killed => "killed".to_string(),
            JobState::Failed {
                message,
            } => format!("failed: {message}"),
        }
    }

    /// One-line summary: `<id> <state> <elapsed>s <bytes> bytes <command>`.
    pub fn summary(&self) -> String {
        format!(
            "{} | {} | {:.1}s | {} bytes | {}",
            self.id,
            self.state_label(),
            self.duration_ms() as f64 / 1000.0,
            self.output_bytes,
            self.command
        )
    }
}

/// A byte buffer keeping only the tail of a stream.
struct OutputBuffer {
    bytes: Vec<u8>,
    cap: usize,
    total: u64,
}

impl OutputBuffer {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap: cap.max(1),
            total: 0,
        }
    }

    /// Appends `chunk`, dropping the oldest bytes once the cap is exceeded.
    ///
    /// Trimming happens in blocks so a chatty command does not pay a memmove
    /// per chunk: the buffer may grow by a quarter of the cap before the front
    /// is dropped.
    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        self.bytes.extend_from_slice(chunk);
        let slack = self.cap / 4;
        if self.bytes.len() > self.cap + slack {
            let drop = self.bytes.len() - self.cap;
            self.bytes.drain(..drop);
        }
    }

    /// Renders the last `lines` lines.
    ///
    /// Trimming drops bytes from the front, so the first line may be partial
    /// and a UTF-8 sequence may be cut; both are decoded lossily.
    fn tail(&self, lines: usize) -> String {
        let text = String::from_utf8_lossy(&self.bytes);
        let mut out: Vec<&str> = text.lines().rev().take(lines.max(1)).collect();
        out.reverse();
        out.join("\n")
    }
}

/// One managed process.
struct JobEntry {
    snapshot: std::sync::Mutex<JobSnapshot>,
    output: std::sync::Mutex<OutputBuffer>,
    /// Output produced since the last notice, awaiting a flush.
    pending: std::sync::Mutex<Vec<u8>>,
    /// Consumed by the waiter task to terminate the process on request.
    kill: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl JobEntry {
    fn state(&self) -> JobState {
        self.snapshot.lock().unwrap().state.clone()
    }

    /// The job id.
    ///
    /// Separate from [`Self::view`] on purpose: taking an id inside a notice
    /// struct literal keeps the temporary guard alive until the end of the
    /// statement, and locking the same `std::sync::Mutex` twice deadlocks.
    fn id(&self) -> String {
        self.snapshot.lock().unwrap().id.clone()
    }

    fn view(&self) -> JobSnapshot {
        let snapshot = self.snapshot.lock().unwrap();
        JobSnapshot {
            output_bytes: self.output.lock().unwrap().total,
            ..snapshot.clone()
        }
    }

    /// Publishes the terminal state. Later calls are ignored so the first
    /// outcome wins.
    fn finish(&self, state: JobState, at: u64) {
        let mut snapshot = self.snapshot.lock().unwrap();
        if !snapshot.state.is_running() {
            return;
        }
        snapshot.state = state;
        snapshot.finished_at = Some(at);
    }

    fn tail(&self, lines: usize) -> String {
        self.output.lock().unwrap().tail(lines)
    }

    /// Records output for both the retained tail and the next notice.
    fn push_output(&self, chunk: &[u8]) {
        self.output.lock().unwrap().push(chunk);
        self.pending.lock().unwrap().extend_from_slice(chunk);
    }

    /// Takes the pending output, stopping at a UTF-8 boundary so a multi-byte
    /// character is never split between two notices.
    fn take_pending(&self) -> String {
        const MAX: usize = OUTPUT_CHUNK_BYTES;
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            return String::new();
        }
        let complete = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(err) => err.valid_up_to(),
        };
        let take = complete.min(MAX);
        if take == 0 {
            // An incomplete character is all we have; wait for its remainder.
            return String::new();
        }
        let drained: Vec<u8> = pending.drain(..take).collect();
        String::from_utf8_lossy(&drained).to_string()
    }
}

/// Background commands of one workspace.
pub struct JobManager {
    root: PathBuf,
    jobs: std::sync::Mutex<HashMap<String, Arc<JobEntry>>>,
    notify: Arc<Notify>,
    /// Lifecycle and output notices for subscribers (the web UI's job panel).
    notices: tokio::sync::broadcast::Sender<JobNotice>,
}

impl JobManager {
    /// Creates an empty manager for a workspace.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            jobs: std::sync::Mutex::new(HashMap::new()),
            notify: Arc::new(Notify::new()),
            notices: tokio::sync::broadcast::channel(NOTICE_BUFFER).0,
        }
    }

    /// Subscribes to job notices for this workspace.
    ///
    /// A subscriber can fall behind; it then receives `Lagged` and should
    /// rebuild its view with [`Self::list`], which is why notices are only
    /// ever additive information.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<JobNotice> {
        self.notices.subscribe()
    }

    /// Publishes a notice, ignoring the absence of subscribers.
    fn publish(&self, notice: JobNotice) {
        let _ = self.notices.send(notice);
    }

    /// The workspace root jobs are started under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a command's working directory inside the workspace.
    pub fn resolve_cwd(&self, requested: Option<&str>) -> DaemonResult<PathBuf> {
        let fs = crate::workspace::fs::WorkspaceFs::new(self.root.clone());
        match requested.filter(|dir| !dir.is_empty()) {
            Some(dir) => fs.resolve_existing(dir),
            None => Ok(self.root.clone()),
        }
    }

    /// Starts `command` in `cwd` and returns its job id.
    ///
    /// The caller is responsible for sandbox authorization; `output_cap`
    /// bounds the retained tail of the combined stdout/stderr stream.
    pub fn start(
        &self,
        command: &str,
        cwd: &Path,
        owner: uuid::Uuid,
        output_cap: usize,
    ) -> DaemonResult<String> {
        let mut cmd = build_shell_command(command);
        // A command that forks (shell scripts, build tools) must be killable as
        // a whole; on Unix the child leads its own process group for that.
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|err| DaemonError::Execution(format!("failed to spawn command: {err}")))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let id = new_job_id();
        let entry = Arc::new(JobEntry {
            snapshot: std::sync::Mutex::new(JobSnapshot {
                id: id.clone(),
                command: command.to_string(),
                cwd: self.display_cwd(cwd),
                state: JobState::Running,
                owner,
                started_at: now_millis(),
                finished_at: None,
                output_bytes: 0,
            }),
            output: std::sync::Mutex::new(OutputBuffer::new(output_cap)),
            pending: std::sync::Mutex::new(Vec::new()),
            kill: std::sync::Mutex::new(None),
        });

        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        *entry.kill.lock().unwrap() = Some(kill_tx);

        // Readers run until the pipes close, which happens when the process
        // exits; the waiter joins them before publishing the terminal state, so
        // a snapshot taken at exit always carries the complete output.
        let out_reader = spawn_reader(stdout, Arc::clone(&entry));
        let err_reader = spawn_reader(stderr, Arc::clone(&entry));

        let notify = Arc::clone(&self.notify);
        let notices = self.notices.clone();
        // A running job flushes whatever it produced; the waiter publishes the
        // terminal state and flushes the remainder, so nothing is lost.
        let flusher = Arc::clone(&entry);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(OUTPUT_FLUSH_INTERVAL).await;
                let chunk = flusher.take_pending();
                if !chunk.is_empty() {
                    let job_id = flusher.id();
                    let snapshot = flusher.view();
                    let _ = notices.send(JobNotice {
                        job_id,
                        kind: JobNoticeKind::Output,
                        chunk,
                        snapshot,
                    });
                }
                if flusher.state().is_finished() {
                    break;
                }
            }
        });

        let waiter = Arc::clone(&entry);
        let notices = self.notices.clone();
        tokio::spawn(async move {
            let mut killed = false;
            let status = tokio::select! {
                status = child.wait() => status,
                request = &mut kill_rx => {
                    // An `Err` means the sender was dropped (manager shutdown),
                    // not a kill request: fall through to a normal wait.
                    if request.is_ok() {
                        killed = true;
                        terminate(&mut child).await;
                    }
                    child.wait().await
                }
            };
            let readers = async {
                if let Some(reader) = out_reader {
                    let _ = reader.await;
                }
                if let Some(reader) = err_reader {
                    let _ = reader.await;
                }
            };
            // Normally the readers finish as soon as the process exits; the
            // timeout covers pipes still held by a surviving grandchild.
            let _ = tokio::time::timeout(READER_GRACE, readers).await;
            let state = if killed {
                JobState::Killed
            } else {
                match status {
                    Ok(status) => JobState::Exited {
                        code: status.code().unwrap_or(-1),
                    },
                    Err(err) => JobState::Failed {
                        message: err.to_string(),
                    },
                }
            };
            // Publish any output the flusher has not sent yet, then the state.
            let chunk = waiter.take_pending();
            if !chunk.is_empty() {
                let job_id = waiter.id();
                let snapshot = waiter.view();
                let _ = notices.send(JobNotice {
                    job_id,
                    kind: JobNoticeKind::Output,
                    chunk,
                    snapshot,
                });
            }
            waiter.finish(state, now_millis());
            let job_id = waiter.id();
            let snapshot = waiter.view();
            let _ = notices.send(JobNotice {
                job_id,
                kind: JobNoticeKind::Finished,
                chunk: String::new(),
                snapshot,
            });
            notify.notify_waiters();
        });

        self.jobs.lock().unwrap().insert(id.clone(), entry);
        let snapshot = self
            .snapshot(&id)
            .ok_or_else(|| DaemonError::Execution(format!("job {id} vanished at start")))?;
        self.publish(JobNotice {
            job_id: id.clone(),
            kind: JobNoticeKind::Started,
            chunk: String::new(),
            snapshot,
        });
        Ok(id)
    }

    /// Returns a snapshot of one job.
    pub fn snapshot(&self, id: &str) -> Option<JobSnapshot> {
        self.entry(id).map(|entry| entry.view())
    }

    /// Returns snapshots of every job, oldest first, optionally by owner.
    pub fn list(&self, owner: Option<uuid::Uuid>) -> Vec<JobSnapshot> {
        let mut jobs: Vec<JobSnapshot> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|entry| {
                owner.map(|owner| entry.snapshot.lock().unwrap().owner == owner).unwrap_or(true)
            })
            .map(|entry| entry.view())
            .collect();
        jobs.sort_by_key(|job| job.started_at);
        jobs
    }

    /// Renders the tail of a job's combined output.
    pub fn tail(&self, id: &str, lines: usize) -> String {
        self.entry(id).map(|entry| entry.tail(lines)).unwrap_or_default()
    }

    /// Waits until the job leaves `Running`, returning its final snapshot.
    ///
    /// `None` means the id is unknown (never started here, or the daemon
    /// restarted since).
    pub async fn wait(&self, id: &str) -> Option<JobSnapshot> {
        let entry = self.entry(id)?;
        while entry.state().is_running() {
            self.notified().await;
        }
        Some(entry.view())
    }

    /// Waits until any of `owner`'s jobs finishes and is not in `announced`.
    ///
    /// Already-finished jobs are returned immediately, so a caller that parks
    /// after a short command does not wait at all. `None` means the owner has
    /// no running job left to wait for.
    pub async fn wait_any_finished(
        &self,
        owner: uuid::Uuid,
        announced: &[String],
    ) -> Option<JobSnapshot> {
        loop {
            if let Some(finished) = self.first_finished(owner, announced) {
                return Some(finished);
            }
            if !self.any_running(owner) {
                return None;
            }
            self.notified().await;
        }
    }

    /// Returns the oldest finished job of `owner` that is not announced yet.
    pub fn first_finished(&self, owner: uuid::Uuid, announced: &[String]) -> Option<JobSnapshot> {
        self.list(Some(owner))
            .into_iter()
            .find(|job| job.state.is_finished() && !announced.iter().any(|id| id == &job.id))
    }

    /// Whether any job of `owner` is still running.
    pub fn any_running(&self, owner: uuid::Uuid) -> bool {
        self.list(Some(owner)).iter().any(|job| job.state.is_running())
    }

    /// Requests termination of one job.
    ///
    /// Returns whether a running job was asked to stop; a job that already
    /// finished is left alone (killing is idempotent).
    pub fn kill(&self, id: &str) -> bool {
        let Some(entry) = self.entry(id) else {
            return false;
        };
        if !entry.state().is_running() {
            return false;
        }
        let sender = entry.kill.lock().unwrap().take();
        match sender {
            Some(tx) => tx.send(()).is_ok(),
            // No sender left while the process still runs: a kill is already in
            // flight, which is what the caller asked for.
            None => true,
        }
    }

    /// Requests termination of every running job, whatever its owner.
    ///
    /// Used when a workspace closes: no agent run outlives its workspace, so
    /// neither should its processes.
    pub fn kill_all(&self) -> usize {
        self.list(None)
            .into_iter()
            .filter(|job| job.state.is_running())
            .filter(|job| self.kill(&job.id))
            .count()
    }

    /// Requests termination of every running job of `owner`, returning how many
    /// were asked to stop.
    pub fn kill_owned_by(&self, owner: uuid::Uuid) -> usize {
        self.list(Some(owner))
            .into_iter()
            .filter(|job| job.state.is_running())
            .filter(|job| self.kill(&job.id))
            .count()
    }

    /// Number of tracked jobs (running or finished).
    pub fn len(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }

    /// Whether no job is tracked.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Waits for the next state change, re-checking periodically.
    async fn notified(&self) {
        tokio::select! {
            _ = self.notify.notified() => {}
            _ = tokio::time::sleep(WAIT_POLL_INTERVAL) => {}
        }
    }

    fn entry(&self, id: &str) -> Option<Arc<JobEntry>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    /// Renders a working directory relative to the workspace when possible.
    fn display_cwd(&self, cwd: &Path) -> String {
        let shown = cwd.strip_prefix(&self.root).unwrap_or(cwd);
        let text = shown.to_string_lossy().replace('\\', "/");
        if text.is_empty() {
            ".".to_string()
        } else {
            text
        }
    }
}

/// Completes when a run is cancelled, so a blocking wait can be aborted.
///
/// The flag is polled rather than raced with `select!` because callers combine
/// it with waits that are not cancellation-aware.
pub async fn wait_for_cancel(cancel: Arc<std::sync::atomic::AtomicBool>) {
    loop {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Spawns a task that appends a stream to the job's output buffer.
fn spawn_reader(
    stream: Option<impl AsyncReadExt + Unpin + Send + 'static>,
    entry: Arc<JobEntry>,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut stream = stream?;
    Some(tokio::spawn(async move {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => entry.push_output(&buffer[..read]),
            }
        }
    }))
}

/// Terminates a process *and its children*.
///
/// Killing the shell alone would leave a forked build or test runner running:
/// on Unix the child leads its own process group (see `start`), on Windows
/// `taskkill /T` walks the tree.
pub(crate) async fn terminate(child: &mut tokio::process::Child) {
    let Some(pid) = child.id() else {
        let _ = child.kill().await;
        return;
    };
    #[cfg(unix)]
    {
        // Safe: `killpg` only signals the group the child created.
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
}

/// Builds the platform shell command wrapping a raw command line.
pub fn build_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C");
        // `raw_arg` avoids quoting so cmd receives the line verbatim.
        cmd.raw_arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Generates a short, human-typeable job id.
fn new_job_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Returns the current time in milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
