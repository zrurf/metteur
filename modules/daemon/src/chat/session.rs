//! Persisted chat session storage.
//!
//! Sessions live in the workspace RocksDB under the `chat_threads` column
//! family, keyed by session id. The legacy single `active` slot in
//! `chat_sessions` is migrated on first access and then removed. The full
//! [`ContextManager`] is persisted so a conversation can resume after a
//! restart with its tool results intact.

use metteur_shared::llm::{ContextManager, Role};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::transcript::TranscriptEntry;
use crate::error::{DaemonError, DaemonResult};
use crate::storage::persistence::{Db, cf};

/// The DB key of the legacy active session record.
const ACTIVE_KEY: &[u8] = b"active";

/// Truncation length for the session title.
const TITLE_MAX_CHARS: usize = 80;

/// Maximum retained threads per workspace; older ones are evicted.
pub const MAX_THREADS: usize = 20;

/// A persisted chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionRecord {
    /// The session identifier.
    pub session_id: Uuid,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Last write time in milliseconds since the Unix epoch.
    pub updated_at: u64,
    /// Number of completed turns (user messages with a finished reply).
    pub turns: u64,
    /// First user message, truncated, for list display.
    pub title: Option<String>,
    /// The conversation context, including retained tool results.
    pub context: ContextManager,
    /// The agent's task list at the end of the last turn.
    #[serde(default)]
    pub todos: Vec<metteur_shared::llm::TodoItem>,
    /// What the conversation looked like: every turn, tool call and notice.
    ///
    /// Kept apart from `context`, which is the model's (compressed, evicted)
    /// view and cannot reproduce the transcript. Records written before this
    /// field existed deserialize as empty, and readers fall back to the
    /// context's user/assistant turns.
    #[serde(default)]
    pub transcript: Vec<TranscriptEntry>,
}

impl ChatSessionRecord {
    /// Creates a record for a fresh session started at `now`.
    pub fn new(now: u64, context: ContextManager) -> Self {
        Self {
            session_id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            turns: 0,
            title: title_of(&context),
            context,
            todos: Vec::new(),
            transcript: Vec::new(),
        }
    }
}

/// Loads the workspace's active session, if any.
///
/// Migrates the legacy single slot into the threads family on first access.
pub fn load_active(db: &Db) -> DaemonResult<Option<ChatSessionRecord>> {
    migrate_legacy(db)?;
    let mut threads = list_threads(db)?;
    threads.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    Ok(threads.into_iter().next())
}

/// Overwrites the workspace's active session.
///
/// Preserved for compatibility: writes into the threads family under the
/// record's own id (the newest thread reads back as active).
pub fn save_active(db: &Db, record: &ChatSessionRecord) -> DaemonResult<()> {
    save_thread(db, record)
}

/// Removes the workspace's active session (idempotent).
pub fn delete_active(db: &Db) -> DaemonResult<()> {
    migrate_legacy(db)?;
    if let Some(active) = load_active(db)? {
        delete_thread(db, &active.session_id)?;
    }
    Ok(())
}

/// Loads one thread by session id.
pub fn load_thread(db: &Db, session_id: &Uuid) -> DaemonResult<Option<ChatSessionRecord>> {
    migrate_legacy(db)?;
    let Some(data) = db.get(cf::CHAT_THREADS, session_id.as_bytes())? else {
        return Ok(None);
    };
    let mut record: ChatSessionRecord =
        serde_json::from_slice(&data).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    // Sessions written by an older build can carry an invalid tool-call
    // sequence that a provider rejects on the next request; repair it before
    // the conversation is resumed (the next save persists the fix).
    let repaired = record.context.repair_tool_pairing();
    if repaired > 0 {
        tracing::warn!("chat session {session_id}: repaired {repaired} out-of-order message(s)");
    }
    Ok(Some(record))
}

/// Saves one thread, evicting the oldest threads past [`MAX_THREADS`].
pub fn save_thread(db: &Db, record: &ChatSessionRecord) -> DaemonResult<()> {
    migrate_legacy(db)?;
    let data = serde_json::to_vec(record).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    db.save_chat_revision(record.session_id.as_bytes(), &data)?;
    // Evict oldest past the cap; the id tie-break keeps eviction deterministic
    // when several threads share the same `updated_at` millisecond.
    let mut threads = list_threads(db)?;
    threads.sort_by_key(|r| (r.updated_at, r.session_id));
    if threads.len() > MAX_THREADS {
        for stale in threads.iter().take(threads.len() - MAX_THREADS) {
            delete_thread(db, &stale.session_id)?;
        }
    }
    Ok(())
}

/// Removes one thread by session id (idempotent).
pub fn delete_thread(db: &Db, session_id: &Uuid) -> DaemonResult<()> {
    super::checkpoint::delete_for_session(db, *session_id)?;
    db.delete(cf::CHAT_THREADS, session_id.as_bytes())
}

/// Lists all threads, oldest first.
pub fn list_threads(db: &Db) -> DaemonResult<Vec<ChatSessionRecord>> {
    migrate_legacy(db)?;
    let mut out = Vec::new();
    for (_, value) in db.scan(cf::CHAT_THREADS)? {
        let record: ChatSessionRecord = serde_json::from_slice(&value)
            .map_err(|e| DaemonError::Serialization(e.to_string()))?;
        out.push(record);
    }
    out.sort_by_key(|r| r.updated_at);
    Ok(out)
}

