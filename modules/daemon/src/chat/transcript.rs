//! The display transcript of a chat session.
//!
//! The model's context and the user's transcript are different things. The
//! context is compressed, evicted and rewritten with markers as a run proceeds,
//! and it drops everything that is not a user/assistant message because a
//! provider only accepts a narrow shape. A transcript rebuilt from it therefore
//! loses tool calls entirely — which is exactly what happened before this
//! module existed: switching sessions showed a conversation with no tools in
//! it.
//!
//! So the daemon records what the *conversation looked like* alongside the
//! context: every tool call with its subject, duration and outcome, notices,
//! failures, and the assistant's reasoning. [`Transcript`] accumulates those
//! entries while a turn streams, and the record persists them.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Maximum transcript entries kept per session.
///
/// Oldest entries are dropped first, which bounds a long-lived session without
/// touching the model's context.
pub const MAX_ENTRIES: usize = 400;

/// Maximum characters kept per tool result in the transcript.
///
/// The expanded view of a restored tool call shows this much; the live view
/// shows everything. Keeping full outputs here would duplicate the context in
/// the database for a view nobody reads in full.
pub const MAX_TOOL_CHARS: usize = 8 * 1024;

/// The role of a transcript entry, as rendered by a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryRole {
    /// A turn written by the user.
    User,
    /// An assistant answer, possibly with the reasoning that preceded it.
    Assistant,
    /// One tool call.
    Tool,
    /// An engine notice (retry, job progress, context release).
    Notice,
    /// A failed turn.
    Error,
}

/// One entry of the display transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Who or what produced this entry.
    pub role: EntryRole,
    /// Creation time in milliseconds since the Unix epoch.
    pub at: u64,
    /// Rendered text (the answer, the user's turn, the tool's output).
    #[serde(default)]
    pub content: String,
    /// Pre-turn file/model checkpoint. Only user entries carry this id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub checkpoint: String,
    /// Reasoning that preceded an assistant answer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    /// Time from first reasoning through the complete answer, including tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_elapsed_ms: Option<u64>,
    /// Whole request duration, distinct from legacy reasoning-only timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_elapsed_ms: Option<u64>,
    /// Tool name, for `Tool` entries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    /// Provider call id, which pairs a start with its result.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub call_id: String,
    /// One-line subject of the call (the same summary the live row shows).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Whether the call succeeded; meaningful for `Tool` entries.
    #[serde(default)]
    pub ok: bool,
    /// Wall-clock duration of the call in milliseconds.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// Host task identity for clients following a yielded conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_task: Option<uuid::Uuid>,
}

impl TranscriptEntry {
    fn new(role: EntryRole, at: u64) -> Self {
        Self {
            role,
            at,
            content: String::new(),
            checkpoint: String::new(),
            reasoning: String::new(),
            reasoning_elapsed_ms: None,
            turn_elapsed_ms: None,
            tool: String::new(),
            call_id: String::new(),
            summary: String::new(),
            ok: true,
            elapsed_ms: 0,
            continuation_task: None,
        }
    }
}

/// Accumulates the transcript of one turn.
///
/// A tool call is recorded twice: when it starts (so an interrupted turn still
/// shows what it was doing) and when it returns, which updates the same entry.
#[derive(Debug, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    /// Previous transcript of the session, carried through the turn.
    history: Vec<TranscriptEntry>,
    reasoning_started: Option<Instant>,
    reasoning_elapsed: Option<Duration>,
}

impl Transcript {
    /// Continues the transcript of an existing session.
    pub fn resuming(history: Vec<TranscriptEntry>) -> Self {
        Self {
            entries: Vec::new(),
            history,
            ..Self::default()
        }
    }

    /// Starts a reasoning segment without resetting earlier segments.
    pub fn start_reasoning(&mut self) {
        self.reasoning_started.get_or_insert_with(Instant::now);
    }

