//! Execution implementation for validated schema-2 plans.
//!
//! Process execution is added independently from the protocol crate so the
//! eventual Rust daemon can link the engine without depending on the CLI.

mod capacity;
pub mod diagnostics;
mod error;
mod evidence;
mod process;
mod runner;

pub use capacity::{
    CapacityObservation, LocalPermitProvider, PermitProvider, PermitRequest, UnixPermitProvider,
};
pub use devcoordinator2_executor_protocol as protocol;
pub use error::ExecutorError;
pub use evidence::{artifact_receipts, receipts_match, source_digest};
pub use process::Cancellation;
pub use runner::Executor;
