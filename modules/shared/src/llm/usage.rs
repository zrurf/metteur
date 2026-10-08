//! Token usage reported by an LLM request.

use serde::{Deserialize, Serialize};

/// Token usage for a single LLM request.
///
/// `input_tokens` is the *total* input count, including the parts served from
/// and written to the provider's prompt cache; the two cache fields break that
/// total down. Providers report these differently, so each provider adapter
/// normalizes to this single convention (see the provider modules).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Whether the provider explicitly reported complete normalized token totals.
    #[serde(default)]
    pub tokens_reported: bool,
    /// Whether cache reads were explicitly reported, including a confirmed zero.
    #[serde(default)]
    pub cache_read_reported: bool,
    /// Tokens consumed by the input (cache reads and writes included).
    pub input_tokens: u64,
    /// Total output tokens, including reasoning tokens.
    pub output_tokens: u64,
    /// Reasoning detail already included in output (only reported by some models).
    pub reasoning_tokens: u64,
    /// Input plus output tokens, without adding cache or reasoning details again.
    pub total_tokens: u64,
    /// Input tokens served from the provider's prompt cache.
    #[serde(default)]
    pub cached_input_tokens: u64,
    /// Input tokens written into the provider's prompt cache.
    #[serde(default)]
    pub cache_write_input_tokens: u64,
}

impl Usage {
    /// Cache-hit ratio over the input, or `None` when nothing was reportable.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        if !self.tokens_reported
            || !self.cache_read_reported
            || self.input_tokens == 0
            || self.cached_input_tokens > self.input_tokens
        {
            return None;
        }
        Some(self.cached_input_tokens as f64 / self.input_tokens as f64)
    }

    /// Input tokens billed at the regular (uncached) rate.
    pub fn uncached_input_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_sub(self.cached_input_tokens)
            .saturating_sub(self.cache_write_input_tokens)
    }

    /// Accumulates another usage into this one.
    pub fn add(&mut self, other: &Usage) {
        // An unknown operand cannot become complete by adding known counters.
        self.tokens_reported &= other.tokens_reported;
        self.cache_read_reported &= other.cache_read_reported;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.total_tokens += other.total_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.cache_write_input_tokens += other.cache_write_input_tokens;
    }
}
