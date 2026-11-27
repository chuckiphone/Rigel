//! The IPC boundary, split by the subsystem each command belongs to.
//!
//! Commands are `pub(crate)` so the crate root can hand them to Tauri; each file owns one
//! subsystem and uses the shared types and helpers in the crate root.

pub(crate) mod agents;
pub(crate) mod audit;
pub(crate) mod chat;
pub(crate) mod cloud;
pub(crate) mod coordination;
pub(crate) mod mcp;
pub(crate) mod memory;
pub(crate) mod patches;
pub(crate) mod providers;
pub(crate) mod safety;
pub(crate) mod sessions;
pub(crate) mod settings;
pub(crate) mod shell;
pub(crate) mod skills;
pub(crate) mod swarm;
pub(crate) mod turns;
pub(crate) mod workspace;
