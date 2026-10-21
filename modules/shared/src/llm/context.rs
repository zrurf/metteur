//! Context manager for assembling and pruning LLM context.
//!
//! The [`ContextManager`] holds system fragments, conversation messages and
//! tool results, and produces the message array sent to the model. It is
//! designed to be cloned freely so that a node can work on an isolated copy
//! without polluting the original context.

use serde::{Deserialize, Serialize};

use super::message::{Message, Role};
use super::token::estimate_message;
use super::tool::ToolResult;

/// A system prompt fragment with a priority and scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemFragment {
    /// Higher priority fragments are placed first.
    pub priority: i32,
    /// The scope this fragment applies to (e.g. a node kind).
    pub scope: String,
    /// The fragment content.
    pub content: String,
}

/// The policy used when evicting low-value tool results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Evict one-shot results first, then the oldest.
    Default,
}

/// Marker left in place of an evicted tool result.
const EVICTED_MARKER: &str = "[evicted: tool result removed to save context]";

/// Selects which tool results to release.
///
/// The selectors are a union: a result is released when it matches *any*
/// selector. `keep_recent` is the only subtraction and applies last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseQuery {
    /// Exact workspace-relative paths.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Glob patterns over workspace-relative paths (e.g. `src/**/*.rs`).
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Restrict to results produced by these tool names.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Release every releasable result (subject to `keep_recent`).
    #[serde(default)]
    pub all: bool,
    /// Never release the newest N results.
    #[serde(default)]
    pub keep_recent: usize,
}

impl ReleaseQuery {
    /// Whether the query selects anything at all.
    pub fn selects_anything(&self) -> bool {
        self.all || !self.paths.is_empty() || !self.patterns.is_empty() || !self.tools.is_empty()
    }
}

/// What one release pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseReport {
    /// Number of results released.
    pub released: usize,
    /// Number of results still retained afterwards.
    pub retained: usize,
    /// Characters freed by the release.
    pub freed_chars: usize,
    /// Estimated tokens freed (see `token::estimate_tokens`).
    pub estimated_tokens: usize,
    /// Per-tool released counts, sorted by tool name.
    pub by_tool: Vec<(String, usize)>,
}

impl ReleaseReport {
    /// Renders the report as the notice appended to the conversation.
    pub fn notice(&self) -> String {
        if self.released == 0 {
            return format!(
                "[engine] release request matched 0 tool results; {} retained.",
                self.retained
            );
        }
        let tools = self
            .by_tool
            .iter()
            .map(|(name, count)| format!("{name}: {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "[engine] released {} tool result(s) ({tools}), ~{} tokens freed; {} retained. \
             Re-read a file if you need its contents again.",
            self.released, self.estimated_tokens, self.retained
        )
    }
}

/// Normalizes a path for matching: forward slashes, no leading `./`.
fn normalize_path(raw: &str) -> String {
    let normalized = raw.replace('\\', "/");
    normalized.strip_prefix("./").unwrap_or(&normalized).to_string()
}

/// Merges consecutive messages of the same role into one.
///
/// Providers (Anthropic in particular) require the roles of a request to
/// alternate; a filtered history can easily produce two user turns in a row.
fn merge_adjacent_roles(messages: Vec<Message>) -> Vec<Message> {
    let mut merged: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages {
        match merged.last_mut() {
            Some(last) if last.role == message.role => {
                let combined =
                    format!("{}\n\n{}", last.text_content(), message.text_content());
                *last = Message::text(message.role, combined);
            }
            _ => merged.push(message),
        }
    }
    merged
}

/// Compiles release glob patterns, skipping malformed ones.
fn compile_patterns(patterns: &[String]) -> Vec<globset::GlobMatcher> {
    patterns
        .iter()
        .filter_map(|pattern| globset::Glob::new(pattern).ok().map(|glob| glob.compile_matcher()))
        .collect()
}

/// Whether one tool result is selected by a release query.
fn matches_release(
    result: &ToolResult,
    query: &ReleaseQuery,
    matchers: &[globset::GlobMatcher],
) -> bool {
    if query.all {
        return true;
    }
    if !query.tools.is_empty() && query.tools.contains(&result.tool) {
        return true;
    }
    if result.paths.is_empty() {
        return false;
    }
    let paths: Vec<String> =
        result.paths.iter().map(|path| normalize_path(&path.to_string_lossy())).collect();
    if !query.paths.is_empty()
        && paths.iter().any(|path| query.paths.iter().any(|q| normalize_path(q) == *path))
    {
        return true;
    }
    matchers.iter().any(|matcher| paths.iter().any(|path| matcher.is_match(path)))
}

/// An immutable snapshot of the context produced by [`ContextManager::filter`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextSnapshot {
    /// The system fragments.
    pub system_fragments: Vec<SystemFragment>,
    /// The messages.
    pub messages: Vec<Message>,
}

