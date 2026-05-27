//! Multi-file orchestrator for [`GettextStore`].
//!
//! The MCP server owns a single [`GettextStoreManager`] and routes every
//! tool call's `path` argument through [`store_for`]. The manager caches
//! per-path stores, validates paths against an optional base directory,
//! and reloads stores when the on-disk mtime advances ahead of what the
//! cached store recorded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use tokio::sync::RwLock;

use crate::error::GettextError;
use crate::io::FileStore;
use crate::service::store::GettextStore;

/// Manager owning the [`FileStore`] and a path-keyed cache of stores.
pub struct GettextStoreManager {
    default_path: Option<PathBuf>,
    file_store: Arc<dyn FileStore>,
    stores: Arc<RwLock<IndexMap<PathBuf, Arc<GettextStore>>>>,
}

impl GettextStoreManager {
    /// Construct a manager using [`crate::io::FsFileStore`] as the
    /// backing store. Same signature the rest of the crate (tests, web,
    /// CLI) has historically used.
    pub fn new(default_path: Option<PathBuf>) -> Self {
        Self::with_file_store(default_path, Arc::new(crate::io::FsFileStore::new()))
    }

    /// Construct a manager with a caller-supplied [`FileStore`].
    pub fn with_file_store(
        default_path: Option<PathBuf>,
        file_store: Arc<dyn FileStore>,
    ) -> Self {
        Self {
            default_path,
            file_store,
            stores: Arc::new(RwLock::new(IndexMap::new())),
        }
    }

    /// Recursively scan the default path (if it is a directory) for
    /// `.po`/`.pot` files, pre-load them, and return the count.
    pub async fn scan_directory(&self) -> Result<usize, GettextError> {
        let dir = match self.default_path {
            Some(ref p) if p.is_dir() => p.clone(),
            _ => return Ok(0),
        };

        let po_files = Self::find_po_files(&dir).await?;
        let count = po_files.len();

        let mut stores = self.stores.write().await;
        for file_path in po_files {
            if !stores.contains_key(&file_path) {
                let store = Arc::new(
                    GettextStore::new(&file_path, Arc::clone(&self.file_store)).await?,
                );
                stores.insert(file_path, store);
            }
        }

        Ok(count)
    }

