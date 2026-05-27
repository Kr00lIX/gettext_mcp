//! Production filesystem-backed [`FileStore`] implementation.
//!
//! Includes atomic writes via temp-file + fsync + rename, best-effort
//! POSIX advisory locking (`flock`) during writes, leading-BOM stripping
//! on reads, and orphan temp-file cleanup at startup.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use tracing::{info, warn};

use super::FileStore;
use crate::error::GettextError;

/// Prefix used for temporary files created during atomic writes.
pub(crate) const TMP_FILE_PREFIX: &str = ".gettext-mcp-";
/// Suffix used for temporary files created during atomic writes.
pub(crate) const TMP_FILE_SUFFIX: &str = ".tmp";

/// Strip a leading UTF-8 BOM (`\u{feff}`) if present.
fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

/// Best-effort cleanup of orphan `.gettext-mcp-*.tmp` files left over from
/// crashed writes. Scans the given directory non-recursively.
pub fn cleanup_orphan_tmps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(TMP_FILE_PREFIX) && name_str.ends_with(TMP_FILE_SUFFIX) {
            let path = entry.path();
            match fs::remove_file(&path) {
                Ok(()) => info!("cleaned up orphan temp file: {}", path.display()),
                Err(e) => warn!(
                    "failed to remove orphan temp file {}: {}",
                    path.display(),
                    e
                ),
            }
        }
    }
}

/// Default filesystem-backed file store: every method calls straight
/// through to `std::fs` with the safety extras described in the module
/// docs.
#[derive(Debug, Default)]
pub struct FsFileStore;

impl FsFileStore {
    pub fn new() -> Self {
        Self
    }
}

impl FileStore for FsFileStore {
    fn read(&self, path: &Path) -> Result<String, GettextError> {
        let content = fs::read_to_string(path)?;
        Ok(strip_bom(&content).to_string())
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), GettextError> {
        atomic_write(path, content)
    }

    fn modified_time(&self, path: &Path) -> Result<SystemTime, GettextError> {
        let meta = fs::metadata(path)?;
        Ok(meta.modified()?)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Atomically write `content` to `target` using a temp-file + fsync + rename
/// sequence. On Unix, acquires a best-effort `flock(LOCK_EX | LOCK_NB)` on
/// the existing target for the duration of the write. If the lock is held by
/// another process, returns [`GettextError::FileLocked`].
fn atomic_write(target: &Path, content: &str) -> Result<(), GettextError> {
    let dir = target.parent().ok_or_else(|| {
        GettextError::InvalidPath(format!(
            "no parent directory for target path: {}",
            target.display()
        ))
    })?;

    let _lock_guard = acquire_flock(target)?;

    // Process-wide monotonic counter so concurrent writers in the same
    // process can't collide on the millisecond resolution of SystemTime.
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_name = format!(
        "{}{}-{}-{}{}",
        TMP_FILE_PREFIX,
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        seq,
        TMP_FILE_SUFFIX,
    );
    let tmp_path = dir.join(&tmp_name);

    let result = (|| -> Result<(), GettextError> {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        // `rename` is atomic on POSIX when source and target are on the same
        // filesystem — we always write the temp file in the same directory.
        fs::rename(&tmp_path, target)?;
        Ok(())
    })();

    if result.is_err() {
        // Best-effort cleanup of the temp file when write/rename fails.
        let _ = fs::remove_file(&tmp_path);
    }

    result
}

/// RAII guard for an `flock`-acquired file descriptor on Unix. The lock is
/// released automatically when the guard is dropped (the kernel releases
/// the lock when the underlying fd is closed).
#[cfg(unix)]
struct FlockGuard {
    _file: fs::File,
}

#[cfg(unix)]
fn acquire_flock(target: &Path) -> Result<Option<FlockGuard>, GettextError> {
    use std::os::unix::io::AsRawFd;

    if !target.exists() {
        return Ok(None);
    }

    let file = match fs::File::open(target) {
        Ok(f) => f,
        Err(e) => {
            warn!(
                "could not open {} for advisory lock: {} — proceeding without lock",
                target.display(),
                e
            );
            return Ok(None);
        }
    };

    let fd = file.as_raw_fd();
    // SAFETY: `flock` is a POSIX syscall; `fd` is valid because `file` is alive.
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.kind() == std::io::ErrorKind::WouldBlock {
            return Err(GettextError::FileLocked {
                path: target.to_path_buf(),
            });
        }
        warn!(
            "advisory flock unavailable for {}: {} — proceeding without lock",
            target.display(),
            errno
        );
        return Ok(None);
    }

    Ok(Some(FlockGuard { _file: file }))
}

#[cfg(not(unix))]
struct FlockGuard;

#[cfg(not(unix))]
fn acquire_flock(_target: &Path) -> Result<Option<FlockGuard>, GettextError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_orphan_tmps_removes_only_matching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let orphan = dir
            .path()
            .join(format!("{TMP_FILE_PREFIX}123-456{TMP_FILE_SUFFIX}"));
        let keep = dir.path().join("messages.po");
        std::fs::write(&orphan, b"junk").unwrap();
        std::fs::write(&keep, b"msgid \"\"\nmsgstr \"\"\n").unwrap();

