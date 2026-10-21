//! Chat session persistence.
//!
//! A session stores the full conversation context so a chat can be resumed
//! after a daemon restart with its tool results intact, plus the display
//! transcript the client renders (the two differ: see [`transcript`]). Storage
//! is scoped to the workspace database; see [`session`] for the record model.

pub mod checkpoint;
pub mod session;
pub mod transcript;

pub mod turn;
pub mod wait;