/// A per-region estimate of the context composition, for the audit UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRegion {
    /// The region label (`system`, `user`, `assistant`, `tool`).
    pub region: String,
    /// Estimated characters held by this region.
    pub chars: usize,
    /// Coarse token estimate (`chars / 4`).
    pub tokens: usize,
}

/// Manages the LLM context for a conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextManager {
    /// Ordered system fragments.
    pub system_fragments: Vec<SystemFragment>,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Tool results retained for potential eviction.
    pub tool_results: Vec<ToolResult>,
}

impl ContextManager {
    /// Creates a context from system fragments and an initial user prompt.
    pub fn new_from_prompt(system: Vec<SystemFragment>, initial_prompt: impl Into<String>) -> Self {
        Self {
            system_fragments: system,
            messages: vec![Message::text(Role::User, initial_prompt)],
            tool_results: Vec::new(),
        }
    }

    /// Mixes a tool result into the context (side effect).
    ///
    /// The result is appended to the messages as a tool message and retained
    /// in `tool_results` for later eviction.
    pub fn mix_in_tool_result(&mut self, result: ToolResult) {
        let tool_call_id = result.tool_call_id.clone();
        self.messages.push(Message {
            origin: Default::default(),
            role: Role::Tool,
            content: vec![super::message::ContentBlock::Text(result.content.clone())],
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id),
        });
        self.tool_results.push(result);
    }

    /// Appends a message to the context (side effect).
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Returns an immutable snapshot filtered by `predicate` (no side effect).
    pub fn filter(&self, predicate: impl Fn(&Message) -> bool) -> ContextSnapshot {
        ContextSnapshot {
            system_fragments: self.system_fragments.clone(),
            messages: self.messages.iter().filter(|m| predicate(m)).cloned().collect(),
        }
    }

    /// Evicts low-value tool results according to `policy` (side effect).
    ///
    /// One-shot results are removed first, followed by the oldest results,
    /// until the context is within `target_tool_results` retained results.
    /// Returns the number of evicted results.
    ///
    /// Evicted tool messages are *rewritten* to a short marker rather than
    /// deleted: providers reject a conversation whose assistant `tool_calls`
    /// entry has no matching tool response, so the message must keep its role
    /// and `tool_call_id`.
    pub fn evict(&mut self, policy: EvictionPolicy, target_tool_results: usize) -> usize {
        if self.tool_results.len() <= target_tool_results {
            return 0;
        }
        let mut evictable: Vec<usize> = (0..self.tool_results.len()).collect();
        match policy {
            EvictionPolicy::Default => {
                evictable.sort_by_key(|&i| {
                    let r = &self.tool_results[i];
                    (r.lifetime != super::tool::ToolResultLifetime::OneShot, r.timestamp)
                });
            }
        }
        let to_remove = self.tool_results.len() - target_tool_results;
        let removed: std::collections::HashSet<usize> =
            evictable.into_iter().take(to_remove).collect();

        let removed_ids: std::collections::HashSet<String> = self
            .tool_results
            .iter()
            .enumerate()
            .filter(|(i, _)| removed.contains(i))
            .map(|(_, r)| r.tool_call_id.clone())
            .collect();

        self.tool_results = self
            .tool_results
            .drain(..)
            .enumerate()
            .filter(|(i, _)| !removed.contains(i))
            .map(|(_, r)| r)
            .collect();
        for message in &mut self.messages {
            let is_evicted = message.role == Role::Tool
                && message
                    .tool_call_id
                    .as_deref()
                    .map(|id| removed_ids.contains(id))
                    .unwrap_or(false);
            if is_evicted && message.text_content() != EVICTED_MARKER {
                message.content =
                    vec![super::message::ContentBlock::Text(EVICTED_MARKER.to_string())];
            }
        }
        removed_ids.len()
    }

    /// Rewrites tool results that read `paths` to a stale marker (side effect).
    ///
    /// Called after a tool mutated files: a read performed before the change
    /// no longer describes the file, so keeping its content would let the
    /// model edit against an outdated view. Messages keep their role and
    /// `tool_call_id` so the tool-call pairing stays valid. Returns the number
    /// of invalidated results.
    pub fn invalidate_paths(&mut self, paths: &[std::path::PathBuf]) -> usize {
        if paths.is_empty() {
            return 0;
        }
        let stale_ids: std::collections::HashSet<String> = self
            .tool_results
            .iter()
            .filter(|result| result.paths.iter().any(|p| paths.contains(p)))
            .map(|result| result.tool_call_id.clone())
            .collect();
        if stale_ids.is_empty() {
            return 0;
        }
        let marker = format!(
            "[stale: {} was modified after this read; read it again for current content]",
            paths
                .first()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default()
        );
        for message in &mut self.messages {
            let is_stale = message.role == Role::Tool
                && message
                    .tool_call_id
                    .as_deref()
                    .map(|id| stale_ids.contains(id))
                    .unwrap_or(false);
            if is_stale {
                message.content = vec![super::message::ContentBlock::Text(marker.clone())];
            }
        }
        self.tool_results.retain(|result| !stale_ids.contains(&result.tool_call_id));
        stale_ids.len()
    }

    /// Restores the message order providers require (side effect).
    ///
    /// Every tool result must directly follow the assistant turn that requested
    /// it. A daemon defect wrote an extra assistant text message in between, so
    /// sessions stored by an older build are invalid on replay: the provider
    /// rejects the request and the conversation cannot continue. This folds any
    /// stranded assistant text back into the calling turn (when it is not
    /// already there) and drops the extra message.
    ///
    /// Returns the number of messages removed.
    pub fn repair_tool_pairing(&mut self) -> usize {
        let mut removed = 0usize;
        let mut index = 0usize;
        while index < self.messages.len() {
            if self.messages[index].tool_calls.is_empty() {
                index += 1;
                continue;
            }
            let call_ids: std::collections::HashSet<&str> = self.messages[index]
                .tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect();
            // The run of assistant messages sitting between the call and its
            // results is what an old build produced.
            let mut end = index + 1;
            while end < self.messages.len()
                && self.messages[end].role == Role::Assistant
                && self.messages[end].tool_calls.is_empty()
            {
                end += 1;
            }
            let results_follow = self
                .messages
                .get(end)
                .map(|message| {
                    message.role == Role::Tool
                        && message
                            .tool_call_id
                            .as_deref()
                            .map(|id| call_ids.contains(id))
                            .unwrap_or(false)
                })
                .unwrap_or(false);
            if end == index + 1 || !results_follow {
                index = end.max(index + 1);
                continue;
            }
            // Keep the text if the calling turn does not already carry it.
            let existing = self.messages[index].text_content();
            let stranded: Vec<super::message::ContentBlock> = self.messages[index + 1..end]
                .iter()
                .flat_map(|message| message.content.iter())
                .filter(|block| match block {
                    super::message::ContentBlock::Text(text) => {
                        !text.is_empty() && !existing.contains(text.as_str())
                    }
                    _ => false,
                })
                .cloned()
                .collect();
            self.messages[index].content.extend(stranded);
            self.messages.drain(index + 1..end);
            removed += end - index - 1;
            index += 1;
        }
        removed
    }

    /// Releases the tool results selected by `query` (side effect).
    ///
    /// Selected results are rewritten in place to a short marker — the message
    /// keeps its role and `tool_call_id` so the assistant's `tool_calls` entry
    /// still has a matching response — and pruned from `tool_results`.
    ///
    /// Releasing rewrites earlier messages, which invalidates the provider
    /// prefix cache from that position on; callers that want to keep the cache
    /// warm should release the oldest results and leave the recent tail alone
    /// (`keep_recent`). Invalid glob patterns are ignored rather than failing
    /// the whole pass.
    pub fn release(&mut self, query: &ReleaseQuery) -> ReleaseReport {
        let matchers = compile_patterns(&query.patterns);
        let releasable = self.tool_results.len().saturating_sub(query.keep_recent);

        let mut markers: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut by_tool: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let mut report = ReleaseReport::default();
        for (index, result) in self.tool_results.iter().enumerate() {
            if index >= releasable || !matches_release(result, query, &matchers) {
                continue;
            }
            let label = if result.tool.is_empty() {
                "tool".to_string()
            } else {
                result.tool.clone()
            };
            markers.insert(
                result.tool_call_id.clone(),
                format!("[released: {label} result removed by request]"),
            );
            report.freed_chars += result.content.chars().count();
            report.estimated_tokens += super::token::estimate_tokens(&result.content) as usize;
            *by_tool.entry(label).or_insert(0) += 1;
        }

        if markers.is_empty() {
            report.retained = self.tool_results.len();
            return report;
        }
        self.tool_results.retain(|result| !markers.contains_key(&result.tool_call_id));
        for message in &mut self.messages {
            let marker = message
                .tool_call_id
                .as_deref()
                .filter(|_| message.role == Role::Tool)
                .and_then(|id| markers.get(id));
            if let Some(marker) = marker {
                message.content = vec![super::message::ContentBlock::Text(marker.clone())];
            }
        }
        report.released = markers.len();
        report.retained = self.tool_results.len();
        report.by_tool = by_tool.into_iter().collect();
        report
    }

    /// Rewrites earlier full reads of `paths` made by `tool` (side effect).
    ///
    /// Called before a newer full read is mixed in: the newer result carries at
    /// least as much information as the older one, so keeping both wastes
    /// context. Only results whose path set matches exactly are superseded —
    /// a windowed read or a multi-file search is left untouched. Returns the
    /// number of superseded results.
    pub fn supersede_reads(&mut self, tool: &str, paths: &[std::path::PathBuf]) -> usize {
        if tool.is_empty() || paths.is_empty() {
            return 0;
        }
        let wanted: std::collections::HashSet<std::path::PathBuf> =
            paths.iter().cloned().collect();
        let label = normalize_path(&paths[0].to_string_lossy());
        let superseded: std::collections::HashMap<String, String> = self
            .tool_results
            .iter()
            .filter(|result| {
                result.tool == tool
                    && !result.paths.is_empty()
                    && result.paths.iter().cloned().collect::<std::collections::HashSet<_>>()
                        == wanted
            })
            .map(|result| {
                (
                    result.tool_call_id.clone(),
                    format!("[superseded: a later read of {label} replaced this result]"),
                )
            })
            .collect();
        if superseded.is_empty() {
            return 0;
        }
        self.tool_results.retain(|result| !superseded.contains_key(&result.tool_call_id));
        for message in &mut self.messages {
            let marker = message
                .tool_call_id
                .as_deref()
                .filter(|_| message.role == Role::Tool)
                .and_then(|id| superseded.get(id));
            if let Some(marker) = marker {
                message.content = vec![super::message::ContentBlock::Text(marker.clone())];
            }
        }
        superseded.len()
    }

    /// Returns the recent text-only conversation, newest message last.
    ///
    /// Only prose user/assistant turns survive: an assistant turn that carries
    /// tool calls is dropped together with its responses, because keeping one
    /// side of the pair produces a sequence providers reject. The tail is kept
    /// within `max_tokens`; the newest message always survives.
    ///
    /// Adjacent turns of the same role are merged back into one: dropping the
    /// tool traffic in between can leave two user turns in a row, which
    /// Anthropic rejects ("roles must alternate").
    pub fn text_history(&self, max_tokens: usize) -> Vec<Message> {
        let mut kept: Vec<Message> = Vec::new();
        let mut tokens = 0u64;
        for message in self.messages.iter().rev() {
            let is_prose = matches!(message.role, Role::User | Role::Assistant)
                && message.tool_calls.is_empty()
                && message.tool_call_id.is_none();
            if !is_prose {
                continue;
            }
            let cost = estimate_message(message);
            if !kept.is_empty() && tokens + cost > max_tokens as u64 {
                break;
            }
            tokens += cost;
            kept.push(message.clone());
        }
        kept.reverse();
        merge_adjacent_roles(kept)
    }

    /// Returns a compression boundary that keeps `keep_recent` tail messages
    /// without splitting a tool call from its responses.
    ///
    /// Providers reject a conversation that carries an assistant `tool_calls`
    /// entry without its matching responses (or the reverse), so the naive
    /// `len - keep_recent` boundary moves left while either side of it would
    /// separate a pair. Moving left always resolves both directions: the pair
    /// either travels into the summarized prefix or stays whole in the tail.
    pub fn pair_safe_split(&self, keep_recent: usize) -> usize {
        if self.messages.len() <= keep_recent {
            return 0;
        }
        let mut split_at = self.messages.len() - keep_recent;
        while split_at > 0 {
            let first_kept = &self.messages[split_at];
            let last_removed = &self.messages[split_at - 1];
            // A kept response whose call was summarized, or a summarized call
            // whose responses are kept, are both invalid sequences.
            let separates_a_pair =
                first_kept.role == Role::Tool || !last_removed.tool_calls.is_empty();
            if !separates_a_pair {
                break;
            }
            split_at -= 1;
        }
        split_at
    }

    /// Returns the system fragments in their canonical order.
    ///
    /// Fragments are sorted by descending [`SystemFragment::priority`], then by
    /// `scope` and content. The tie-breakers make the order total: two callers
    /// that assemble the same fragments in different orders produce the same
    /// system text, which is what keeps the provider prefix cache usable.
    pub fn ordered_fragments(&self) -> Vec<&SystemFragment> {
        let mut fragments: Vec<&SystemFragment> = self.system_fragments.iter().collect();
        fragments.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scope.cmp(&b.scope))
                .then_with(|| a.content.cmp(&b.content))
        });
        fragments
    }

    /// Returns the ordered fragments joined into a single system prompt.
    pub fn system_text(&self) -> String {
        self.ordered_fragments().into_iter().map(|f| f.content.as_str()).collect::<Vec<_>>().join("\n\n")
    }

    /// Merge all messages before `split_at` into a single assistant summary
    /// produced by `summarize`; no-op when `split_at` is zero or `summarize`
    /// returns `None`.
    ///
    /// The caller chooses `split_at`; use [`Self::pair_safe_split`] to pick a
    /// boundary that does not orphan a tool response. The message-count
    /// overload [`Self::compress`] applies that helper automatically.
    ///
    /// Tool results whose id appears only in the removed messages are pruned
    /// from `tool_results`.
    pub fn compress_to(
        &mut self,
        split_at: usize,
        summarize: impl FnOnce(&[Message]) -> Option<String>,
    ) {
        let split_at = split_at.min(self.messages.len());
        if split_at == 0 {
            return;
        }
        let Some(summary) = summarize(&self.messages[..split_at]) else {
            return;
        };
        let removed_ids: std::collections::HashSet<String> =
            self.messages[..split_at].iter().filter_map(|m| m.tool_call_id.clone()).collect();
        let kept_ids: std::collections::HashSet<String> =
            self.messages[split_at..].iter().filter_map(|m| m.tool_call_id.clone()).collect();
        // Prune results orphaned by the removal; ids still referenced by a
        // kept message must survive.
        self.tool_results.retain(|r| {
            !(removed_ids.contains(&r.tool_call_id) && !kept_ids.contains(&r.tool_call_id))
        });
        self.messages.drain(..split_at);
        self.messages.insert(0, Message::text(Role::Assistant, summary));
    }

    /// Merge all messages except the most recent `keep_recent` into a single
    /// assistant summary produced by `summarize`; no-op below threshold or
    /// when `summarize` returns `None`.
    ///
    /// The boundary is chosen by [`Self::pair_safe_split`].
    pub fn compress(
        &mut self,
        keep_recent: usize,
        summarize: impl FnOnce(&[Message]) -> Option<String>,
    ) {
        if self.messages.len() <= keep_recent {
            return;
        }
        self.compress_to(self.pair_safe_split(keep_recent), summarize);
    }

    /// Builds the message array to send to the model.
    ///
    /// Tool results are appended at the end to keep the prefix cache stable.
    pub fn build(&self) -> Vec<Message> {
        self.messages.clone()
    }

    /// Estimates the context composition by region for the audit UI.
    ///
    /// System fragments form the `system` region; messages are grouped by
    /// [`Role`] into `user`, `assistant` and `tool` regions.
    ///
    /// Tokens come from the same estimator the window guard uses. A separate
    /// formula here would let the meter the user sees disagree with the guard
    /// that actually protects the request, which is worse than either number
    /// being approximate.
    pub fn usage_report(&self) -> Vec<ContextRegion> {
        let mut tokens: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
        let mut chars: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let sys_chars: usize = self.system_fragments.iter().map(|f| f.content.chars().count()).sum();
        let sys_tokens: u64 =
            self.system_fragments.iter().map(|f| super::token::estimate_tokens(&f.content)).sum();
        if sys_tokens > 0 {
            tokens.insert("system", sys_tokens);
            chars.insert("system", sys_chars);
        }
        for message in &self.messages {
            let region = match message.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let text = message.text_content();
            *chars.entry(region).or_insert(0) += text.chars().count();
            *tokens.entry(region).or_insert(0) += super::token::estimate_message(message);
        }
        let mut regions: Vec<ContextRegion> = tokens
            .into_iter()
            .map(|(region, region_tokens)| ContextRegion {
                region: region.to_string(),
                chars: chars.get(region).copied().unwrap_or(0),
                tokens: region_tokens as usize,
            })
            .collect();
        regions.sort_by_key(|a| std::cmp::Reverse(a.tokens));
        regions
    }
}