    /// Freezes the measured time once the complete answer has arrived.
    pub fn finish_reasoning(&mut self) -> Option<u64> {
        if let Some(started) = self.reasoning_started.take() {
            *self.reasoning_elapsed.get_or_insert(Duration::ZERO) += started.elapsed();
        }
        self.reasoning_elapsed.map(|elapsed| elapsed.as_millis() as u64)
    }

    /// Records the user's turn.
    pub fn push_user(&mut self, text: impl Into<String>, at: u64) {
        let mut entry = TranscriptEntry::new(EntryRole::User, at);
        entry.content = text.into();
        self.entries.push(entry);
    }

    /// Links the latest user turn to its paired file/model checkpoint.
    pub fn set_user_checkpoint(&mut self, checkpoint: &str) {
        if let Some(entry) = self.entries.last_mut().filter(|e| e.role == EntryRole::User) {
            entry.checkpoint = checkpoint.to_string();
        }
    }

    /// Records a finished assistant answer.
    pub fn push_assistant(
        &mut self,
        text: impl Into<String>,
        reasoning: impl Into<String>,
        at: u64,
    ) {
        let mut entry = TranscriptEntry::new(EntryRole::Assistant, at);
        entry.content = text.into();
        entry.reasoning = reasoning.into();
        if !entry.reasoning.is_empty() {
            entry.reasoning_elapsed_ms = self.finish_reasoning();
            self.reasoning_elapsed = None;
        }
        self.entries.push(entry);
    }

    /// Records a tool call that is about to run.
    pub fn push_tool_start(
        &mut self,
        call_id: impl Into<String>,
        tool: impl Into<String>,
        summary: impl Into<String>,
        at: u64,
    ) {
        let mut entry = TranscriptEntry::new(EntryRole::Tool, at);
        entry.call_id = call_id.into();
        entry.tool = tool.into();
        entry.summary = summary.into();
        self.entries.push(entry);
    }

    /// Completes the tool call started with `call_id`.
    ///
    /// Falls back to appending when the start was never seen (a resumed turn, or
    /// a provider that reports only results).
    pub fn push_tool_result(
        &mut self,
        call_id: &str,
        tool: &str,
        ok: bool,
        elapsed_ms: u64,
        content: impl Into<String>,
        at: u64,
    ) {
        let content = truncate(content.into(), MAX_TOOL_CHARS);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.role == EntryRole::Tool && entry.call_id == call_id)
        {
            entry.ok = ok;
            entry.elapsed_ms = elapsed_ms;
            entry.content = content;
            entry.at = at;
            return;
        }
        let mut entry = TranscriptEntry::new(EntryRole::Tool, at);
        entry.call_id = call_id.to_string();
        entry.tool = tool.to_string();
        entry.ok = ok;
        entry.elapsed_ms = elapsed_ms;
        entry.content = content;
        self.entries.push(entry);
    }

    /// Records an engine notice.
    pub fn push_notice(&mut self, text: impl Into<String>, at: u64) {
        let mut entry = TranscriptEntry::new(EntryRole::Notice, at);
        entry.content = text.into();
        self.entries.push(entry);
    }

    /// Retains a structured yield marker across browser reloads.
    pub fn push_waiting(&mut self, task: uuid::Uuid, at: u64) {
        let mut entry = TranscriptEntry::new(EntryRole::Notice, at);
        entry.content = "Task yielded to an owned event.".into();
        entry.continuation_task = Some(task);
        self.entries.push(entry);
    }

    /// Records a failed turn.
    pub fn push_error(&mut self, text: impl Into<String>, at: u64) {
        let mut entry = TranscriptEntry::new(EntryRole::Error, at);
        entry.content = text.into();
        self.entries.push(entry);
    }

    /// Attaches the complete request duration only to this turn's answers.
    pub fn finish_turn(&mut self, elapsed_ms: u64) {
        for entry in &mut self.entries {
            if entry.role == EntryRole::Assistant {
                entry.turn_elapsed_ms = Some(elapsed_ms);
            }
        }
    }

    /// Returns the session's transcript, oldest entry first, bounded in size.
    pub fn finish(self) -> Vec<TranscriptEntry> {
        let mut all = self.history;
        all.extend(self.entries);
        if all.len() > MAX_ENTRIES {
            all.drain(..all.len() - MAX_ENTRIES);
        }
        all
    }
}

