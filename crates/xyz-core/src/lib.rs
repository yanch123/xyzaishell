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
mod risk;

pub mod host_storage;
pub mod storage;

pub mod history;
pub mod persistence;
pub mod task_secrets;

pub mod shell_log;
pub mod shell_log_protocol;
pub mod shell_log_store;
pub mod shell_log_types;

pub mod shell_integration;