        cleanup_orphan_tmps(dir.path());

        assert!(!orphan.exists(), "orphan temp file should be removed");
        assert!(keep.exists(), "unrelated .po file must be preserved");
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hello.po");
        let store = FsFileStore::new();

        store
            .write(&path, "msgid \"Hi\"\nmsgstr \"Hej\"\n")
            .unwrap();
        let content = store.read(&path).unwrap();
        assert!(content.contains("Hej"));
    }

    #[test]
    fn read_strips_leading_bom() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bom.po");
        let body = "msgid \"\"\nmsgstr \"\"\n";
        let with_bom = format!("\u{feff}{body}");
        std::fs::write(&path, with_bom.as_bytes()).unwrap();

        let store = FsFileStore::new();
        let read = store.read(&path).unwrap();
        assert!(!read.starts_with('\u{feff}'));
        assert_eq!(read, body);
    }

    #[test]
    fn atomic_write_leaves_no_orphan_tmp_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("clean.po");
        let store = FsFileStore::new();

        store.write(&path, "msgid \"a\"\nmsgstr \"b\"\n").unwrap();
        store.write(&path, "msgid \"c\"\nmsgstr \"d\"\n").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(TMP_FILE_SUFFIX))
            .collect();
        assert!(leftovers.is_empty(), "found orphan tmp files");
    }

    #[cfg(unix)]
    #[test]
    fn flock_blocks_concurrent_write() {
        use std::os::unix::io::AsRawFd;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("locked.po");
        let store = FsFileStore::new();

        store.write(&path, "initial").unwrap();

        let lock_file = fs::File::open(&path).unwrap();
        let fd = lock_file.as_raw_fd();
        // SAFETY: fd lives as long as lock_file.
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(ret, 0);

        let err = store.write(&path, "updated").unwrap_err();
        assert!(
            matches!(err, GettextError::FileLocked { .. }),
            "expected FileLocked, got: {err}"
        );

        // SAFETY: fd lives as long as lock_file.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        drop(lock_file);

        store.write(&path, "updated").unwrap();
        let content = store.read(&path).unwrap();
        assert_eq!(content, "updated");
    }

    #[test]
    fn modified_time_returns_recent_value() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("timed.po");
        let store = FsFileStore::new();

        store.write(&path, "content").unwrap();
        let mtime = store.modified_time(&path).unwrap();
        let elapsed = SystemTime::now().duration_since(mtime).unwrap();
        assert!(elapsed.as_secs() < 5);
    }

    #[test]
    fn exists_reflects_filesystem() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ex.po");
        let store = FsFileStore::new();
        assert!(!store.exists(&path));
        store.write(&path, "x").unwrap();
        assert!(store.exists(&path));
    }
}