/// Truncates `text` to `max` bytes on a character boundary.
fn truncate(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[transcript truncated]", &text[..end])
}

/// Serializes a transcript for the wire, or `[]` when it cannot be encoded.
pub fn to_json(entries: &[TranscriptEntry]) -> String {
    serde_json::to_string(entries).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod reasoning_tests {
    use super::*;

    #[test]
    fn whole_turn_timing_updates_current_answers_without_changing_history() {
        let mut old = Transcript::default();
        old.push_assistant("old answer", "", 1);
        old.finish_turn(1000);
        let mut current = Transcript::resuming(old.finish());
        current.push_assistant("preamble", "", 2);
        current.push_tool_start("call", "ReadFile", "file", 3);
        current.push_assistant("answer", "", 4);
        current.finish_turn(9300);
        let entries: Vec<TranscriptEntry> =
            serde_json::from_str(&to_json(&current.finish())).unwrap();
        assert_eq!(entries[0].turn_elapsed_ms, Some(1000));
        assert_eq!(entries[1].turn_elapsed_ms, Some(9300));
        assert_eq!(entries[2].turn_elapsed_ms, None);
        assert_eq!(entries[3].turn_elapsed_ms, Some(9300));
    }

    #[test]
    fn duration_freezes_and_survives_serialization() {
        let mut transcript = Transcript {
            reasoning_started: Some(Instant::now() - Duration::from_secs(2)),
            ..Transcript::default()
        };
        let elapsed = transcript.finish_reasoning().unwrap();
        assert!(elapsed >= 2000);
        assert_eq!(transcript.finish_reasoning(), Some(elapsed));
        transcript.push_assistant("answer", "reasoning", 123);
        let restored: Vec<TranscriptEntry> =
            serde_json::from_str(&to_json(&transcript.finish())).unwrap();
        assert_eq!(restored[0].reasoning_elapsed_ms, Some(elapsed));
    }

    #[test]
    fn separate_segments_accumulate_but_answers_reset_timing() {
        let mut transcript = Transcript {
            reasoning_started: Some(Instant::now() - Duration::from_secs(2)),
            ..Transcript::default()
        };
        let _ = transcript.finish_reasoning();
        transcript.reasoning_started = Some(Instant::now() - Duration::from_secs(3));
        transcript.push_assistant("answer", "reasoning", 123);
        transcript.push_assistant("untimed", "old reasoning", 124);
        let entries = transcript.finish();
        assert!(entries[0].reasoning_elapsed_ms.unwrap() >= 5000);
        assert_eq!(entries[1].reasoning_elapsed_ms, None);
    }

    #[test]
    fn legacy_transcripts_have_unknown_duration_not_zero() {
        let entry: TranscriptEntry = serde_json::from_str(
            r#"{"role":"assistant","at":123,"content":"answer","reasoning":"thinking"}"#,
        )
        .unwrap();
        assert_eq!(entry.reasoning_elapsed_ms, None);
        assert!(!serde_json::to_string(&entry).unwrap().contains("reasoning_elapsed_ms"));
    }

    #[test]
    fn interim_text_and_tools_do_not_stop_the_answer_timer() {
        let mut transcript = Transcript {
            reasoning_started: Some(Instant::now() - Duration::from_secs(2)),
            ..Transcript::default()
        };
        transcript.push_assistant("Let me read the file.", "", 1);
        transcript.push_tool_start("call", "ReadFile", "file", 2);
        assert!(transcript.reasoning_started.is_some());
        transcript.push_assistant("Complete answer.", "Reasoning", 3);
        assert!(transcript.reasoning_started.is_none());
        let entries = transcript.finish();
        assert_eq!(entries[0].reasoning_elapsed_ms, None);
        assert!(entries[2].reasoning_elapsed_ms.unwrap() >= 2000);
    }
}