/// Moves the legacy `active` record into the threads family once.
fn migrate_legacy(db: &Db) -> DaemonResult<()> {
    let Some(data) = db.get(cf::CHAT_SESSIONS, ACTIVE_KEY)? else {
        return Ok(());
    };
    let record: ChatSessionRecord =
        serde_json::from_slice(&data).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    let fresh =
        serde_json::to_vec(&record).map_err(|e| DaemonError::Serialization(e.to_string()))?;
    db.put(cf::CHAT_THREADS, record.session_id.as_bytes(), &fresh)?;
    db.delete(cf::CHAT_SESSIONS, ACTIVE_KEY)
}

/// Counts user and assistant messages (system/tool messages excluded).
pub fn display_message_count(record: &ChatSessionRecord) -> usize {
    if !record.transcript.is_empty() {
        return record
            .transcript
            .iter()
            .filter(|entry| {
                matches!(
                    entry.role,
                    super::transcript::EntryRole::User | super::transcript::EntryRole::Assistant
                )
            })
            .count();
    }
    record
        .context
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .count()
}

/// Drops the duplicated opening turn written before the send path was fixed.
///
/// A new session used to be seeded from the client's history *and* from the
/// message field, so its first user turn appeared twice — in the transcript and
/// in every restored view. The duplicate is only repaired when it is the very
/// first pair of messages, which cannot be a legitimate repetition.
pub fn repair_duplicate_opening(context: &mut ContextManager) -> usize {
    let mut removed = 0;
    while context.messages.len() >= 2 {
        let (first, second) = (&context.messages[0], &context.messages[1]);
        let same = first.role == Role::User
            && second.role == Role::User
            && first.text_content() == second.text_content()
            && !first.text_content().trim().is_empty();
        if !same {
            break;
        }
        context.messages.remove(1);
        removed += 1;
    }
    removed
}

/// Builds a truncated title from the first user message.
pub(crate) fn title_of(context: &ContextManager) -> Option<String> {
    let text = context.messages.iter().find(|m| m.role == Role::User).map(|m| m.text_content())?;
    Some(text.chars().take(TITLE_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::llm::Message;

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("metteur-chat-{}", Uuid::new_v4()));
        Db::open(&dir).unwrap()
    }

    fn context_with(text: &str) -> ContextManager {
        let mut ctx = ContextManager::default();
        ctx.push_message(Message::text(Role::User, text));
        ctx.push_message(Message::text(Role::Assistant, "ok"));
        ctx
    }

    #[test]
    fn save_load_roundtrip() {
        let db = db();
        assert!(load_active(&db).unwrap().is_none());
        let record = ChatSessionRecord::new(1000, context_with("hello"));
        save_active(&db, &record).unwrap();
        let loaded = load_active(&db).unwrap().unwrap();
        assert_eq!(loaded.session_id, record.session_id);
        assert_eq!(loaded.context.messages.len(), 2);
        assert_eq!(loaded.title.as_deref(), Some("hello"));
        assert_eq!(display_message_count(&loaded), 2);
    }

    #[test]
    fn delete_is_idempotent() {
        let db = db();
        delete_active(&db).unwrap();
        let record = ChatSessionRecord::new(1000, context_with("hi"));
        save_active(&db, &record).unwrap();
        delete_active(&db).unwrap();
        assert!(load_active(&db).unwrap().is_none());
    }

    #[test]
    fn threads_list_and_delete_by_id() {
        let db = db();
        let first = ChatSessionRecord::new(1000, context_with("first"));
        let second = ChatSessionRecord::new(2000, context_with("second"));
        save_thread(&db, &first).unwrap();
        save_thread(&db, &second).unwrap();
        let threads = list_threads(&db).unwrap();
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].session_id, first.session_id);
        assert_eq!(threads[1].session_id, second.session_id);
        // The newest thread reads back as active.
        assert_eq!(load_active(&db).unwrap().unwrap().session_id, second.session_id);
        delete_thread(&db, &first.session_id).unwrap();
        assert_eq!(list_threads(&db).unwrap().len(), 1);
        assert!(load_thread(&db, &first.session_id).unwrap().is_none());
        // Deleting a missing thread is idempotent.
        delete_thread(&db, &first.session_id).unwrap();
    }

    #[test]
    fn legacy_active_slot_migrates_once() {
        let db = db();
        let record = ChatSessionRecord::new(1000, context_with("legacy"));
        let data = serde_json::to_vec(&record).unwrap();
        db.put(cf::CHAT_SESSIONS, ACTIVE_KEY, &data).unwrap();
        // First access migrates; the legacy slot is removed.
        let migrated = load_thread(&db, &record.session_id).unwrap().unwrap();
        assert_eq!(migrated.title, record.title);
        assert!(db.get(cf::CHAT_SESSIONS, ACTIVE_KEY).unwrap().is_none());
        assert_eq!(list_threads(&db).unwrap().len(), 1);
    }

    #[test]
    fn thread_count_is_capped() {
        let db = db();
        for i in 0..(MAX_THREADS + 5) {
            let record = ChatSessionRecord::new(1000 + i as u64, context_with("filler"));
            save_thread(&db, &record).unwrap();
        }
        assert_eq!(list_threads(&db).unwrap().len(), MAX_THREADS);
    }

    #[test]
    fn title_is_truncated() {
        let long = "x".repeat(TITLE_MAX_CHARS + 50);
        let ctx = context_with(&long);
        let title = title_of(&ctx).unwrap();
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS);
    }
}