#[cfg(test)]
mod tests {
    use super::super::tool::{ToolResult, ToolResultLifetime};
    use super::*;

    fn result(id: &str, lifetime: ToolResultLifetime, ts: u64) -> ToolResult {
        ToolResult {
            tool_call_id: id.to_string(),
            tool: "ReadFile".to_string(),
            content: format!("result {id}"),
            timestamp: ts,
            lifetime,
            paths: Vec::new(),
        }
    }

    #[test]
    fn new_from_prompt_creates_user_message() {
        let ctx = ContextManager::new_from_prompt(vec![], "hello");
        assert_eq!(ctx.messages.len(), 1);
        assert_eq!(ctx.messages[0].role, Role::User);
    }

    #[test]
    fn mix_in_tool_result_appends_message() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[1].role, Role::Tool);
        assert_eq!(ctx.messages[1].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn filter_has_no_side_effect() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        let snapshot = ctx.filter(|m| m.role == Role::User);
        assert_eq!(snapshot.messages.len(), 1);
        // Original context is unchanged.
        assert_eq!(ctx.messages.len(), 2);
    }

    #[test]
    fn evict_removes_one_shot_first() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        ctx.mix_in_tool_result(result("call_2", ToolResultLifetime::Persistent, 2));
        let removed = ctx.evict(EvictionPolicy::Default, 1);
        assert_eq!(removed, 1);
        assert_eq!(ctx.tool_results.len(), 1);
        assert_eq!(ctx.tool_results[0].tool_call_id, "call_2");
        // The evicted message survives as a marker so the assistant's
        // tool_calls entry keeps a matching response.
        assert_eq!(ctx.messages.len(), 3);
        assert_eq!(ctx.messages[1].role, Role::Tool);
        assert_eq!(ctx.messages[1].tool_call_id.as_deref(), Some("call_1"));
        assert!(ctx.messages[1].text_content().contains("evicted"));
    }

    #[test]
    fn evict_keeps_tool_call_pairing_valid() {
        let mut ctx = ContextManager::default();
        ctx.push_message(Message {
            origin: Default::default(),
            role: Role::Assistant,
            content: vec![super::super::message::ContentBlock::Text("working".to_string())],
            tool_calls: vec![super::super::tool::ToolCall {
                id: "call_1".to_string(),
                name: "ReadFile".to_string(),
                arguments: serde_json::json!({ "path": "a.rs" }),
            }],
            tool_call_id: None,
        });
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        ctx.evict(EvictionPolicy::Default, 0);
        // Every tool call still has exactly one tool response.
        let call_ids: Vec<&str> =
            ctx.messages.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str())).collect();
        let result_ids: Vec<&str> =
            ctx.messages.iter().filter_map(|m| m.tool_call_id.as_deref()).collect();
        assert_eq!(call_ids, result_ids);
    }

    #[test]
    fn invalidate_paths_marks_stale_reads() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        let mut read = result("call_1", ToolResultLifetime::Persistent, 1);
        read.paths = vec![std::path::PathBuf::from("src/lib.rs")];
        ctx.mix_in_tool_result(read);
        ctx.mix_in_tool_result(result("call_2", ToolResultLifetime::Persistent, 2));
        let invalidated = ctx.invalidate_paths(&[std::path::PathBuf::from("src/lib.rs")]);
        assert_eq!(invalidated, 1);
        assert!(ctx.messages[1].text_content().contains("stale"));
        // The unrelated result is untouched.
        assert_eq!(ctx.messages[2].text_content(), "result call_2");
        assert_eq!(ctx.tool_results.len(), 1);
    }

    #[test]
    fn clone_is_deep() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        let mut cloned = ctx.clone();
        cloned.mix_in_tool_result(result("call_2", ToolResultLifetime::OneShot, 2));
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(cloned.messages.len(), 3);
    }

    #[test]
    fn compress_below_threshold_is_noop() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "hello");
        ctx.push_message(Message::text(Role::Assistant, "hi"));
        let mut called = false;
        ctx.compress(5, |_| {
            called = true;
            Some("summary".to_string())
        });
        assert!(!called);
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, Role::User);
    }

    #[test]
    fn compress_merges_into_single_assistant_summary() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "q1");
        ctx.push_message(Message::text(Role::Assistant, "a1"));
        ctx.push_message(Message::text(Role::User, "q2"));
        ctx.compress(1, |older| {
            assert_eq!(older.len(), 2);
            Some(format!("summary of {}", older.len()))
        });
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, Role::Assistant);
        assert_eq!(ctx.messages[0].text_content(), "summary of 2");
        // The most recent message is kept untouched.
        assert_eq!(ctx.messages[1].role, Role::User);
        assert_eq!(ctx.messages[1].text_content(), "q2");
    }

    #[test]
    fn compress_skips_when_summarizer_returns_none() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "q1");
        ctx.push_message(Message::text(Role::Assistant, "a1"));
        ctx.compress(1, |_| None);
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.messages[0].role, Role::User);
    }

    #[test]
    fn compress_prunes_orphaned_tool_results() {
        let mut ctx = ContextManager::new_from_prompt(vec![], "q1");
        ctx.mix_in_tool_result(result("call_1", ToolResultLifetime::OneShot, 1));
        ctx.push_message(Message::text(Role::Assistant, "a1"));
        ctx.mix_in_tool_result(result("call_2", ToolResultLifetime::Persistent, 2));
        ctx.push_message(Message::text(Role::Assistant, "done"));
        // Removes [user q1, tool call_1]; keeps [assistant a1, tool call_2, assistant done].
        ctx.compress(3, |_| Some("earlier work".to_string()));
        assert_eq!(ctx.messages.len(), 4);
        assert_eq!(ctx.messages[0].role, Role::Assistant);
        assert_eq!(ctx.messages[0].text_content(), "earlier work");
        assert_eq!(ctx.tool_results.len(), 1);
        assert_eq!(ctx.tool_results[0].tool_call_id, "call_2");
    }
}
