//! Best-effort, bounded post-commit observers. This module exposes no model tool.
use super::package::Package;
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicU8, Ordering},
    },
};

const QUEUE_LIMIT: usize = 32;
const HISTORY_LIMIT: usize = 128;
const RUN_EVENT_LIMIT: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub name: String,
    pub event: String,
    pub scope_root: String,
    pub event_id: String,
    pub status: String,
    pub completed: u64,
    pub failed: u64,
    pub error: String,
}
#[derive(Clone, Serialize)]
pub(crate) struct Event {
    event: &'static str,
    event_id: String,
    workspace: String,
    run_id: Option<uuid::Uuid>,
    node_id: Option<uuid::Uuid>,
    sequence: Option<u64>,
    status: &'static str,
}
impl Event {
    fn workspace(root: &Path, open: bool) -> Self {
        Self {
            event: if open {
                "workspace.open"
            } else {
                "workspace.close"
            },
            event_id: uuid::Uuid::new_v4().to_string(),
            workspace: scope_hash(root),
            run_id: None,
            node_id: None,
            sequence: None,
            status: "Committed",
        }
    }
    fn node(root: &Path, run: uuid::Uuid, node: uuid::Uuid, sequence: u64, status: &str) -> Self {
        Self {
            event: "node.finished",
            event_id: format!("{run}:node:{sequence}"),
            workspace: scope_hash(root),
            run_id: Some(run),
            node_id: Some(node),
            sequence: Some(sequence),
            status: match status {
                "Completed" => "Completed",
                "Stopped" => "Stopped",
                _ => "Failed",
            },
        }
    }
    fn terminal(root: &Path, run: uuid::Uuid, status: crate::execution::RunStatus) -> Option<Self> {
        use crate::execution::RunStatus;
        let status = match status {
            RunStatus::Completed => "Completed",
            RunStatus::Cancelled => "Cancelled",
            RunStatus::Failed => "Failed",
            _ => return None,
        };
        Some(Self {
            event: "run.terminal",
            event_id: format!("{run}:terminal"),
            workspace: scope_hash(root),
            run_id: Some(run),
            node_id: None,
            sequence: None,
            status,
        })
    }
}
fn scope_hash(root: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(root.to_string_lossy().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) struct Binding {
    package: Arc<Package>,
    function: String,
    /// 0 retired, 1 active, 2 closing (only the committed close may run).
    active: AtomicU8,
    report: Mutex<Report>,
    bus: Weak<Bus>,
}
impl Binding {
    fn allows(&self, event: &Event) -> bool {
        matches!(self.active.load(Ordering::SeqCst), 1)
            || (self.active.load(Ordering::SeqCst) == 2 && event.event == "workspace.close")
    }
    pub(crate) fn submit(self: &Arc<Self>, event: &Event) {
        if self.report.lock().event != event.event || !self.allows(event) {
            return;
        }
        if let Some(bus) = self.bus.upgrade() {
            bus.enqueue(self.clone(), event.clone());
        }
    }
    fn failed(&self, id: &str, message: &str) {
        let mut report = self.report.lock();
        report.event_id = id.into();
        report.status = "Failed".into();
        report.failed = report.failed.saturating_add(1);
        report.error = message.into();
    }
}
struct Work {
    binding: Arc<Binding>,
    event: Event,
    input: String,
}
struct Historical {
    owner: String,
    fingerprint: String,
    report: Report,
}
#[derive(Default)]
pub(crate) struct Bus {
    bindings: Mutex<BTreeMap<String, Arc<Binding>>>,
    sender: Mutex<Option<tokio::sync::mpsc::Sender<Work>>>,
    history: Arc<Mutex<VecDeque<Historical>>>,
}
impl Drop for Bus {
    fn drop(&mut self) {
        for binding in self.bindings.get_mut().values() {
            binding.active.store(0, Ordering::SeqCst);
        }
    }
}
impl Bus {
    pub(super) fn bind(
        self: &Arc<Self>,
        package: Arc<Package>,
        root: Option<&Path>,
    ) -> Vec<Arc<Binding>> {
        let Some(root) = root else {
            return vec![];
        };
        let mut bindings = self.bindings.lock();
        package
            .manifest
            .hooks
            .iter()
            .map(|hook| {
                let key = format!(
                    "{}|{}|{}",
                    root.to_string_lossy(),
                    package.identity.owner(),
                    hook.name
                );
                if let Some(binding) = bindings.get(&key)
                    && binding.package.identity == package.identity
                    && binding.active.load(Ordering::SeqCst) == 1
                {
                    return binding.clone();
                }
                if let Some(old) = bindings.remove(&key) {
                    old.active.store(0, Ordering::SeqCst);
                }
                let binding = Arc::new(Binding {
                    package: package.clone(),
                    function: hook.function.clone(),
                    active: AtomicU8::new(1),
                    report: Mutex::new(Report {
                        name: hook.name.clone(),
                        event: hook.event.clone(),
                        scope_root: root.to_string_lossy().into_owned(),
                        status: "Ready".into(),
                        ..Default::default()
                    }),
                    bus: Arc::downgrade(self),
                });
                bindings.insert(key, binding.clone());
                binding
            })
            .collect()
    }
    pub(super) fn prune(&self, identities: &BTreeMap<String, String>) {
        self.bindings.lock().retain(|_, binding| {
            let state = binding.active.load(Ordering::SeqCst);
            let keep = state == 2
                || (identities.get(&binding.package.identity.owner())
                    == Some(&binding.package.identity.fingerprint)
                    && state != 0);
            if !keep {
                binding.active.store(0, Ordering::SeqCst);
            }
            keep
        });
    }
    pub(super) fn retire_owner(&self, owner: &str) {
        for binding in self.bindings.lock().values().filter(|b| b.package.identity.owner() == owner)
        {
            binding.active.store(0, Ordering::SeqCst);
        }
    }
    pub(super) fn retire(&self, root: Option<&Path>, owner: &str) {
        let scope = root.map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
        for binding in
            self.bindings.lock().values().filter(|b| {
                b.package.identity.owner() == owner && b.report.lock().scope_root == scope
            })
        {
            binding.active.store(0, Ordering::SeqCst);
        }
    }
    pub(super) fn workspace(&self, root: &Path, open: bool) {
        let scope = root.to_string_lossy();
        let selected: Vec<_> = self
            .bindings
            .lock()
            .values()
            .filter(|b| b.report.lock().scope_root == scope && b.active.load(Ordering::SeqCst) == 1)
            .cloned()
            .collect();
        let event = Event::workspace(root, open);
        for binding in selected {
            if !open {
                binding.active.store(2, Ordering::SeqCst);
            }
            binding.submit(&event);
            if !open && binding.report.lock().event != "workspace.close" {
                binding.active.store(0, Ordering::SeqCst);
            }
        }
    }
    pub(super) fn reports(&self, owner: &str, fingerprint: &str, roots: &[PathBuf]) -> Vec<Report> {
        let relevant = |report: &Report| {
            roots.is_empty() || roots.iter().any(|r| r.to_string_lossy() == report.scope_root)
        };
        let mut reports: BTreeMap<(String, String), Report> = BTreeMap::new();
        for item in self
            .history
            .lock()
            .iter()
            .filter(|h| h.owner == owner && h.fingerprint == fingerprint && relevant(&h.report))
        {
            reports.insert(
                (item.report.scope_root.clone(), item.report.name.clone()),
                item.report.clone(),
            );
        }
        for binding in self.bindings.lock().values().filter(|b| {
            b.package.identity.owner() == owner && b.package.identity.fingerprint == fingerprint
        }) {
            let report = binding.report.lock().clone();
            if relevant(&report) {
                reports.insert((report.scope_root.clone(), report.name.clone()), report);
            }
        }
        reports.into_values().collect()
    }
    fn enqueue(&self, binding: Arc<Binding>, event: Event) {
        let input = serde_json::to_string(&event).unwrap_or_default();
        if input.len() > 4096 {
            binding.failed(&event.event_id, "Hook event payload limit exceeded");
            return;
        }
        let mut sender = self.sender.lock();
        if sender.is_none() {
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                binding.failed(&event.event_id, "Hook worker unavailable");
                return;
            };
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Work>(QUEUE_LIMIT);
            let history = self.history.clone();
            runtime.spawn(async move {
                while let Some(work) = rx.recv().await {
                    if !work.binding.allows(&work.event) {
                        continue;
                    }
                    {
                        let mut report = work.binding.report.lock();
                        if report.event_id == work.event.event_id {
                            report.status = "Running".into();
                        }
                    }
                    let package = work.binding.package.clone();
                    let function = work.binding.function.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        super::runtime::observe(
                            &package.wasm,
                            &function,
                            &work.input,
                            package.manifest.call_timeout_ms(1000),
                        )
                    })
                    .await;
                    if !work.binding.allows(&work.event) {
                        continue;
                    }
                    let report = {
                        let mut report = work.binding.report.lock();
                        let success = matches!(result, Ok(Ok(())));
                        if success {
                            report.completed = report.completed.saturating_add(1);
                        } else {
                            report.failed = report.failed.saturating_add(1);
                        }
                        if report.event_id == work.event.event_id {
                            report.status = if success {
                                "Succeeded"
                            } else {
                                "Failed"
                            }
                            .into();
                            report.error = if success {
                                String::new()
                            } else {
                                "Hook callback failed, exceeded its limits or timed out".into()
                            };
                        }
                        report.clone()
                    };
                    if work.event.event == "workspace.close" {
                        work.binding.active.store(0, Ordering::SeqCst);
                        remember(&history, &work.binding, report);
                    }
                }
            });
            *sender = Some(tx);
        }
        {
            let mut report = binding.report.lock();
            report.event_id = event.event_id.clone();
            report.status = "Queued".into();
            report.error.clear();
        }
        if sender
            .as_ref()
            .expect("worker sender")
            .try_send(Work {
                binding: binding.clone(),
                event: event.clone(),
                input,
            })
            .is_err()
        {
            binding.failed(
                &event.event_id,
                "Hook queue is full or unavailable; event was not delivered",
            );
            if event.event == "workspace.close" {
                binding.active.store(0, Ordering::SeqCst);
                remember(&self.history, &binding, binding.report.lock().clone());
            }
        }
    }
}
fn remember(history: &Mutex<VecDeque<Historical>>, binding: &Binding, report: Report) {
    let mut history = history.lock();
    if history.len() == HISTORY_LIMIT {
        history.pop_front();
    }
    history.push_back(Historical {
        owner: binding.package.identity.owner(),
        fingerprint: binding.package.identity.fingerprint.clone(),
        report,
    });
}

