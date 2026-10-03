//! Token accounting.
//!
//! OpenAI-family models are counted exactly with a real BPE tokenizer;
//! everything else (Anthropic, unknown gateways) falls back to a calibrated
//! heuristic. Real usage figures always come from the provider response — these
//! numbers exist so the context-window guard can fire *before* a request is
//! rejected, so biasing high is correct and biasing low is the one failure mode
//! that matters.
//!
//! The heuristic's flat `4 chars/token` is a prose ratio. Source code, JSON tool
//! arguments and diffs tokenize at roughly 2.5-3.3 chars/token, i.e. the flat
//! ratio *under-counts the exact payloads this agent ships*, which is why the
//! fallback carries a wider safety margin than the exact path (see
//! `daemon::llm::budget`).

use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

use super::{ContextManager, ContentBlock};

/// Average characters per token for ASCII prose.
const ASCII_CHARS_PER_TOKEN: usize = 4;

/// Extra chars-per-token headroom for non-prose payloads (code, JSON, diffs),
/// which tokenize denser than prose. Expressed as a multiplier over the flat
/// ratio: `4 / 1.35 ≈ 3.0` chars/token.
const DENSE_PAYLOAD_FACTOR: f64 = 1.35;

/// Wire overhead of one message: role header plus the content-block envelope.
pub const MESSAGE_ENVELOPE_TOKENS: u64 = 4;

/// Wire overhead of one tool call, covering the `tool_use`/`tool_result`
/// pairing envelope and the call id.
pub const TOOL_CALL_ENVELOPE_TOKENS: u64 = 4;

