//! Execution implementation for validated schema-2 plans.
//!
//! Process execution is added independently from the protocol crate so the
//! eventual Rust daemon can link the engine without depending on the CLI.

pub use devcoordinator2_executor_protocol as protocol;
