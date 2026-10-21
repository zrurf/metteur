use crate::chat::wait::{MatchMode, WaitIntent};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Active,
    WaitPreparing,
    Waiting,
    Ready,
    Dispatching,
    Paused,
    Stopped,
    Completed,
    NeedsAttention,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Terminal {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
    Indeterminate,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    Claimed,
    Started,
    Committed,
    InDoubt,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub task: Uuid,
    pub generation: u64,
    pub may_write: bool,
    pub terminal: Option<Terminal>,
    pub result: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeEvent {
    pub id: Uuid,
    pub source_id: String,
    pub generation: u64,
    pub terminal: Terminal,
    pub occurred_ms: u64,
    pub payload: serde_json::Value,
    pub consumed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dispatch {
    pub id: Uuid,
    pub generation: u64,
    pub session_revision: u64,
    pub event_ids: Vec<Uuid>,
    pub turn: Uuid,
    pub state: DispatchState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub max_turns: u32,
    pub used_turns: u32,
    pub token_limit: u64,
    pub reserved_tokens: u64,
    pub consumed_tokens: u64,
    pub unknown_usage: bool,
    pub call_ids: BTreeSet<String>,
}
impl Budget {
    pub fn reserve(&mut self, tokens: u64) -> Result<(), &'static str> {
        if tokens == 0
            || self.used_turns >= self.max_turns
            || self.unknown_usage
            || self.consumed_tokens.saturating_add(self.reserved_tokens).saturating_add(tokens)
                > self.token_limit
        {
            return Err("Continuation budget exhausted or usage unknown");
        }
        self.reserved_tokens += tokens;
        self.used_turns += 1;
        Ok(())
    }
    pub fn settle(&mut self, call: String, reserved: u64, actual: Option<u64>) {
        if !self.call_ids.insert(call) {
            return;
        }
        if let Some(actual) = actual {
            self.reserved_tokens = self.reserved_tokens.saturating_sub(reserved);
            self.consumed_tokens = self.consumed_tokens.saturating_add(actual);
        } else {
            self.unknown_usage = true;
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub session: Uuid,
    pub generation: u64,
    pub expected_revision: u64,
    pub state: TaskState,
    pub paused: bool,
    pub policy_active: bool,
    pub detached: bool,
    pub sources: BTreeMap<String, Source>,
    pub events: BTreeMap<Uuid, WakeEvent>,
    pub wait: Option<WaitIntent>,
    pub dispatch: Option<Dispatch>,
    pub dispatch_history: Vec<Dispatch>,
    pub stop_generation: Option<u64>,
    pub attention: Option<String>,
    pub budget: Budget,
}
impl Task {
    pub fn register_source(&mut self, source: Source) -> Result<(), &'static str> {
        if self.state != TaskState::Active
            || source.task != self.id
            || source.generation != self.generation
            || source.terminal.is_some()
            || self.sources.contains_key(&source.id)
            || self.sources.len() >= 64
        {
            return Err("Invalid or duplicate host source binding");
        }
        self.sources.insert(source.id.clone(), source);
        Ok(())
    }
    /// Host adapter only: the RPC/tool surface must never accept a WakeEvent.
    pub(crate) fn finish_source(
        &mut self,
        id: &str,
        terminal: Terminal,
        payload: serde_json::Value,
        now: u64,
    ) -> Result<Uuid, &'static str> {
        if payload.to_string().len() > 24 * 1024 {
            return Err("Source data requires an artifact reference");
        }
        let source = self.sources.get_mut(id).ok_or("Unknown source")?;
        let event_id = Uuid::new_v5(&self.id, format!("{}:{}", id, source.generation).as_bytes());
        if let Some(event) = self.events.get(&event_id) {
            if event.terminal != terminal || event.payload != payload {
                return Err("Conflicting terminal receipt");
            }
            return Ok(event_id);
        }
        if self.events.len() >= 256 {
            self.attention = Some("Backpressure".into());
            return Err("Event queue requires attention");
        }
        source.terminal = Some(terminal);
        source.result = payload.clone();
        self.events.insert(
            event_id,
            WakeEvent {
                id: event_id,
                source_id: id.into(),
                generation: source.generation,
                terminal,
                occurred_ms: now,
                payload,
                consumed: false,
            },
        );
        self.reconcile(now);
        Ok(event_id)
    }
    pub fn commit_wait(
        &mut self,
        wait: WaitIntent,
        revision: u64,
        now: u64,
    ) -> Result<(), &'static str> {
        wait.validate(now)?;
        if self.state != TaskState::Active
            || !self.policy_active
            || revision != self.expected_revision
        {
            return Err("Task is not eligible to wait");
        }
        if wait.sources.iter().any(|id| {
            self.sources
                .get(id)
                .is_none_or(|s| s.generation != self.generation || s.task != self.id)
        }) {
            return Err("Wait may reference only owned sources in the current generation");
        }
        self.wait = Some(wait);
        self.state = TaskState::Waiting;
        self.reconcile(now);
        Ok(())
    }
    pub fn reconcile(&mut self, now: u64) {
        if !matches!(self.state, TaskState::Waiting | TaskState::Ready | TaskState::Paused) {
            return;
        }
        if self.paused {
            self.state = TaskState::Paused;
            return;
        }
        let Some(wait) = &self.wait else {
            return;
        };
        if now >= wait.deadline_ms {
            let source_id = format!("deadline:{}:{}", self.generation, wait.deadline_ms);
            let id = Uuid::new_v5(&self.id, source_id.as_bytes());
            self.events.entry(id).or_insert(WakeEvent {
                id,
                source_id,
                generation: self.generation,
                terminal: Terminal::TimedOut,
                occurred_ms: wait.deadline_ms,
                payload: serde_json::json!({"reason":"wait_deadline"}),
                consumed: false,
            });
            self.state = TaskState::Ready;
            return;
        }
        let count = wait
            .sources
            .iter()
            .filter(|id| {
                self.events
                    .values()
                    .any(|e| &e.source_id == *id && e.generation == self.generation && !e.consumed)
            })
            .count();
        let ready = match wait.mode {
            MatchMode::Any => count > 0,
            MatchMode::All => count == wait.sources.len(),
        };
        self.state = if ready {
            TaskState::Ready
        } else {
            TaskState::Waiting
        };
    }
    pub fn claim(
        &mut self,
        revision: u64,
        now: u64,
        token_reserve: u64,
    ) -> Result<Uuid, &'static str> {
        self.reconcile(now);
        if self.state != TaskState::Ready
            || !self.policy_active
            || self.paused
            || self.stop_generation.is_some()
            || self.dispatch.is_some()
        {
            return Err("Task is not ready");
        }
        if revision != self.expected_revision {
            self.state = TaskState::NeedsAttention;
            self.attention = Some("Session changed".into());
            return Err("Session revision changed");
        }
        if self.sources.values().any(|s| {
            s.may_write
                && (s.terminal.is_none()
                    || matches!(
                        s.terminal,
                        Some(Terminal::Indeterminate | Terminal::Interrupted | Terminal::Cancelled)
                    ))
        }) {
            return Err("Background write effect remains unresolved");
        }
        let wait = self.wait.as_ref().ok_or("No committed wait")?;
        let mut events: Vec<_> = self
            .events
            .values()
            .filter(|e| {
                !e.consumed
                    && e.generation == self.generation
                    && (wait.sources.contains(&e.source_id)
                        || e.source_id
                            == format!("deadline:{}:{}", self.generation, wait.deadline_ms))
            })
            .collect();
        events.sort_by_key(|e| (e.occurred_ms, e.id));
        let ids: Vec<_> = events.into_iter().take(16).map(|e| e.id).collect();
        if ids.is_empty() {
            return Err("No matching events");
        }
        let key =
            serde_json::to_vec(&(self.generation, &ids)).map_err(|_| "Invalid event batch")?;
        let id = Uuid::new_v5(&self.id, &key);
        if token_reserve > 0 {
            self.budget.reserve(token_reserve)?;
        } else {
            if self.budget.unknown_usage || self.budget.used_turns >= self.budget.max_turns {
                return Err("Continuation budget exhausted");
            }
            self.budget.used_turns += 1;
        }
        self.dispatch = Some(Dispatch {
            id,
            generation: self.generation,
            session_revision: revision,
            event_ids: ids,
            turn: Uuid::new_v4(),
            state: DispatchState::Claimed,
        });
        self.state = TaskState::Dispatching;
        Ok(id)
    }
    pub fn started(&mut self, id: Uuid) -> Result<Uuid, &'static str> {
        if self.state != TaskState::Dispatching || !self.policy_active || self.paused {
            return Err("Dispatch revoked");
        }
        let d = self.dispatch.as_mut().ok_or("No dispatch")?;
        if d.id != id || d.generation != self.generation || d.state != DispatchState::Claimed {
            return Err("Stale dispatch");
        }
        d.state = DispatchState::Started;
        self.state = TaskState::Active;
        Ok(d.turn)
    }
    pub fn commit_dispatch(&mut self, id: Uuid, revision: u64) -> Result<(), &'static str> {
        let d = self.dispatch.as_mut().ok_or("No dispatch")?;
        if d.id != id
            || d.generation != self.generation
            || !matches!(d.state, DispatchState::Started | DispatchState::Committed)
        {
            return Err("Stale dispatch commit");
        }
        d.state = DispatchState::Committed;
        for id in &d.event_ids {
            if let Some(e) = self.events.get_mut(id) {
                e.consumed = true;
            }
        }
        self.expected_revision = revision;
        Ok(())
    }
    pub fn cancel_timer_sources(&mut self, reason: &str, now: u64) -> Result<(), &'static str> {
        let ids: Vec<_> = self.sources.values().filter(|s|s.kind=="timer" && s.terminal.is_none()).map(|s|s.id.clone()).collect();
        for id in ids {self.finish_source(&id,Terminal::Cancelled,serde_json::json!({"reason":reason}),now)?;}
        Ok(())
    }
    pub fn stop(&mut self) {
        self.stop_generation = Some(self.generation);
        self.generation = self.generation.saturating_add(1);
        self.policy_active = false;
        self.state = TaskState::Stopped;
    }
    pub fn recover(&mut self) {
        if self.stop_generation.is_some() {
            self.state = TaskState::Stopped;
            return;
        }
        if let Some(d) = &mut self.dispatch {
            if matches!(d.state, DispatchState::Started | DispatchState::InDoubt) {
                d.state = DispatchState::InDoubt;
                self.state = TaskState::NeedsAttention;
                self.attention =
                    Some("Turn may have performed effects; automatic replay blocked".into());
            }
            // Claimed is deliberately not retried merely because a lease expired.
            else if d.state == DispatchState::Claimed {
                self.state = TaskState::NeedsAttention;
                self.attention = Some("Claim requires proof that no turn started".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task() -> Task {
        Task {
            id: Uuid::new_v4(),
            session: Uuid::new_v4(),
            generation: 1,
            expected_revision: 3,
            state: TaskState::Active,
            paused: false,
            policy_active: true,
            detached: true,
            sources: BTreeMap::new(),
            events: BTreeMap::new(),
            wait: None,
            dispatch: None,
            dispatch_history: vec![],
            stop_generation: None,
            attention: None,
            budget: Budget {
                max_turns: 3,
                used_turns: 0,
                token_limit: 1000,
                reserved_tokens: 0,
                consumed_tokens: 0,
                unknown_usage: false,
                call_ids: BTreeSet::new(),
            },
        }
    }
    fn source(t: &Task, id: &str) -> Source {
        Source {
            id: id.into(),
            kind: "job".into(),
            task: t.id,
            generation: t.generation,
            may_write: false,
            terminal: None,
            result: serde_json::Value::Null,
        }
    }
    #[test]
    fn terminal_before_wait_deduplicates_and_stop_fences_dispatch() {
        let mut t = task();
        t.register_source(source(&t, "job-1")).unwrap();
        let id =
            t.finish_source("job-1", Terminal::Failed, serde_json::json!({"exit":2}), 10).unwrap();
        assert_eq!(
            id,
            t.finish_source("job-1", Terminal::Failed, serde_json::json!({"exit":2}), 11).unwrap()
        );
        t.commit_wait(
            WaitIntent {
                sources: vec!["job-1".into()],
                mode: MatchMode::All,
                deadline_ms: 100,
                params: serde_json::json!({}),
            },
            3,
            12,
        )
        .unwrap();
        assert_eq!(t.state, TaskState::Ready);
        let d = t.claim(3, 13, 100).unwrap();
        assert!(t.claim(3, 13, 100).is_err());
        t.stop();
        assert!(t.started(d).is_err());
        t.recover();
        assert_eq!(t.state, TaskState::Stopped);
    }
    #[test]
    fn all_sources_conflict_and_unknown_usage_are_not_success() {
        let mut t = task();
        t.register_source(source(&t, "a")).unwrap();
        t.register_source(source(&t, "b")).unwrap();
        t.commit_wait(
            WaitIntent {
                sources: vec!["a".into(), "b".into()],
                mode: MatchMode::All,
                deadline_ms: 100,
                params: serde_json::json!({}),
            },
            3,
            1,
        )
        .unwrap();
        t.finish_source("a", Terminal::Succeeded, serde_json::json!({}), 2).unwrap();
        assert_eq!(t.state, TaskState::Waiting);
        t.finish_source("b", Terminal::Cancelled, serde_json::json!({}), 3).unwrap();
        assert_eq!(t.state, TaskState::Ready);
        assert!(t.claim(4, 4, 100).is_err());
        assert_eq!(t.state, TaskState::NeedsAttention);
        let mut b = t.budget;
        b.reserve(100).unwrap();
        b.settle("call".into(), 100, None);
        assert!(b.reserve(100).is_err());
        assert_eq!(b.reserved_tokens, 100);
    }
    #[test]
    fn durable_store_keeps_wait_context_and_receipts_across_restart() {
        use crate::continuation::store::ContinuationStore;
        use crate::storage::persistence::Db;
        let root = std::env::temp_dir().join(format!("nx-cont-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db")).unwrap();
        let store = ContinuationStore::new(db.clone(), &root).unwrap();
        let mut t = task();
        t.expected_revision = 0;
        t.register_source(source(&t, "job")).unwrap();
        let id = t.id;
        let r = store.create(t).unwrap();
        let mut context = crate::chat::session::ChatSessionRecord::new(1, Default::default());
        context.session_id = r.task.session;
        let r = store
            .save_wait(
                id,
                1,
                context,
                WaitIntent {
                    sources: vec!["job".into()],
                    mode: MatchMode::Any,
                    deadline_ms: 100,
                    params: serde_json::json!({}),
                },
                2,
            )
            .unwrap();
        let r = store
            .update(id, r.revision, |r| {
                r.task
                    .finish_source(
                        "job",
                        Terminal::Succeeded,
                        serde_json::json!({"summary":"done"}),
                        3,
                    )
                    .unwrap();
                Ok(())
            })
            .unwrap();
        assert!(store.update(id, 1, |_| Ok(())).is_err());
        assert!(r.saved_context.is_some());
        drop(store);
        drop(db);
        let db = Db::open(&root.join("db")).unwrap();
        let store = ContinuationStore::new(db, &root).unwrap();
        let r = store.get(id).unwrap().unwrap();
        assert_eq!(r.task.state, TaskState::Ready);
        assert_eq!(r.task.events.len(), 1);
        assert!(r.saved_context.is_some());
    }
    #[test]
    fn crash_windows_timeout_and_stop_never_duplicate_dispatch() {
        let mut t = task();
        t.register_source(source(&t, "job")).unwrap();
        t.commit_wait(
            WaitIntent {
                sources: vec!["job".into()],
                mode: MatchMode::All,
                deadline_ms: 10,
                params: serde_json::json!({}),
            },
            3,
            1,
        )
        .unwrap();
        t.reconcile(11);
        assert_eq!(t.state, TaskState::Ready);
        assert_eq!(t.events.len(), 1);
        t.reconcile(12);
        assert_eq!(t.events.len(), 1);
        let id = t.claim(3, 12, 0).unwrap();
        assert!(t.claim(3, 12, 0).is_err());
        let mut claimed = t.clone();
        claimed.recover();
        assert_eq!(claimed.state, TaskState::NeedsAttention);
        t.started(id).unwrap();
        t.recover();
        assert_eq!(t.dispatch.as_ref().unwrap().state, DispatchState::InDoubt);
        assert!(t.claim(3, 13, 0).is_err());
        t.stop();
        t.finish_source("job", Terminal::Succeeded, serde_json::json!({}), 14).unwrap();
        t.recover();
        assert_eq!(t.state, TaskState::Stopped);
        assert!(t.commit_dispatch(id, 4).is_err());
    }
    #[test]
    fn failed_wait_is_atomic_and_stopped_recovery_retains_artifacts() {
        use crate::continuation::store::ContinuationStore;
        use crate::storage::persistence::Db;
        let root = std::env::temp_dir().join(format!("nx-fault-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(&root.join("db")).unwrap();
        let store = ContinuationStore::new(db.clone(), &root).unwrap();
        let mut t = task();
        t.expected_revision = 0;
        t.register_source(source(&t, "job")).unwrap();
        let id = t.id;
        let r = store.create(t).unwrap();
        let mut context = crate::chat::session::ChatSessionRecord::new(1, Default::default());
        context.session_id = r.task.session;
        assert!(
            store
                .save_wait(
                    id,
                    r.revision,
                    context,
                    WaitIntent {
                        sources: vec!["foreign".into()],
                        mode: MatchMode::Any,
                        deadline_ms: 100,
                        params: serde_json::json!({})
                    },
                    2
                )
                .is_err()
        );
        assert_eq!(db.chat_revision(r.task.session.as_bytes()).unwrap(), 0);
        assert!(store.get(id).unwrap().unwrap().saved_context.is_none());
        let artifact = store.result_artifact(id, "job", &"证据".repeat(9000)).unwrap();
        let r = store
            .update(id, r.revision, |r| {
                r.task.finish_source("job", Terminal::Succeeded, artifact, 3).unwrap();
                r.task.register_source(source(&r.task, "child")).unwrap();
                r.task.stop();
                Ok(())
            })
            .unwrap();
        let first = store.read_result_artifact(id, "job", 0).unwrap();
        let offset = first["next_offset"].as_u64().unwrap() as usize;
        assert!(offset > 0);
        assert!(store.read_result_artifact(id, "job", offset).is_ok());
        assert!(store.read_result_artifact(id, "foreign", 0).is_err());
        store.recover().unwrap();
        let recovered = store.get(id).unwrap().unwrap();
        assert_eq!(recovered.task.state, TaskState::Stopped);
        assert_eq!(recovered.task.generation, r.task.generation);
        assert_eq!(recovered.task.sources["child"].terminal, Some(Terminal::Indeterminate));
    }
    #[test]
    fn nx_m6_bounded_duplicate_burst_and_repeated_recovery() {
        let start = std::time::Instant::now();
        let mut t = task();
        for i in 0..64 { t.register_source(source(&t, &format!("job-{i}"))).unwrap(); }
        assert!(t.register_source(source(&t, "overflow")).is_err());
        for i in 0..64 { t.finish_source(&format!("job-{i}"), Terminal::Succeeded, serde_json::json!({"summary":"done"}), 10).unwrap(); }
        let before = serde_json::to_value(&t.events).unwrap();
        for i in 0..4096 { t.finish_source(&format!("job-{}", i%64), Terminal::Succeeded, serde_json::json!({"summary":"done"}), 11).unwrap(); }
        assert_eq!(t.events.len(),64);
        assert_eq!(serde_json::to_value(&t.events).unwrap(),before);
        t.stop();
        let generation = t.generation;
        for _ in 0..8 { t.recover(); }
        assert_eq!(t.state,TaskState::Stopped);
        assert_eq!(t.generation,generation);
        assert_eq!(t.events.len(),64);
        eprintln!("NX-M6 bounded probe: 64 sources, 4096 duplicate notifications, 8 recoveries; elapsed_us={}",start.elapsed().as_micros());
    }

}
