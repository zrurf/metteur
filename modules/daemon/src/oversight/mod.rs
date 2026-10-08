//! Restricted run side channels. No execution tools or approval broker are exposed.
pub mod budget;
pub mod conversation;
pub mod diagnostic;
pub(crate) mod projection;
pub mod requests;

pub(crate) mod actions;
pub(crate) mod control;
pub mod review;
pub(crate) mod runtime;
pub mod scheduler;

pub(crate) mod policy;

pub(crate) mod recovery;
