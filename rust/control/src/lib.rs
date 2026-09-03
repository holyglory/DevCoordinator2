//! Rust control plane for DevCoordinator2.

pub mod bugs;
pub mod client;
pub mod config;
pub mod daemon;
pub mod database;
pub mod ids;
pub mod plan;
pub mod repository;

pub const DATABASE_SCHEMA_VERSION: u32 = 15;
pub const SOURCE_COMMIT: &str = match option_env!("DEVCOORDINATOR2_SOURCE_COMMIT") {
    Some(value) => value,
    None => "development",
};