    async fn find_po_files(dir: &Path) -> Result<Vec<PathBuf>, GettextError> {
        let mut result = Vec::new();
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&current).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(ext) = path.extension() {
                    if ext == "po" || ext == "pot" {
                        result.push(path);
                    }
                }
            }
        }

        result.sort();
        Ok(result)
    }

    /// Look up (or create) a store for the caller-supplied path. When
    /// `path` is `None` the default path is used (in single-file mode).
    pub async fn store_for(&self, path: Option<&str>) -> Result<Arc<GettextStore>, GettextError> {
        let path_buf = self.resolve_path(path)?;

        // Fast path: cache hit with no observed mtime drift.
        {
            let stores = self.stores.read().await;
            if let Some(store) = stores.get(&path_buf) {
                if !self.is_stale(store, &path_buf).await {
                    return Ok(Arc::clone(store));
                }
            }
        }

        // Slow path: cache miss OR a stale entry was evicted.
        let mut stores = self.stores.write().await;
        if let Some(store) = stores.get(&path_buf) {
            if !self.is_stale(store, &path_buf).await {
                return Ok(Arc::clone(store));
            }
            stores.shift_remove(&path_buf);
        }

        let store = Arc::new(GettextStore::new(&path_buf, Arc::clone(&self.file_store)).await?);
        stores.insert(path_buf, Arc::clone(&store));
        Ok(store)
    }

    /// Compare the cached store's recorded mtime with what's on disk now.
    /// File missing or stat failed → keep the cached copy (callers will
    /// surface real I/O errors on the next persist).
    async fn is_stale(&self, store: &Arc<GettextStore>, path: &Path) -> bool {
        let current_mtime = self.file_store.modified_time(path).ok();
        let cached_mtime = store.loaded_mtime().await;
        match (current_mtime, cached_mtime) {
            (Some(current), Some(cached)) => current != cached,
            // First-time observation of an mtime (cached has None but the
            // file now exists): treat as stale so we reload.
            (Some(_), None) => true,
            // File missing or stat failed: keep cached.
            (None, _) => false,
        }
    }

    fn resolve_path(&self, path: Option<&str>) -> Result<PathBuf, GettextError> {
        if let Some(p) = path {
            let pb = PathBuf::from(p);
            if pb.is_relative() {
                if let Some(ref base) = self.default_path {
                    if base.is_dir() {
                        let resolved = base.join(&pb);
                        self.validate_path(&resolved)?;
                        return Ok(resolved);
                    }
                }
                self.validate_path(&pb)?;
                Ok(pb)
            } else {
                self.validate_path(&pb)?;
                Ok(pb)
            }
        } else if let Some(ref p) = self.default_path {
            if p.is_dir() {
                Err(GettextError::PathRequired)
            } else {
                Ok(p.clone())
            }
        } else {
            Err(GettextError::PathRequired)
        }
    }

    pub(crate) fn validate_path(&self, path: &Path) -> Result<(), GettextError> {
        // Reject path traversal.
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                return Err(GettextError::InvalidPath(
                    "Path traversal not allowed".into(),
                ));
            }
        }

        if let Some(ref default) = self.default_path {
            // Use parent directory as base when default_path points to a file.
            let base = if default.is_dir() {
                default.as_path()
            } else {
                default.parent().unwrap_or(default)
            };

            let canonical_base = base.canonicalize().map_err(|e| {
                GettextError::InvalidPath(format!("Cannot resolve base path: {}", e))
            })?;
            let canonical_path = path.canonicalize().or_else(|_| {
                if let (Some(parent), Some(filename)) = (path.parent(), path.file_name()) {
                    parent.canonicalize().map(|p| p.join(filename))
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Cannot resolve path",
                    ))
                }
            })
            .map_err(|e| GettextError::InvalidPath(format!("Cannot resolve path: {}", e)))?;

            if !canonical_path.starts_with(&canonical_base) {
                return Err(GettextError::InvalidPath(
                    "Path must be within base directory".into(),
                ));
            }
        } else if path.is_absolute() {
            return Err(GettextError::InvalidPath(
                "Absolute paths not allowed without a configured base directory".into(),
            ));
        }

        Ok(())
    }

    /// Paths of every store currently loaded (including those preloaded
    /// by [`scan_directory`]).
    pub async fn discovered_paths(&self) -> Vec<PathBuf> {
        let stores = self.stores.read().await;
        stores.keys().cloned().collect()
    }

    /// Whether the configured default path is a directory (vs. a single
    /// file or `None`).
    pub fn is_directory_mode(&self) -> bool {
        self.default_path.as_ref().is_some_and(|p| p.is_dir())
    }

    /// Base directory path, if any.
    pub fn base_dir(&self) -> Option<&Path> {
        self.default_path.as_deref().filter(|p| p.is_dir())
    }

    /// Shared reference to the backing [`FileStore`]. Used by tools that
    /// read or write auxiliary files (e.g. XLIFF documents) so they go
    /// through the same I/O layer (atomic writes, advisory locking, BOM
    /// stripping) as the PO files themselves.
    pub fn file_store(&self) -> &Arc<dyn FileStore> {
        &self.file_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_rejects_traversal() {
        let manager = GettextStoreManager::new(None);
        let result = manager.validate_path(&PathBuf::from("../etc/passwd"));
        assert!(result.is_err());
    }

    #[test]
    fn validate_path_rejects_absolute_without_base() {
        let manager = GettextStoreManager::new(None);
        let result = manager.validate_path(&PathBuf::from("/etc/passwd"));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn store_for_requires_path_in_dynamic_mode() {
        let manager = GettextStoreManager::new(None);
        let result = manager.store_for(None).await;
        match result {
            Err(e) => assert!(e.to_string().contains("Path required")),
            Ok(_) => panic!("expected PathRequired"),
        }
    }

    #[tokio::test]
    async fn cache_invalidates_on_external_modification() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();
        assert_eq!(store1.get("Hello", None).await.unwrap().msgstr, "Bonjour");

        // Some filesystems have ~1s mtime resolution; sleep long enough to
        // get a distinct timestamp.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Salut\"\n",
        )
        .unwrap();

        let store2 = manager.store_for(None).await.unwrap();
        assert_eq!(store2.get("Hello", None).await.unwrap().msgstr, "Salut");
        assert!(
            !Arc::ptr_eq(&store1, &store2),
            "stale store should have been evicted"
        );
    }

    #[tokio::test]
    async fn cache_serves_unchanged_file_without_rereading() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();
        let store2 = manager.store_for(None).await.unwrap();
        assert!(Arc::ptr_eq(&store1, &store2));
    }

    #[tokio::test]
    async fn cache_survives_internal_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();
        store1
            .upsert("Greeting", None, "Salutation", None)
            .await
            .unwrap();

        let store2 = manager.store_for(None).await.unwrap();
        assert!(Arc::ptr_eq(&store1, &store2));
        assert_eq!(
            store2.get("Greeting", None).await.unwrap().msgstr,
            "Salutation"
        );
    }

}
