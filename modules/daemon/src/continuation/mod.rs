//! Durable host continuation state, separate from rewindable chat snapshots.
pub mod policy;
pub mod state;
pub mod store;
pub mod timer;

pub mod handoff;

pub mod launch;
