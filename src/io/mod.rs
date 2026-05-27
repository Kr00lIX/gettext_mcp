//! Filesystem abstraction used by the store and manager.
//!
//! Production code uses [`FsFileStore`] from [`fs`]; tests can plug in
//! their own [`FileStore`] impl for in-memory or fault-injected scenarios.

pub mod fs;

use std::path::Path;
use std::time::SystemTime;

use crate::error::GettextError;

pub use fs::{cleanup_orphan_tmps, FsFileStore};

/// File I/O contract used by [`crate::service::store::GettextStore`].
///
/// Implementations are expected to be `Send + Sync` so they can be shared
/// behind an `Arc` across async tasks. Reads and writes are blocking by
/// design (called from `spawn_blocking` in the store layer).
pub trait FileStore: Send + Sync {
    /// Read the file at `path` as a UTF-8 string. Implementations should
    /// strip any leading UTF-8 BOM before returning.
    fn read(&self, path: &Path) -> Result<String, GettextError>;

    /// Atomically write `content` to `path`. Implementations should hold
    /// an advisory lock on the target during the write where possible.
    fn write(&self, path: &Path, content: &str) -> Result<(), GettextError>;

    /// Return the on-disk modified time of `path`, used by the store
    /// manager to detect external edits and invalidate caches.
    fn modified_time(&self, path: &Path) -> Result<SystemTime, GettextError>;

    /// `true` if `path` exists on disk.
    fn exists(&self, path: &Path) -> bool;
}