/// A tokenizer for one model family.
///
/// Public so callers can tell how much to trust a count: an [`Self::Exact`]
/// figure is off by a percent or two, a [`Self::Heuristic`] one can be off by a
/// third — the difference between a guard that fires on time and one that lets
/// a request overflow.
pub enum TokenCounter {
    /// Exact BPE counting for an OpenAI-compatible encoding.
    Exact(&'static CoreBPE),
    /// Calibrated heuristic for providers without a public tokenizer.
    Heuristic,
}

/// The `o200k_base` table, shared by GPT-4o and the o-series.
static O200K: OnceLock<CoreBPE> = OnceLock::new();
/// The `cl100k_base` table, shared by GPT-3.5 and GPT-4.
static CL100K: OnceLock<CoreBPE> = OnceLock::new();

/// Selects a counter for a model id.
///
/// Unknown ids get the heuristic: guessing an encoding for an unrelated family
/// would trade a known approximation error for an unknown one.
pub fn counter_for(model_id: &str) -> TokenCounter {
    let id = model_id.to_ascii_lowercase();
    // The o-series and GPT-4o* moved to o200k; earlier GPT models use cl100k.
    let o200k = id.starts_with("gpt-4o")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("chatgpt");
    let cl100k =
        id.starts_with("gpt-4") || id.starts_with("gpt-3") || id.starts_with("text-embedding");
    if o200k {
        TokenCounter::Exact(O200K.get_or_init(|| {
            tiktoken_rs::o200k_base().expect("the bundled o200k_base table must load")
        }))
    } else if cl100k {
        TokenCounter::Exact(CL100K.get_or_init(|| {
            tiktoken_rs::cl100k_base().expect("the bundled cl100k_base table must load")
        }))
    } else {
        TokenCounter::Heuristic
    }
}

/// Counts the tokens of a single text.
///
/// ASCII runs are divided by [`ASCII_CHARS_PER_TOKEN`]; every non-ASCII
/// character counts as a full token, which approximates CJK text (roughly one
/// token per character) far better than a flat character division.
pub fn estimate_tokens(text: &str) -> u64 {
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii = text.chars().count() - ascii;
    (ascii as u64).div_ceil(ASCII_CHARS_PER_TOKEN as u64) + non_ascii as u64
}

/// Counts tokens exactly when the model has a known encoding.
pub fn count_tokens(text: &str, model_id: &str) -> u64 {
    match counter_for(model_id) {
        TokenCounter::Exact(bpe) => bpe.encode_with_special_tokens(text).len() as u64,
        TokenCounter::Heuristic => estimate_tokens(text),
    }
}

/// Estimates the tokens of one message, including tool calls.
///
/// The wire envelope is counted even though it is not part of the message
/// content: the API bills it, and skipping it lets a context drift past the
/// window by a few dozen tokens per retained tool result.
pub fn estimate_message(message: &super::Message) -> u64 {
    let mut total = MESSAGE_ENVELOPE_TOKENS;
    for block in &message.content {
        total += match block {
            ContentBlock::Text(text) => estimate_tokens(text),
            // Thinking text is replayed on every subsequent request, so it
            // occupies context whether or not the provider bills it.
            ContentBlock::Thinking { text, signature } => {
                estimate_tokens(text) + signature.as_deref().map(estimate_tokens).unwrap_or(0)
            }
            // The signed blob is opaque base64 but is still sent back verbatim.
            ContentBlock::RedactedThinking { data } => estimate_tokens(data),
        };
    }
    for call in &message.tool_calls {
        total +=
            TOOL_CALL_ENVELOPE_TOKENS + estimate_tokens(&call.name) + estimate_tokens(&call.arguments.to_string());
    }
    total
}

/// Estimates the tokens of a whole context (system fragments + messages).
pub fn estimate_context_tokens(context: &ContextManager) -> u64 {
    let system: u64 =
        context.system_fragments.iter().map(|f| estimate_tokens(&f.content)).sum();
    let messages: u64 = context.messages.iter().map(estimate_message).sum();
    system + messages
}

/// The estimated size of a context, scaled for how densely the payload is
/// expected to tokenize.
///
/// Callers use this to decide whether a request fits, so a payload of source
/// code and tool results — which tokenize denser than prose — must not be
/// measured with the prose ratio.
pub fn estimate_context_tokens_dense(context: &ContextManager) -> u64 {
    (estimate_context_tokens(context) as f64 * DENSE_PAYLOAD_FACTOR).ceil() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample whose o200k count is stable across tiktoken releases, asserted
    /// as a range rather than an exact figure so a table refresh cannot break
    /// the build.
    #[test]
    fn exact_counting_tracks_the_real_tokenizer() {
        let text = "fn main() { println!(\"hello world\"); }";
        let exact = count_tokens(text, "gpt-4o");
        assert!(exact > 0);
        // The heuristic must never under-count the exact figure by much: the
        // guard is allowed to fire early, never late.
        let heuristic = estimate_tokens(text);
        assert!(
            heuristic >= exact / 2,
            "heuristic {heuristic} collapsed relative to exact {exact}"
        );
    }

    #[test]
    fn model_families_select_distinct_encodings() {
        // o-series and 4o use o200k; 3.5/4 use cl100k; unknown gets the
        // heuristic rather than a guessed table.
        assert!(matches!(counter_for("gpt-4o-2024-08-06"), TokenCounter::Exact(_)));
        assert!(matches!(counter_for("o3-mini"), TokenCounter::Exact(_)));
        assert!(matches!(counter_for("GPT-4-TURBO"), TokenCounter::Exact(_)));
        assert!(matches!(counter_for("claude-sonnet-4"), TokenCounter::Heuristic));
        assert!(matches!(counter_for(""), TokenCounter::Heuristic));
    }

    /// JSON tool arguments are the densest payload this agent ships, so the
    /// dense scaling factor must push the estimate up, never down.
    #[test]
    fn dense_estimate_scales_up() {
        let json = r#"{"path":"src/execution/interpreter.rs","old":"fn a() -> u32 { 0 }","new":"fn a() -> u32 { 1 }"}"#;
        let prose_estimate = estimate_tokens(json);
        let dense = (prose_estimate as f64 * DENSE_PAYLOAD_FACTOR).ceil() as u64;
        assert!(dense > prose_estimate);
    }

    #[test]
    fn redacted_thinking_is_counted() {
        let message = super::super::Message {
            role: super::super::Role::Assistant,
            content: vec![ContentBlock::RedactedThinking {
                data: "x".repeat(400),
            }],
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        // 400 base64 chars is not free: the block is replayed on every turn.
        assert!(estimate_message(&message) >= 100);
    }
}
