//! Batch writer abstraction for `commit_plan`.
//!
//! `RealBatchWriter` delegates each write to `atomic::atomic_write`.
//! `FailingWriter` is a test scaffold — passes through the first `allow_n`
//! writes, then fails. Used by `tests/init_partial_failure.rs` to assert
//! `LensError::BatchPartial { applied, pending, failed, source }` (CB6).

use std::path::Path;

use crate::errors::LensError;

pub trait BatchWriter {
    fn write(&mut self, dest: &Path, contents: &[u8]) -> Result<(), LensError>;
}

pub struct RealBatchWriter;

impl BatchWriter for RealBatchWriter {
    fn write(&mut self, dest: &Path, contents: &[u8]) -> Result<(), LensError> {
        crate::cli::init::atomic::atomic_write(dest, contents)
    }
}

/// Test scaffold: succeeds for first `allow_n` writes, then fails. Use to drive
/// `commit_plan` into the `LensError::BatchPartial` path.
pub struct FailingWriter {
    pub allow_n: usize,
    pub count: usize,
}

impl FailingWriter {
    pub fn new(allow_n: usize) -> Self {
        Self { allow_n, count: 0 }
    }
}

impl BatchWriter for FailingWriter {
    fn write(&mut self, dest: &Path, contents: &[u8]) -> Result<(), LensError> {
        if self.count >= self.allow_n {
            self.count += 1;
            return Err(LensError::Profile {
                detail: format!("FailingWriter induced failure on {}", dest.display()),
            });
        }
        self.count += 1;
        crate::cli::init::atomic::atomic_write(dest, contents)
    }
}
