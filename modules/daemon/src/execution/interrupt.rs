//! Interrupts for injecting temporary messages into a running execution.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// The priority of an interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPriority {
    /// Queued normally, injected at the next Call LLM node.
    Normal,
    /// Injected before the next LLM call (e.g. between tool calls).
    Urgent,
    /// Immediately aborts the current LLM call and forces a message to the LLM.
    Emergency,
}

/// A temporary message injected into a running execution.
#[derive(Debug, Clone)]
pub struct Interrupt {
    /// The priority of the interrupt.
    pub priority: InterruptPriority,
    /// The message content.
    pub message: String,
}

/// A durable input observer, owned by one foreground turn. Receiving input
/// and releasing it into model context are different durable facts.
pub trait OwnedInterrupt: Send + Sync {
    fn claim(&self) -> crate::error::DaemonResult<()>;
    fn unconsumed(&self) -> crate::error::DaemonResult<()>;
    fn applied(&self, context_persisted: bool) -> crate::error::DaemonResult<()>;
}
struct Entry {
    interrupt: Interrupt,
    owner: Option<Arc<dyn OwnedInterrupt>>,
}
#[derive(Default)]
struct Queue {
    pending: VecDeque<Entry>,
    claimed: Vec<(Option<uuid::Uuid>, Arc<dyn OwnedInterrupt>)>,
    sealed: bool,
}
/// The existing bus also keeps optional durable input ownership. Nested
/// consumers cannot drain an owned parent input through the legacy methods.
#[derive(Clone)]
pub struct InterruptBus {
    inner: Arc<Mutex<Queue>>,
    notify: Arc<Notify>,
}
impl InterruptBus {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Queue::default())),
            notify: Arc::new(Notify::new()),
        }
    }
    /// Internal best-effort signals accompany independent cancel/pause flags.
    /// User input RPCs use try_send or send_owned and surface refusal.
    pub fn send(&self, interrupt: Interrupt) {
        let _ = self.try_send(interrupt);
    }
    pub fn try_send(&self, interrupt: Interrupt) -> crate::error::DaemonResult<()> {
        let mut queue = self.inner.lock().unwrap();
        Self::available(&queue)?;
        queue.pending.push_back(Entry {
            interrupt,
            owner: None,
        });
        self.notify.notify_one();
        Ok(())
    }
    fn available(queue: &Queue) -> crate::error::DaemonResult<()> {
        if queue.sealed {
            return Err(crate::DaemonError::Execution(
                "Input target has ended; retain the draft".into(),
            ));
        }
        if queue.pending.len() >= 256 {
            return Err(crate::DaemonError::Execution(
                "Input queue is full; retain the draft".into(),
            ));
        }
        Ok(())
    }
    /// Admission of the body and its queue entry is fenced against seal().
    /// The persistence callback must finish its WAL before returning success.
    pub fn send_owned<T>(
        &self,
        interrupt: Interrupt,
        owner: Arc<dyn OwnedInterrupt>,
        persist: impl FnOnce() -> crate::error::DaemonResult<(T, bool)>,
    ) -> crate::error::DaemonResult<T> {
        if interrupt.priority == InterruptPriority::Emergency {
            return Err(crate::DaemonError::Execution(
                "Reliable input is Queue or Steer; use direct Stop for emergency control".into(),
            ));
        }
        let mut queue = self.inner.lock().unwrap();
        Self::available(&queue)?;
        if queue.pending.iter().filter(|entry| entry.owner.is_some()).count() >= 64 {
            return Err(crate::DaemonError::Execution(
                "Reliable input queue is full; retain the draft".into(),
            ));
        }
        let (receipt, newly_received) = persist()?;
        if !newly_received {
            return Ok(receipt);
        }
        queue.pending.push_back(Entry {
            interrupt,
            owner: Some(owner),
        });
        self.notify.notify_one();
        Ok(receipt)
    }
    pub fn drain(&self, priority: InterruptPriority) -> Vec<String> {
        let mut queue = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut index = 0;
        while index < queue.pending.len() {
            if queue.pending[index].owner.is_none()
                && queue.pending[index].interrupt.priority == priority
            {
                out.push(queue.pending.remove(index).unwrap().interrupt.message);
            } else {
                index += 1;
            }
        }
        out
    }
    /// Only the owning root loop calls this method. A claim failure prevents
    /// that input from entering context or being reported as consumed.
    pub fn drain_owned(
        &self,
        priority: InterruptPriority,
    ) -> Result<Vec<String>, (crate::DaemonError, Vec<String>)> {
        self.drain_owned_for(priority, None)
    }
    pub fn drain_owned_for(
        &self,
        priority: InterruptPriority,
        consumer: Option<uuid::Uuid>,
    ) -> Result<Vec<String>, (crate::DaemonError, Vec<String>)> {
        let mut queue = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut index = 0;
        while index < queue.pending.len() {
            if queue.pending[index].owner.is_some()
                && queue.pending[index].interrupt.priority == priority
            {
                let owner = queue.pending[index].owner.as_ref().unwrap().clone();
                if let Err(error) = owner.claim() {
                    return Err((error, out));
                }
                let entry = queue.pending.remove(index).unwrap();
                queue.claimed.push((consumer, owner));
                out.push(entry.interrupt.message);
            } else {
                index += 1;
            }
        }
        Ok(out)
    }
    /// Legacy drains leave all owned input for the foreground loop.
    pub fn drain_all(&self) -> Vec<String> {
        let mut queue = self.inner.lock().unwrap();
        let mut out = Vec::new();
        let mut index = 0;
        while index < queue.pending.len() {
            if queue.pending[index].owner.is_none() {
                out.push(queue.pending.remove(index).unwrap().interrupt.message);
            } else {
                index += 1;
            }
        }
        out
    }
    /// No input can be admitted after this boundary, including while the
    /// caller persists the root context. A failure retains the durable body.
    pub fn seal(&self) -> crate::error::DaemonResult<()> {
        let mut queue = self.inner.lock().unwrap();
        queue.sealed = true;
        let pending = std::mem::take(&mut queue.pending);
        let mut error = None;
        for entry in pending {
            if let Some(owner) = entry.owner
                && let Err(reason) = owner.unconsumed()
            {
                error.get_or_insert(reason);
            }
        }
        error.map_or(Ok(()), Err)
    }
    pub fn finish_owned(&self, context_persisted: bool) -> crate::error::DaemonResult<()> {
        self.finish_consumer(None, context_persisted)
    }
    /// A blueprint receipt is applied only after its consuming node commits.
    /// None finishes all remaining claims at root chat/run termination.
    pub fn finish_consumer(
        &self,
        consumer: Option<uuid::Uuid>,
        context_persisted: bool,
    ) -> crate::error::DaemonResult<()> {
        let mut queue = self.inner.lock().unwrap();
        let claimed = std::mem::take(&mut queue.claimed);
        let mut error = None;
        for (node, owner) in claimed {
            if consumer.is_some() && node != consumer {
                queue.claimed.push((node, owner));
                continue;
            }
            if let Err(reason) = owner.applied(context_persisted) {
                error.get_or_insert(reason);
            }
        }
        error.map_or(Ok(()), Err)
    }
    pub async fn wait_emergency(&self) -> Option<String> {
        loop {
            let mut msgs = self.drain(InterruptPriority::Emergency);
            if let Some(msg) = msgs.pop() {
                return Some(msg);
            }
            self.notify.notified().await;
        }
    }
}
impl Default for InterruptBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[derive(Default)]
    struct Observer {
        claims: AtomicUsize,
        unused: AtomicUsize,
        applied: AtomicUsize,
        fail: AtomicBool,
    }
    impl OwnedInterrupt for Observer {
        fn claim(&self) -> crate::error::DaemonResult<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(crate::DaemonError::Persistence("injected claim failure".into()));
            }
            self.claims.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn unconsumed(&self) -> crate::error::DaemonResult<()> {
            self.unused.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn applied(&self, persisted: bool) -> crate::error::DaemonResult<()> {
            if persisted {
                self.applied.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }
    fn input() -> Interrupt {
        Interrupt {
            priority: InterruptPriority::Normal,
            message: "parent input".into(),
        }
    }
    #[test]
    fn nested_legacy_drains_cannot_consume_owned_parent_input() {
        let bus = InterruptBus::new();
        let owner = Arc::new(Observer::default());
        bus.send_owned(input(), owner.clone(), || Ok(((), true))).unwrap();
        assert!(bus.drain(InterruptPriority::Normal).is_empty());
        assert!(bus.drain_all().is_empty());
        assert_eq!(bus.drain_owned(InterruptPriority::Normal).unwrap(), vec!["parent input"]);
        assert!(bus.drain_owned(InterruptPriority::Normal).unwrap().is_empty());
        bus.seal().unwrap();
        bus.finish_owned(true).unwrap();
        assert_eq!(owner.claims.load(Ordering::SeqCst), 1);
        assert_eq!(owner.unused.load(Ordering::SeqCst), 0);
        assert_eq!(owner.applied.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn failed_backup_or_claim_never_admits_input_to_context() {
        let bus = InterruptBus::new();
        let owner = Arc::new(Observer::default());
        assert!(
            bus.send_owned(input(), owner.clone(), || Err::<((), bool), _>(
                crate::DaemonError::Persistence("disk full".into())
            ))
            .is_err()
        );
        assert!(bus.drain_owned(InterruptPriority::Normal).unwrap().is_empty());
        bus.send_owned(input(), owner.clone(), || Ok(((), true))).unwrap();
        owner.fail.store(true, Ordering::SeqCst);
        assert!(bus.drain_owned(InterruptPriority::Normal).is_err());
        bus.seal().unwrap();
        assert_eq!(owner.unused.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn turn_end_fences_a_late_acceptance_before_its_persistence_callback() {
        let bus = InterruptBus::new();
        let owner = Arc::new(Observer::default());
        bus.send_owned(input(), owner.clone(), || Ok(((), true))).unwrap();
        bus.seal().unwrap();
        assert!(
            bus.send_owned(input(), owner.clone(), || -> crate::error::DaemonResult<((), bool)> {
                panic!("a late input must not be persisted as received")
            })
            .is_err()
        );
        assert!(bus.try_send(input()).is_err());
        assert_eq!(owner.unused.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn concurrent_turn_end_waits_for_accepted_input_and_marks_it_unconsumed() {
        let bus = InterruptBus::new();
        let owner = Arc::new(Observer::default());
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let accepting = bus.clone();
        let observer = owner.clone();
        let send = std::thread::spawn(move || {
            accepting.send_owned(input(), observer, || {
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok((17, true))
            })
        });
        entered_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        let closing = bus.clone();
        let finish = std::thread::spawn(move || closing.seal());
        release.send(()).unwrap();
        assert_eq!(send.join().unwrap().unwrap(), 17);
        finish.join().unwrap().unwrap();
        assert_eq!(owner.unused.load(Ordering::SeqCst), 1);
        assert!(bus.drain_owned(InterruptPriority::Normal).unwrap().is_empty());
    }
    #[test]
    fn later_claim_failure_returns_already_claimed_messages_for_context_preservation() {
        let bus = InterruptBus::new();
        let first = Arc::new(Observer::default());
        let second = Arc::new(Observer::default());
        bus.send_owned(input(), first.clone(), || Ok(((), true))).unwrap();
        bus.send_owned(input(), second.clone(), || Ok(((), true))).unwrap();
        second.fail.store(true, Ordering::SeqCst);
        let (error, claimed) = bus.drain_owned(InterruptPriority::Normal).unwrap_err();
        assert!(error.to_string().contains("claim failure"));
        assert_eq!(claimed, vec!["parent input"]);
        bus.seal().unwrap();
        bus.finish_owned(true).unwrap();
        assert_eq!(first.applied.load(Ordering::SeqCst), 1);
        assert_eq!(second.unused.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn a_replayed_persistence_receipt_does_not_enqueue_or_claim_again() {
        let bus = InterruptBus::new();
        let owner = Arc::new(Observer::default());
        assert_eq!(bus.send_owned(input(), owner.clone(), || Ok((17, true))).unwrap(), 17);
        assert_eq!(bus.send_owned(input(), owner.clone(), || Ok((17, false))).unwrap(), 17);
        assert_eq!(bus.drain_owned(InterruptPriority::Normal).unwrap(), vec!["parent input"]);
        assert_eq!(owner.claims.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn blueprint_consumption_finishes_only_after_its_own_node_commits() {
        let bus = InterruptBus::new();
        let first = Arc::new(Observer::default());
        let second = Arc::new(Observer::default());
        let node = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        bus.send_owned(input(), first.clone(), || Ok(((), true))).unwrap();
        assert_eq!(bus.drain_owned_for(InterruptPriority::Normal, Some(node)).unwrap().len(), 1);
        bus.send_owned(input(), second.clone(), || Ok(((), true))).unwrap();
        bus.drain_owned_for(InterruptPriority::Normal, Some(other)).unwrap();
        bus.finish_consumer(Some(node), true).unwrap();
        assert_eq!(first.applied.load(Ordering::SeqCst), 1);
        assert_eq!(second.applied.load(Ordering::SeqCst), 0);
        bus.finish_owned(false).unwrap();
        assert_eq!(second.applied.load(Ordering::SeqCst), 0);
    }
}
