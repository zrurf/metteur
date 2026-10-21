//! LLM domain model shared across crates.
//!
//! These types are provider-agnostic: they describe messages, tool calls,
//! generation parameters and usage in a way that any LLM provider (OpenAI,
//! Anthropic, etc.) can be mapped onto. The daemon's provider implementations
//! translate between these types and each vendor's wire format.

pub mod context;
pub mod message;
pub mod params;
pub mod todo;
pub mod token;
pub mod tool;
pub mod usage;

pub use context::{
    ContextManager, ContextRegion, ContextSnapshot, EvictionPolicy, ReleaseQuery, ReleaseReport,
    SystemFragment,
};
pub use message::{ContentBlock, Message, MessageOrigin, Role};
pub use params::{GenerationParams, ReasoningEffort};
pub use todo::{TodoItem, TodoStatus, render as render_todos};
pub use token::{estimate_context_tokens, estimate_message, estimate_tokens};
pub use tool::{ToolCall, ToolDefinition, ToolResult, ToolResultLifetime};
pub use usage::Usage;
