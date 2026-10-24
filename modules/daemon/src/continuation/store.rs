//! Durable task/source/event/dispatch transactions share one authoritative record.
//! Notifications are hints: recovery scans these records, never an in-memory inbox.
use super::state::{Task, TaskState};
use crate::{
    chat::session::ChatSessionRecord,
    error::{DaemonError, DaemonResult},
    storage::{
        persistence::{Db, cf},
        resources::{ResourceStore, digest},
    },
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    #[serde(default)]
    pub rebind_receipt: Option<(Uuid, String)>,
    pub schema_version: u32,
    pub revision: u64,
    pub workspace: String,
    pub task: Task,
    pub saved_context: Option<ChatSessionRecord>,
    #[serde(default)]
    pub owner_subject: String,
    #[serde(default)]
    pub model_options: serde_json::Value,
    #[serde(default)]
    pub policy: Option<super::policy::Policy>,
    #[serde(default)]
    pub schedules: Vec<super::timer::Schedule>,
    #[serde(default)]
    pub binding_digest: Option<String>,
    #[serde(default)]
    pub launch_intent: Option<Uuid>,
}
#[derive(Clone)]
pub struct ContinuationStore {
    db: Db,
    workspace: String,
    prefix: String,
}
impl ContinuationStore {
    /// Construct a read view without creating directories, a checkpoint or a
    /// schema marker. Unknown resource formats remain unavailable.
    pub(crate) fn inspect(db: Db, root: &Path) -> DaemonResult<Self> {
        let root = root.canonicalize()?;
        if ResourceStore::inspect(&db).map_err(|e| DaemonError::Persistence(e.to_string()))?
            .is_some_and(|schema| schema != metteur_shared::resources::SCHEMA_VERSION) {
            return Err(DaemonError::Persistence("Unknown resource schema".into()));
        }
        let workspace = root.to_string_lossy().into_owned();
        Ok(Self {db, prefix: format!("nx/v1/continuation/{}/", digest(workspace.as_bytes())), workspace})
    }
    pub(crate) fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.db.subscribe_changes()
    }
    pub fn new(db: Db, root: &Path) -> DaemonResult<Self> {
        let root = root.canonicalize()?;
        std::fs::create_dir_all(root.join(".metteur"))?;
        ResourceStore::prepare(&db, &root.join(".metteur/nx-before-v1"))
            .map_err(|e| DaemonError::Persistence(e.to_string()))?;
        let workspace = root.to_string_lossy().into_owned();
        Ok(Self {
            db,
            prefix: format!("nx/v1/continuation/{}/", digest(workspace.as_bytes())),
            workspace,
        })
    }
    fn key(&self, id: Uuid) -> Vec<u8> {
        format!("{}task/{id}", self.prefix).into_bytes()
    }
    pub fn get(&self, id: Uuid) -> DaemonResult<Option<TaskRecord>> {
        self.db
            .get(cf::EXECUTION_STATE, &self.key(id))?
            .map(|raw| {
                let r: TaskRecord = serde_json::from_slice(&raw)
                    .map_err(|e| DaemonError::Serialization(e.to_string()))?;
                if r.schema_version != 1 || r.workspace != self.workspace {
                    return Err(DaemonError::Persistence(
                        "Invalid continuation record identity or schema".into(),
                    ));
                }
                Ok(r)
            })
            .transpose()
    }
    /// Owned, redacted result artifacts live outside the bounded event envelope.
    pub fn result_artifact(
        &self,
        task: Uuid,
        source: &str,
        text: &str,
    ) -> DaemonResult<serde_json::Value> {
        let r = self.get(task)?.ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
        if !r.task.sources.contains_key(source) {
            return Err(DaemonError::PermissionDenied("Unowned source".into()));
        }
        let mut end = text.len().min(256 * 1024);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let body = &text[..end];
        let hash = digest(body.as_bytes());
        let key =
            format!("{}artifact/{}/{}/{}", self.prefix, task, digest(source.as_bytes()), hash);
        self.db.continuation_batch(&[(key.into_bytes(), body.as_bytes().to_vec())], None)?;
        Ok(
            serde_json::json!({"summary":crate::registry::tools::result::truncate_result(body,8192),"artifact_ref":hash,"artifact_bytes":end,"truncated":end<text.len(),"origin":"source_data"}),
        )
    }
    pub fn read_result_artifact(
        &self,
        task: Uuid,
        source: &str,
        offset: usize,
    ) -> DaemonResult<serde_json::Value> {
        let r = self.get(task)?.ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
        let result = &r
            .task
            .sources
            .get(source)
            .ok_or_else(|| DaemonError::PermissionDenied("Unowned source".into()))?
            .result;
        let hash = result
            .get("artifact_ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DaemonError::Execution("No result artifact".into()))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DaemonError::Persistence("Invalid artifact identity".into()));
        }
        let key =
            format!("{}artifact/{}/{}/{}", self.prefix, task, digest(source.as_bytes()), hash);
        let bytes = self
            .db
            .get(cf::EXECUTION_STATE, key.as_bytes())?
            .ok_or_else(|| DaemonError::Persistence("Result artifact missing".into()))?;
        let body = String::from_utf8(bytes)
            .map_err(|_| DaemonError::Persistence("Invalid artifact encoding".into()))?;
        if digest(body.as_bytes()) != hash || offset > body.len() || !body.is_char_boundary(offset)
        {
            return Err(DaemonError::Execution("Invalid artifact offset or digest".into()));
        }
        let mut end = (offset + 8192).min(body.len());
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        Ok(
            serde_json::json!({"origin":"source_data","source_id":source,"content":&body[offset..end],"offset":offset,"next_offset":(end<body.len()).then_some(end),"total_bytes":body.len(),"truncated":result.get("truncated")}),
        )
    }
    pub fn list(&self, after: Option<Uuid>) -> DaemonResult<Vec<TaskRecord>> {
        let prefix = format!("{}task/", self.prefix);
        let after = after.map(|id| self.key(id)).unwrap_or_default();
        self.db
            .resource_scan(prefix.as_bytes(), &after, 50)?
            .into_iter()
            .map(|(_, raw)| {
                let r: TaskRecord = serde_json::from_slice(&raw)
                    .map_err(|e| DaemonError::Serialization(e.to_string()))?;
                if r.schema_version != 1 || r.workspace != self.workspace {
                    return Err(DaemonError::Persistence("Unknown continuation schema".into()));
                }
                Ok(r)
            })
            .collect()
    }
    pub fn create(&self, task: Task) -> DaemonResult<TaskRecord> {
        let _guard = self
            .db
            .resource_gate
            .lock()
            .map_err(|_| DaemonError::Persistence("Continuation store lock failed".into()))?;
        let mut cursor = None;
        let mut active = 0usize;
        loop {
            let page = self.list(cursor)?;
            if page.is_empty() {
                break;
            }
            for r in page {
                cursor = Some(r.task.id);
                if !matches!(r.task.state, TaskState::Stopped | TaskState::Completed) {
                    active += 1;
                }
            }
            if active >= 64 {
                return Err(DaemonError::Execution(
                    "Workspace continuation capacity reached".into(),
                ));
            }
        }
        if self.get(task.id)?.is_some() {
            return Err(DaemonError::Execution("Task already exists".into()));
        }
        self.write(TaskRecord {
            rebind_receipt: None,
            schema_version: 1,
            revision: 1,
            workspace: self.workspace.clone(),
            task,
            saved_context: None,
            owner_subject: String::new(),
            model_options: serde_json::Value::Null,
            policy: None,
            schedules: vec![],
            binding_digest: None,
            launch_intent: None,
        })
    }
    pub fn update(
        &self,
        id: Uuid,
        expected: u64,
        change: impl FnOnce(&mut TaskRecord) -> DaemonResult<()>,
    ) -> DaemonResult<TaskRecord> {
        let _guard = self
            .db
            .resource_gate
            .lock()
            .map_err(|_| DaemonError::Persistence("Continuation store lock failed".into()))?;
        let mut r = self.get(id)?.ok_or_else(|| DaemonError::Execution("Task not found".into()))?;
        if r.revision != expected {
            return Err(DaemonError::Execution("Continuation revision conflict".into()));
        }
        let identity = (r.task.id, r.task.session, r.workspace.clone());
        let before =
            serde_json::to_vec(&r).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        change(&mut r)?;
        if serde_json::to_vec(&r).map_err(|e| DaemonError::Serialization(e.to_string()))? == before
        {
            return Ok(r);
        }
        if identity != (r.task.id, r.task.session, r.workspace.clone()) {
            return Err(DaemonError::Execution("Task identity cannot change".into()));
        }
        r.revision = r
            .revision
            .checked_add(1)
            .ok_or_else(|| DaemonError::Execution("Revision exhausted".into()))?;
        self.write(r)
    }
    fn write(&self, r: TaskRecord) -> DaemonResult<TaskRecord> {
        if ResourceStore::inspect(&self.db).map_err(|e| DaemonError::Persistence(e.to_string()))?
            != Some(1)
        {
            return Err(DaemonError::Persistence("Unsupported resource schema".into()));
        }
        let raw = serde_json::to_vec(&r).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        if raw.len() > 8 * 1024 * 1024 {
            return Err(DaemonError::Execution(
                "Continuation record exceeds bounded storage".into(),
            ));
        }
        // Task status, source terminal/outbox and inbox receipts commit together.
        self.db.resource_batch(&[
            (self.key(r.task.id), raw.clone()),
            (format!("{}history/{}/{:020}", self.prefix, r.task.id, r.revision).into_bytes(), raw),
        ])?;
        Ok(r)
    }
    pub fn save_wait(
        &self,
        id: Uuid,
        expected: u64,
        context: ChatSessionRecord,
        wait: crate::chat::wait::WaitIntent,
        now: u64,
    ) -> DaemonResult<TaskRecord> {
        self.save_turn(id, expected, context, Some(wait), true, now)
    }
    /// Caller holds the shared resource gate. These values are committed with
    /// the accepted manual input and its session revision, never independently.
    pub(crate) fn accepted_input_values(&self, id: Uuid, session: Uuid, previous: u64, accepted: u64) -> DaemonResult<Vec<(Vec<u8>,Vec<u8>)>> {
        let mut record=self.get(id)?.ok_or_else(||DaemonError::Execution("Input task missing".into()))?;
        if record.task.session!=session || record.task.expected_revision!=previous || matches!(record.task.state,TaskState::Stopped|TaskState::Completed) || record.task.dispatch.is_some() {
            return Err(DaemonError::Execution("Input task or session changed before acceptance".into()));
        }
        record.task.expected_revision=accepted;
        record.revision=record.revision.checked_add(1).ok_or_else(||DaemonError::Execution("Task revision exhausted".into()))?;
        let raw=serde_json::to_vec(&record).map_err(|e|DaemonError::Serialization(e.to_string()))?;
        Ok(vec![(self.key(id),raw.clone()),(format!("{}history/{}/{:020}",self.prefix,id,record.revision).into_bytes(),raw)])
    }
    /// Context, task transition and consumed-event receipts share one WAL batch.
    pub fn save_turn(
        &self,
        id: Uuid,
        expected: u64,
        context: ChatSessionRecord,
        mut wait: Option<crate::chat::wait::WaitIntent>,
        success: bool,
        now: u64,
    ) -> DaemonResult<TaskRecord> {
        let _guard = self
            .db
            .resource_gate
            .lock()
            .map_err(|_| DaemonError::Persistence("Continuation lock unavailable".into()))?;
        if ResourceStore::inspect(&self.db).map_err(|e| DaemonError::Persistence(e.to_string()))?
            != Some(1)
        {
            return Err(DaemonError::Persistence("Unsupported resource schema".into()));
        }
        let mut r = self.get(id)?.ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
        if r.revision != expected || context.session_id != r.task.session {
            return Err(DaemonError::Execution("Wait revision or session mismatch".into()));
        }
        let previous = self.db.chat_revision(context.session_id.as_bytes())?;
        if previous != r.task.expected_revision
            || matches!(r.task.state, TaskState::Stopped | TaskState::Completed)
        {
            return Err(DaemonError::Execution("Session or task changed".into()));
        }
        if let Some(dispatch) = r.task.dispatch.clone() {
            r.task
                .commit_dispatch(dispatch.id, previous)
                .map_err(|e| DaemonError::Execution(e.into()))?;
            let mut committed = dispatch;
            committed.state = super::state::DispatchState::Committed;
            r.task.dispatch_history.push(committed);
            r.task.dispatch = None;
        }
        if success && wait.is_none() {
            if let Some(schedule) = r.schedules.iter().find(|s| {
                !s.cancelled
                    && !s.paused
                    && s.next_due.is_some()
                    && s.occurrences < s.max_occurrences
                    && s.expires_at.timestamp_millis() > now as i64
            }) {
                let source_id = format!(
                    "timer:{}:{}:{}",
                    schedule.id, schedule.generation, schedule.occurrences
                );
                r.task.sources.insert(
                    source_id.clone(),
                    super::state::Source {
                        id: source_id.clone(),
                        kind: "timer".into(),
                        task: id,
                        generation: r.task.generation,
                        may_write: false,
                        terminal: None,
                        result: serde_json::Value::Null,
                    },
                );
                wait = Some(crate::chat::wait::WaitIntent {
                    sources: vec![source_id],
                    mode: crate::chat::wait::MatchMode::Any,
                    deadline_ms: schedule.expires_at.timestamp_millis() as u64,
                    params: schedule.params.clone(),
                });
            }
        }
        if let Some(wait) = wait {
            r.task
                .commit_wait(wait, previous, now)
                .map_err(|e| DaemonError::Execution(e.into()))?;
        } else {
            if !success {
                r.task.attention.get_or_insert_with(|| {
                    "Turn failed; open its session to inspect the saved error".into()
                });
            }
            r.task.state = if success {
                TaskState::Completed
            } else {
                TaskState::NeedsAttention
            };
        }
        r.task.expected_revision = previous
            .checked_add(1)
            .ok_or_else(|| DaemonError::Execution("Session revision exhausted".into()))?;
        r.revision += 1;
        r.saved_context = Some(context.clone());
        let raw = serde_json::to_vec(&r).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        if raw.len() > 8 * 1024 * 1024 {
            return Err(DaemonError::Execution("Continuation storage limit".into()));
        }
        let chat =
            serde_json::to_vec(&context).map_err(|e| DaemonError::Serialization(e.to_string()))?;
        self.db.continuation_batch(
            &[
                (self.key(id), raw.clone()),
                (format!("{}history/{}/{:020}", self.prefix, id, r.revision).into_bytes(), raw),
            ],
            Some((context.session_id.as_bytes(), &chat, previous)),
        )?;
        Ok(r)
    }
    pub fn recover(&self) -> DaemonResult<usize> {
        let mut cursor = None;
        let mut changed = 0;
        loop {
            let page = self.list(cursor)?;
            if page.is_empty() {
                break;
            }
            for r in page {
                cursor = Some(r.task.id);
                if !matches!(r.task.state, TaskState::Stopped | TaskState::Completed)
                    || r.task.sources.values().any(|s| {
                        s.terminal.is_none() && matches!(s.kind.as_str(), "job" | "child" | "timer")
                    })
                    || r.task.budget.reserved_tokens > 0
                {
                    self.update(r.task.id,r.revision,|r|{
                        let terminal_state = matches!(r.task.state,TaskState::Stopped|TaskState::Completed).then_some(r.task.state);
                        let sources:Vec<_>=r.task.sources.values().filter(|s|s.terminal.is_none()&&matches!(s.kind.as_str(),"job"|"child")).map(|s|(s.id.clone(),s.kind.clone())).collect();
                        for (id,kind) in sources {r.task.finish_source(&id,if kind=="job"{super::state::Terminal::Indeterminate}else{super::state::Terminal::Interrupted},serde_json::json!({"reason":"host_restarted","automatic_replay":false}),chrono::Utc::now().timestamp_millis().max(0) as u64).map_err(|e|DaemonError::Execution(e.into()))?;}
                        if let Some(state) = terminal_state {r.task.cancel_timer_sources("task_already_ended",chrono::Utc::now().timestamp_millis().max(0) as u64).map_err(|e|DaemonError::Execution(e.into()))?; r.task.state=state; } else { r.task.recover(); }
                        if r.task.state==TaskState::Active {r.task.state=TaskState::NeedsAttention;r.task.attention=Some("Host restarted during active turn".into());}
                        if r.task.budget.reserved_tokens>0 {r.task.budget.unknown_usage=true;}
                        Ok(())
                    })?;
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }
}

/// A host-created handle cannot be supplied through a tool's JSON arguments.
#[derive(Clone)]
pub struct TaskHandle {
    pub store: ContinuationStore,
    pub id: Uuid,
    pub generation: u64,
}
impl TaskHandle {
    pub fn change(
        &self,
        change: impl Fn(&mut TaskRecord) -> DaemonResult<()>,
    ) -> DaemonResult<TaskRecord> {
        for _ in 0..4 {
            let current = self
                .store
                .get(self.id)?
                .ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
            if current.task.generation != self.generation {
                return Err(DaemonError::Execution("Task generation revoked".into()));
            }
            match self.store.update(self.id, current.revision, |r| change(r)) {
                Err(DaemonError::Execution(s)) if s == "Continuation revision conflict" => continue,
                result => return result,
            }
        }
        Err(DaemonError::Execution("Concurrent task update; retry explicitly".into()))
    }
    pub fn register_run_job(&self, id: &str, run: Uuid) -> DaemonResult<()> {
        let record = self
            .store
            .get(self.id)?
            .ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
        let launches = super::launch::LaunchStore::new(
            self.store.db.clone(),
            std::path::Path::new(&record.workspace),
        )?;
        self.change(|r| {
            let launch = r
                .launch_intent
                .ok_or_else(|| DaemonError::PermissionDenied("No parent launch".into()))?;
            let (_, intent) = launches.get(launch, &r.owner_subject)?;
            if intent.run != Some(run)
                || intent.state != super::launch::LaunchState::Running
                || !r.task.policy_active
                || r.task.generation != self.generation
                || !matches!(r.task.state, TaskState::Waiting | TaskState::Paused)
                || r.task.sources.len() >= 64
                || r.task.sources.contains_key(id)
            {
                return Err(DaemonError::PermissionDenied(
                    "Run job is not owned by the active launch".into(),
                ));
            }
            r.task.sources.insert(
                id.into(),
                super::state::Source {
                    id: id.into(),
                    kind: "job".into(),
                    task: r.task.id,
                    generation: self.generation,
                    may_write: true,
                    terminal: None,
                    result: serde_json::Value::Null,
                },
            );
            Ok(())
        })?;
        Ok(())
    }
    pub fn register(&self, id: &str, kind: &str, may_write: bool) -> DaemonResult<()> {
        self.change(|r| {
            r.task
                .register_source(super::state::Source {
                    id: id.into(),
                    kind: kind.into(),
                    task: self.id,
                    generation: self.generation,
                    may_write,
                    terminal: None,
                    result: serde_json::Value::Null,
                })
                .map_err(|s| DaemonError::Execution(s.into()))
        })?;
        Ok(())
    }
    pub fn finish(
        &self,
        id: &str,
        status: super::state::Terminal,
        data: serde_json::Value,
        now: u64,
    ) -> DaemonResult<()> {
        // A late terminal result is still evidence, even when the task was stopped.
        for _ in 0..4 {
            let r = self
                .store
                .get(self.id)?
                .ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
            let result = self.store.update(self.id, r.revision, |r| {
                let source = r
                    .task
                    .sources
                    .get(id)
                    .ok_or_else(|| DaemonError::Execution("Source missing".into()))?;
                if source.generation != self.generation {
                    return Err(DaemonError::Execution("Source generation mismatch".into()));
                }
                r.task
                    .finish_source(id, status, data.clone(), now)
                    .map_err(|s| DaemonError::Execution(s.into()))?;
                Ok(())
            });
            match result {
                Err(DaemonError::Execution(s)) if s == "Continuation revision conflict" => continue,
                other => {
                    other?;
                    return Ok(());
                }
            }
        }
        Err(DaemonError::Execution("Could not commit source receipt".into()))
    }
}

impl TaskHandle {
    pub fn reserve_call(&self, tokens: u64) -> DaemonResult<()> {
        self.change(|r| {
            if !r.task.policy_active
                || matches!(
                    r.task.state,
                    TaskState::Stopped | TaskState::Completed | TaskState::NeedsAttention
                )
                || r.policy.as_ref().is_some_and(|p| {
                    p.expires_ms <= chrono::Utc::now().timestamp_millis().max(0) as u64
                })
            {
                return Err(DaemonError::PermissionDenied(
                    "Continuation policy inactive or expired".into(),
                ));
            }
            let b = &mut r.task.budget;
            if b.unknown_usage
                || tokens == 0
                || b.consumed_tokens.saturating_add(b.reserved_tokens).saturating_add(tokens)
                    > b.token_limit
            {
                return Err(DaemonError::Execution(
                    "Root continuation budget exhausted or unknown".into(),
                ));
            }
            b.reserved_tokens += tokens;
            Ok(())
        })?;
        Ok(())
    }
    pub fn settle_call(&self, call: &str, reserved: u64, actual: Option<u64>) -> DaemonResult<()> {
        for _ in 0..4 {
            let current = self
                .store
                .get(self.id)?
                .ok_or_else(|| DaemonError::Execution("Task missing".into()))?;
            match self.store.update(self.id, current.revision, |r| {
                r.task.budget.settle(call.into(), reserved, actual);
                Ok(())
            }) {
                Err(DaemonError::Execution(s)) if s == "Continuation revision conflict" => continue,
                result => {
                    result?;
                    return Ok(());
                }
            }
        }
        Err(DaemonError::Persistence("Usage settlement requires reconciliation".into()))
    }
}
