//! No weaker traversal fallback on platforms without Linux mount guarantees.
use super::Measurement;
use crate::storage::unavailable;
use devcoordinator2_api::{ProtocolError, storage::Filesystem};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

fn unsupported<T>() -> Result<T, ProtocolError> {
    Err(unavailable("storage_filesystem_requires_linux"))
}

pub fn open_directory(_: &Path) -> Result<File, ProtocolError> {
    unsupported()
}
pub fn identity(_: &Path) -> Result<(u64, u64), ProtocolError> {
    unsupported()
}
pub fn absent(_: &Path) -> Result<bool, ProtocolError> {
    unsupported()
}
pub fn measure(_: &Path) -> Result<Measurement, ProtocolError> {
    unsupported()
}
pub struct CheckoutSnapshot {
    pub fingerprint: String,
    pub protected: bool,
    pub nested_git: bool,
    pub measurement: Measurement,
}
pub struct CheckoutSnapshotCursor;
impl CheckoutSnapshotCursor {
    pub fn new(_: &Path) -> Result<Self, ProtocolError> {
        unsupported()
    }
    pub fn progress(&self) -> (u64, u64, u64) {
        (0, 0, 0)
    }
    pub fn step(
        &mut self,
        _: usize,
        _: usize,
        _: std::time::Duration,
    ) -> Result<Option<CheckoutSnapshot>, ProtocolError> {
        unsupported()
    }
}
pub fn checkout_snapshot(_: &Path) -> Result<CheckoutSnapshot, ProtocolError> {
    unsupported()
}
pub fn checkout_index_fingerprint(_: &Path) -> Result<Option<String>, ProtocolError> {
    unsupported()
}
pub struct CheckoutIndexCursor;
impl CheckoutIndexCursor {
    pub fn new_reflog(_: &Path) -> Result<Self, ProtocolError> {
        unsupported()
    }
    pub fn reflog_roots(&self) -> Vec<String> {
        Vec::new()
    }
    pub fn new(_: &Path) -> Result<Self, ProtocolError> {
        unsupported()
    }
    pub fn bytes_hashed(&self) -> u64 {
        0
    }
    pub fn step(
        &mut self,
        _: usize,
        _: std::time::Duration,
    ) -> Result<Option<Option<String>>, ProtocolError> {
        unsupported()
    }
}
pub struct MeasurementCursor;
impl MeasurementCursor {
    pub fn new(_: &Path) -> Result<Self, ProtocolError> {
        unsupported()
    }
    pub fn step(
        &mut self,
        _: usize,
        _: std::time::Duration,
    ) -> Result<Option<Measurement>, ProtocolError> {
        unsupported()
    }
}
pub fn mount_targets() -> Result<Vec<PathBuf>, ProtocolError> {
    unsupported()
}
pub fn remove_tree(_: &Path, _: (u64, u64), _: bool, _: bool) -> Result<(), ProtocolError> {
    unsupported()
}
pub fn validate_native_snapshot(_: &Path) -> Result<(), ProtocolError> {
    unsupported()
}
pub fn remove_native_snapshot(_: &Path, _: (u64, u64), _: (u64, u64)) -> Result<(), ProtocolError> {
    unsupported()
}
pub fn filesystem(_: &Path, _: u64, _: &str) -> Result<Filesystem, ProtocolError> {
    unsupported()
}
pub fn unescape_mount(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