/// Per-interpreter cursor, seeded from recovery facts before the next commit.
/// Parent invocations may finish after their children, so sequence maxima alone
/// cannot deduplicate completion notifications.
#[derive(Default)]
pub(crate) struct Cursor {
    finished: BTreeSet<u64>,
    exhausted: bool,
    limit_reported: bool,
    terminal: bool,
}
impl Cursor {
    pub(crate) fn seed(&mut self, view: &crate::execution::view::ExecutionView) {
        self.finished = view
            .invocations
            .iter()
            .filter(|v| v.finished_at.is_some())
            .map(|v| v.sequence)
            .take(RUN_EVENT_LIMIT)
            .collect();
        self.exhausted = self.finished.len() >= RUN_EVENT_LIMIT;
        self.limit_reported = false;
        self.terminal = false;
    }
    pub(crate) fn committed(
        &mut self,
        bindings: &[Arc<Binding>],
        view: &crate::execution::view::ExecutionView,
        root: &Path,
        run: uuid::Uuid,
        status: crate::execution::RunStatus,
    ) {
        if bindings.is_empty() {
            return;
        }
        if !self.exhausted {
            for invocation in view.invocations.iter().filter(|v| v.finished_at.is_some()) {
                if self.finished.contains(&invocation.sequence) {
                    continue;
                }
                if self.finished.len() >= RUN_EVENT_LIMIT {
                    self.exhausted = true;
                    break;
                }
                self.finished.insert(invocation.sequence);
                let event = Event::node(
                    root,
                    run,
                    invocation.node_id,
                    invocation.sequence,
                    &invocation.status,
                );
                for binding in bindings {
                    binding.submit(&event);
                }
            }
        }
        if self.exhausted && !self.limit_reported {
            self.limit_reported = true;
            for binding in bindings.iter().filter(|b| b.report.lock().event == "node.finished") {
                binding.failed(
                    &format!("{run}:limit"),
                    "Hook node event budget exhausted; further node notifications are omitted",
                );
            }
        }
        if !self.terminal
            && let Some(event) = Event::terminal(root, run, status)
        {
            self.terminal = true;
            for binding in bindings {
                binding.submit(&event);
            }
        }
    }
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
