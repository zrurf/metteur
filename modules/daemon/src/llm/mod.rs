//! LLM client abstraction, model pricing and provider implementations.

pub mod billing;
pub mod budget;
pub mod client;
pub mod mock;
pub mod peak;
pub mod provider;
pub mod retry;

pub use billing::{Cost, ModelUsageSummary, UsageSummaryData};
pub use budget::{context_window_budget, has_configured_window};
pub use client::{
    LlmClient, LlmClientFactory, LlmProviderConfig, LlmResponse, ProviderKind, StreamDelta,
    ThinkingBlock,
};
pub use mock::{MockClient, MockStep};
pub use retry::RetryPolicy;

pub mod diagnostics;
