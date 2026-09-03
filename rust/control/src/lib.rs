//! Rust control plane for DevCoordinator2.

pub mod access;
pub mod artifact_materialize;
pub mod bugs;
pub mod capacity;
pub mod check_event;
pub mod cli;
pub mod client;
pub mod config;
pub mod control_plane;
pub mod daemon;
pub mod database;
pub mod docker;
pub mod ids;
pub mod mcp;
pub mod plan;
pub mod platform;
pub mod ports;
pub mod repository;
pub mod repository_config;
pub mod routes;
pub mod systemd;
pub mod telegram;
pub mod test_admission;
pub mod test_artifacts;
pub mod test_logs;

pub const DATABASE_SCHEMA_VERSION: u32 = 15;
pub const SOURCE_COMMIT: &str = match option_env!("DEVCOORDINATOR2_SOURCE_COMMIT") {
    Some(value) => value,
    None => "development",
};
