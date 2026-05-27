//! CRUD layer over a single PO file.
//!
//! [`GettextStore`] keeps the parsed [`GettextFile`] in memory behind an
//! `RwLock` and persists every mutation through the [`FileStore`]
//! abstraction. All file I/O is done on `tokio::task::spawn_blocking` so
//! we never stall the async runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use indexmap::IndexMap;
use tokio::sync::RwLock;

use crate::error::GettextError;
use crate::io::FileStore;
use crate::model::{GettextFile, MessageEntry};
use crate::service::{parser, serializer};

/// Best-effort modified-time lookup via the [`FileStore`]. Returns `None`
/// when the file is missing or the platform doesn't report a mtime —
/// callers treat that as "no observable change".
fn file_mtime(store: &dyn FileStore, path: &Path) -> Option<SystemTime> {
    store.modified_time(path).ok()
}

/// In-memory store wrapping a single PO file. All public methods are
/// `async` because writes are dispatched to a blocking thread.
pub struct GettextStore {
    path: PathBuf,
    file_store: Arc<dyn FileStore>,
    data: Arc<RwLock<GettextFile>>,
    /// On-disk mtime captured the last time this store read or wrote the
    /// file. Used by [`super::manager::GettextStoreManager`] to detect
    /// external modifications and invalidate the cache.
    loaded_mtime: Arc<RwLock<Option<SystemTime>>>,
}

impl GettextStore {
    /// Load (or create empty) a store backed by `path` via `file_store`.
    pub async fn new(
        path: impl Into<PathBuf>,
        file_store: Arc<dyn FileStore>,
    ) -> Result<Self, GettextError> {
        let path = path.into();

        let (data, loaded_mtime) = if file_store.exists(&path) {
            let path_clone = path.clone();
            let fs_clone = Arc::clone(&file_store);
            let (content, mtime) = tokio::task::spawn_blocking(move || {
                let content = fs_clone.read(&path_clone)?;
                // Capture mtime AFTER reading so a concurrent writer that
                // lands between read and stat is detected on the next
                // access.
                let mtime = fs_clone.modified_time(&path_clone).ok();
                Ok::<(String, Option<SystemTime>), GettextError>((content, mtime))
            })
            .await
            .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;
            (parser::parse_po(&content)?, mtime)
        } else {
            (GettextFile::new(), None)
        };

        Ok(Self {
            path,
            file_store,
            data: Arc::new(RwLock::new(data)),
            loaded_mtime: Arc::new(RwLock::new(loaded_mtime)),
        })
    }

    /// Path backing this store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// mtime observed last read or write, if any.
    pub async fn loaded_mtime(&self) -> Option<SystemTime> {
        *self.loaded_mtime.read().await
    }

