//! Shared types and abstractions used across Metteur crates.
//!
//! This crate contains the cross-cutting data model (blueprints, values),
//! the URI-based filesystem abstraction, configuration types and shared
//! error types. It has no dependency on the daemon or any client.

pub mod config;
pub mod dsl;
pub mod error;
pub mod fs;
pub mod llm;
pub mod model;
pub mod uri;
pub mod resources;
pub mod workflow;

pub use error::{SharedError, SharedResult};
pub use fs::{FileSystem, Metadata, NativeFileSystem};
pub use llm::{
    ContentBlock, ContextManager, ContextSnapshot, EvictionPolicy, GenerationParams, Message,
    ReasoningEffort, Role, SystemFragment, TodoItem, TodoStatus, ToolCall, ToolDefinition,
    ToolResult, ToolResultLifetime, Usage,
};
pub use model::types::{coerce, compatible};
pub use model::validate::{BlueprintError, validate};
pub use model::{
    Blueprint, DataType, Edge, EdgeId, Node, NodeId, NodeType, Pin, PinId, PinType, Value,
};
pub use uri::Uri;

#[doc(inline)]
pub use model::function::{
    CALL_FUNCTION_KIND, FUNCTION_ENTRY_KIND, FUNCTION_EXIT_KIND, FnPin, FunctionEntry,
    FunctionSignature, FunctionSource,
};

/// Canonical node contracts and immutable registry snapshots.
pub mod node_catalog;
