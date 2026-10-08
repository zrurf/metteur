//! The Metteur daemon library.
//!
//! This crate exposes the daemon's core modules so they can be reused by the
//! `metteurd` binary and exercised by integration tests.

pub mod addon;
pub mod chat;
pub mod cli;
pub mod config;
pub mod depgraph;
pub mod error;
pub mod execution;
pub mod grpc;
pub mod harness;
pub mod integration;
pub mod llm;
pub mod observability;
pub mod oversight;
pub mod registry;
pub mod replan;
pub mod sandbox;
pub mod startup;
pub mod storage;
pub mod tls;
pub mod wake;
pub mod workspace;

pub use error::{DaemonError, DaemonResult};
pub use grpc::{AppState, DaemonService};
pub use registry::Registry;
pub use workspace::WorkspaceManager;

// Compatibility aliases for the pre-refactor top-level module paths
// (used by the daemon binary and external callers).
pub use addon::signer;
pub use startup::{autostart, service};
pub use tls::cert;