    /// Get a translation entry by `(msgid, msgctxt)`.
    pub async fn get(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
    ) -> Result<MessageEntry, GettextError> {
        let data = self.data.read().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));
        data.entries
            .get(&key)
            .cloned()
            .ok_or_else(|| GettextError::TranslationNotFound {
                key: msgid.to_string(),
                context: msgctxt.map(|s| s.to_string()),
            })
    }

    /// List all entries except the header (empty-msgid, no-context).
    pub async fn list_all(
        &self,
    ) -> Result<Vec<(String, Option<String>, MessageEntry)>, GettextError> {
        let data = self.data.read().await;
        Ok(data
            .entries
            .iter()
            .filter(|((msgid, msgctxt), _)| !(msgid.is_empty() && msgctxt.is_none()))
            .map(|((msgid, msgctxt), entry)| (msgid.clone(), msgctxt.clone(), entry.clone()))
            .collect())
    }

    /// Case-insensitive substring search across msgid + msgstr.
    pub async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MessageEntry>, GettextError> {
        let data = self.data.read().await;
        let query_lower = query.to_lowercase();

        let mut results: Vec<_> = data
            .entries
            .iter()
            .filter(|((msgid, _), entry)| {
                msgid.to_lowercase().contains(&query_lower)
                    || entry.msgstr.to_lowercase().contains(&query_lower)
            })
            .map(|(_, entry)| entry.clone())
            .collect();

        if let Some(limit) = limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Insert or update a translation. Only `msgstr` and (optionally)
    /// `flags` are touched — everything else is preserved.
    pub async fn upsert(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
        msgstr: &str,
        flags: Option<Vec<String>>,
    ) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        let entry = data.entries.entry(key).or_insert_with(|| MessageEntry {
            msgid: msgid.to_string(),
            msgctxt: msgctxt.map(|s| s.to_string()),
            ..Default::default()
        });

        entry.msgstr = msgstr.to_string();
        if let Some(flags) = flags {
            entry.flags = flags;
        }

        self.persist(&data).await?;
        Ok(())
    }

    /// Extended upsert that also handles plural forms and validates flags.
    pub async fn upsert_full(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
        msgstr: &str,
        msgid_plural: Option<&str>,
        msgstr_plural: Option<Vec<String>>,
        flags: Option<Vec<String>>,
    ) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        if let Some(ref flags) = flags {
            for flag in flags {
                if flag.is_empty()
                    || !flag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(GettextError::InvalidInput(format!(
                        "Invalid flag '{}': only alphanumeric characters, hyphens, and underscores are allowed",
                        flag
                    )));
                }
            }
        }

        let entry = data.entries.entry(key).or_insert_with(|| MessageEntry {
            msgid: msgid.to_string(),
            msgctxt: msgctxt.map(|s| s.to_string()),
            ..Default::default()
        });

        entry.msgstr = msgstr.to_string();
        if let Some(plural) = msgid_plural {
            entry.msgid_plural = Some(plural.to_string());
            entry.msgstr_plural = msgstr_plural.unwrap_or_default();
        }
        if let Some(flags) = flags {
            entry.flags = flags;
        }

        self.persist(&data).await?;
        Ok(())
    }

    /// Replace an entry wholesale while keeping the key intact. Used by
    /// the MCP layer to preserve comments/flags when toggling metadata.
    pub async fn update_entry(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
        mut entry: MessageEntry,
    ) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));
        // Ensure entry fields match the key to prevent inconsistency.
        entry.msgid = msgid.to_string();
        entry.msgctxt = msgctxt.map(|s| s.to_string());
        data.entries.insert(key, entry);
        self.persist(&data).await?;
        Ok(())
    }

    /// Delete a single `(msgid, msgctxt)` entry.
    pub async fn delete(&self, msgid: &str, msgctxt: Option<&str>) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));
        if data.entries.shift_remove(&key).is_none() {
            return Err(GettextError::TranslationNotFound {
                key: msgid.to_string(),
                context: msgctxt.map(|s| s.to_string()),
            });
        }
        self.persist(&data).await?;
        Ok(())
    }

    /// Delete every entry that uses the given `msgid` across every
    /// context. Returns the number of entries removed.
    pub async fn delete_by_msgid(&self, msgid: &str) -> Result<usize, GettextError> {
        let mut data = self.data.write().await;
        let keys_to_remove: Vec<_> = data
            .entries
            .keys()
            .filter(|(id, _)| id == msgid)
            .cloned()
            .collect();

        let count = keys_to_remove.len();
        if count == 0 {
            return Err(GettextError::TranslationNotFound {
                key: msgid.to_string(),
                context: None,
            });
        }

        for key in keys_to_remove {
            data.entries.shift_remove(&key);
        }

        self.persist(&data).await?;
        Ok(count)
    }

    /// Persist the in-memory file to disk. Called from every mutator.
    async fn persist(&self, data: &GettextFile) -> Result<(), GettextError> {
        let content = serializer::serialize_po(data);
        let path = self.path.clone();
        let fs_clone = Arc::clone(&self.file_store);
        tokio::task::spawn_blocking(move || fs_clone.write(&path, &content))
            .await
            .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

        // Refresh our recorded mtime to what we just wrote. This prevents
        // the manager's staleness check from re-reading our own write.
        let new_mtime =
            file_mtime(self.file_store.as_ref(), &self.path).unwrap_or_else(SystemTime::now);
        *self.loaded_mtime.write().await = Some(new_mtime);
        Ok(())
    }

    /// Header metadata snapshot.
    pub async fn metadata(&self) -> Result<IndexMap<String, String>, GettextError> {
        let data = self.data.read().await;
        Ok(data.metadata.clone())
    }

    /// `Language` header, if set.
    pub async fn language(&self) -> Result<Option<String>, GettextError> {
        let data = self.data.read().await;
        Ok(data.language())
    }

    /// Set or update a single header. Rejects keys with newlines/colons
    /// and values with newlines.
    pub async fn set_header(&self, key: &str, value: &str) -> Result<(), GettextError> {
        if key.is_empty() || key.trim().is_empty() {
            return Err(GettextError::InvalidInput(
                "Header key must not be empty".into(),
            ));
        }
        if key.contains('\n') || key.contains('\r') {
            return Err(GettextError::InvalidInput(
                "Header key must not contain newlines".into(),
            ));
        }
        if key.contains(':') {
            return Err(GettextError::InvalidInput(
                "Header key must not contain colons".into(),
            ));
        }
        if value.contains('\n') || value.contains('\r') {
            return Err(GettextError::InvalidInput(
                "Header value must not contain newlines".into(),
            ));
        }
        let mut data = self.data.write().await;
        data.metadata.insert(key.to_string(), value.to_string());
        data.rebuild_header_entry();
        self.persist(&data).await?;
        Ok(())
    }

    /// Remove a header. Silently no-ops if the header was not set.
    pub async fn remove_header(&self, key: &str) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        data.metadata.shift_remove(key);
        data.rebuild_header_entry();
        self.persist(&data).await?;
        Ok(())
    }

    /// Convenience: report the languages this PO file declares. PO files
    /// typically store exactly one language, so this returns at most one
    /// element.
    pub async fn list_languages(&self) -> Result<Vec<String>, GettextError> {
        let data = self.data.read().await;
        let mut languages = Vec::new();
        if let Some(lang) = data.language() {
            languages.push(lang);
        }
        Ok(languages)
    }

    /// Set the `Language` header (creating it if absent).
    pub async fn add_language(&self, language: &str) -> Result<(), GettextError> {
        self.set_header("Language", language).await
    }

    /// Remove the `Language` header, but only if it currently matches the
    /// supplied value. Mismatches return [`GettextError::InvalidInput`].
    pub async fn remove_language(&self, language: &str) -> Result<(), GettextError> {
        let mut data = self.data.write().await;
        if data.metadata.get("Language").map(|l| l.as_str()) == Some(language) {
            data.metadata.shift_remove("Language");
            data.rebuild_header_entry();
            self.persist(&data).await?;
            Ok(())
        } else {
            Err(GettextError::InvalidInput(format!(
                "Language '{}' does not match current file language",
                language
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::FsFileStore;

    fn fs() -> Arc<dyn FileStore> {
        Arc::new(FsFileStore::new())
    }

    #[tokio::test]
    async fn upsert_and_get() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Bonjour");
    }

    #[tokio::test]
    async fn search_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let results = store.search("Hello", None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].msgstr, "Bonjour");
    }

    #[tokio::test]
    async fn get_nonexistent_returns_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        let result = store.get("nope", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_then_get_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();
        store.delete("Hello", None).await.unwrap();

        assert!(store.get("Hello", None).await.is_err());
        let world = store.get("World", None).await.unwrap();
        assert_eq!(world.msgstr, "Monde");
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        assert!(store.delete("missing", None).await.is_err());
    }

    #[tokio::test]
    async fn delete_by_msgid_clears_all_contexts() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        store
            .upsert("Save", Some("menu"), "Enregistrer", None)
            .await
            .unwrap();
        store
            .upsert("Save", Some("toolbar"), "Sauvegarder", None)
            .await
            .unwrap();
        store.upsert("Other", None, "Autre", None).await.unwrap();

        let count = store.delete_by_msgid("Save").await.unwrap();
        assert_eq!(count, 2);
        assert!(store.get("Save", Some("menu")).await.is_err());
        assert_eq!(store.get("Other", None).await.unwrap().msgstr, "Autre");
    }

    #[tokio::test]
    async fn update_entry_preserves_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store
            .upsert("Hello", None, "Bonjour", Some(vec!["c-format".into()]))
            .await
            .unwrap();

        let mut entry = store.get("Hello", None).await.unwrap();
        entry.translator_comment = vec!["A greeting".into()];
        store.update_entry("Hello", None, entry).await.unwrap();

        let updated = store.get("Hello", None).await.unwrap();
        assert_eq!(updated.msgstr, "Bonjour");
        assert_eq!(updated.translator_comment, vec!["A greeting".to_string()]);
        assert!(updated.flags.contains(&"c-format".to_string()));
    }

    #[tokio::test]
    async fn set_header_rejects_newlines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        assert!(store.set_header("Bad\nKey", "value").await.is_err());
        assert!(store.set_header("Key", "bad\nvalue").await.is_err());
    }

    #[tokio::test]
    async fn set_header_rejects_colons_in_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        assert!(store.set_header("Bad:Key", "value").await.is_err());
    }

    #[tokio::test]
    async fn set_header_empty_key_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        assert!(store.set_header("", "value").await.is_err());
        assert!(store.set_header("   ", "value").await.is_err());
    }

    #[tokio::test]
    async fn upsert_full_rejects_invalid_flags() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        let result = store
            .upsert_full(
                "Hello",
                None,
                "Bonjour",
                None,
                None,
                Some(vec!["valid-flag".into(), "invalid flag!".into()]),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid flag"));
    }

    #[tokio::test]
    async fn header_and_metadata_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        store.set_header("Language", "es").await.unwrap();
        store
            .set_header("Plural-Forms", "nplurals=2; plural=(n != 1);")
            .await
            .unwrap();

        let meta = store.metadata().await.unwrap();
        assert_eq!(meta.get("Language"), Some(&"es".to_string()));
        assert_eq!(
            meta.get("Plural-Forms"),
            Some(&"nplurals=2; plural=(n != 1);".to_string())
        );
        assert_eq!(store.language().await.unwrap(), Some("es".to_string()));
    }

    #[tokio::test]
    async fn remove_header_drops_value() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        store.set_header("Language", "fr").await.unwrap();
        store.set_header("Custom-Key", "x").await.unwrap();
        store.remove_header("Custom-Key").await.unwrap();

        let meta = store.metadata().await.unwrap();
        assert!(meta.get("Custom-Key").is_none());
        assert_eq!(meta.get("Language"), Some(&"fr".to_string()));
    }

    #[tokio::test]
    async fn add_and_remove_language() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        store.add_language("ja").await.unwrap();
        assert_eq!(store.list_languages().await.unwrap(), vec!["ja"]);
        store.remove_language("ja").await.unwrap();
        assert!(store.list_languages().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn remove_wrong_language_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.add_language("fr").await.unwrap();
        assert!(store.remove_language("de").await.is_err());
    }

    #[tokio::test]
    async fn is_translated_semantics() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();

        store.upsert("Empty", None, "", None).await.unwrap();
        assert!(!store.get("Empty", None).await.unwrap().is_translated());

        store.upsert("Full", None, "Complet", None).await.unwrap();
        assert!(store.get("Full", None).await.unwrap().is_translated());

        store
            .upsert("Fuzzy", None, "Flou", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();
        let entry = store.get("Fuzzy", None).await.unwrap();
        assert!(entry.is_fuzzy());
        assert!(!entry.is_translated());
    }

    #[tokio::test]
    async fn search_is_case_insensitive() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store
            .upsert("Hello World", None, "Bonjour Monde", None)
            .await
            .unwrap();
        assert_eq!(store.search("hello", None).await.unwrap().len(), 1);
        assert_eq!(store.search("MONDE", None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn search_respects_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.upsert("Test A", None, "A", None).await.unwrap();
        store.upsert("Test B", None, "B", None).await.unwrap();
        store.upsert("Test C", None, "C", None).await.unwrap();
        let results = store.search("Test", Some(2)).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn upsert_full_with_plurals() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store
            .upsert_full(
                "%d cat",
                None,
                "",
                Some("%d cats"),
                Some(vec!["%d chat".into(), "%d chats".into()]),
                Some(vec!["c-format".into()]),
            )
            .await
            .unwrap();

        let entry = store.get("%d cat", None).await.unwrap();
        assert_eq!(entry.msgid_plural, Some("%d cats".to_string()));
        assert_eq!(entry.msgstr_plural, vec!["%d chat", "%d chats"]);
        assert!(entry.flags.contains(&"c-format".to_string()));
    }

    #[tokio::test]
    async fn list_all_excludes_header() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let entries = store.list_all().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "Hello");
    }

    #[tokio::test]
    async fn upsert_overwrites_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let store = GettextStore::new(&path, fs()).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("Hello", None, "Salut", None).await.unwrap();
        assert_eq!(store.get("Hello", None).await.unwrap().msgstr, "Salut");
    }

    #[tokio::test]
    async fn loading_preserves_bom_stripped_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bom.po");
        let body = "msgid \"\"\nmsgstr \"\"\n\"Language: en\\n\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n";
        let with_bom = format!("\u{feff}{body}");
        std::fs::write(&path, with_bom.as_bytes()).unwrap();

        let store = GettextStore::new(&path, fs()).await.unwrap();
        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Bonjour");
        assert!(!entry.msgid.contains('\u{feff}'));
    }
}
