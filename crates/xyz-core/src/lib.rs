//! Shared host core. No local command execution or hidden SSH exec channels.
pub mod ssh;

pub mod model;
mod model_transport;
pub mod tasks;

#[cfg(windows)]
mod windows_private_file;

pub mod agent;
pub mod decision;
mod execution;
pub mod observation;
