//! Glossary tool handlers.
//!
//! Tools:
//!
//! * `get_glossary` — return all preferred-translation terms for a
//!   `(source_locale, target_locale)` pair, optionally filtered by a
//!   case-insensitive substring matched against either the source term
//!   or the translation.
//! * `update_glossary` — upsert and/or delete terms for a locale pair.
//!   Reads the existing file (if any), applies the patch, and writes
//!   back through the [`FileStore`] so the write is atomic.
//!
//! The glossary file lives at `$GETTEXT_GLOSSARY_PATH`, falling back to
//! `glossary.json` in the current working directory. The path is
//! resolved at handler call time so a `cd` between tool invocations does
//! the obvious thing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::error::GettextError;
use crate::io::FileStore;
use crate::service::glossary as glossary_svc;
use crate::service::glossary::Glossary;
use crate::service::GettextStoreManager;

/// Environment variable that overrides the default glossary file path.
pub(crate) const GLOSSARY_PATH_ENV: &str = "GETTEXT_GLOSSARY_PATH";
/// Default glossary filename, relative to the process working directory.
pub(crate) const DEFAULT_GLOSSARY_FILENAME: &str = "glossary.json";

/// Resolve the glossary file path: `$GETTEXT_GLOSSARY_PATH` wins,
/// otherwise `./glossary.json` relative to the current working dir.
pub(crate) fn resolve_glossary_path() -> PathBuf {
    match std::env::var(GLOSSARY_PATH_ENV) {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => PathBuf::from(DEFAULT_GLOSSARY_FILENAME),
    }
}

/// Load the glossary at `path` via `store`. A missing file yields an
/// empty glossary (callers update_glossary then create it on write).
fn load_glossary(store: &dyn FileStore, path: &Path) -> Result<Glossary, GettextError> {
    if !store.exists(path) {
        return glossary_svc::parse_glossary(None);
    }
    let raw = store.read(path)?;
    glossary_svc::parse_glossary(Some(&raw))
}

/// Write the glossary back through `store`. Creates parent directories
/// if they are missing (mirrors what `FsFileStore` would need anyway).
fn save_glossary(
    store: &Arc<dyn FileStore>,
    path: &Path,
    data: &Glossary,
) -> Result<(), GettextError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !store.exists(parent) {
            std::fs::create_dir_all(parent)?;
        }
    }
    let json = glossary_svc::serialize_glossary(data)?;
    store.write(path, &json)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GetGlossaryParams {
    /// Source locale (e.g. `"en"`).
    pub source_locale: String,
    /// Target locale (e.g. `"fr"`).
    pub target_locale: String,
    /// Optional case-insensitive substring filter. Matches either the
    /// source term or its translation.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct UpdateGlossaryParams {
    /// Source locale (e.g. `"en"`).
    pub source_locale: String,
    /// Target locale (e.g. `"fr"`).
    pub target_locale: String,
    /// Term → translation pairs to insert or overwrite. Pass an empty
    /// map to perform only deletions.
    #[serde(default)]
    pub entries: BTreeMap<String, String>,
    /// Source terms to remove from the locale pair. Missing terms are
    /// silently ignored.
    #[serde(default)]
    pub delete: Option<Vec<String>>,
}

pub(crate) async fn handle_get_glossary(
    manager: &GettextStoreManager,
    params: GetGlossaryParams,
) -> Result<Value, GettextError> {
    if params.source_locale.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "source_locale must not be empty".into(),
        ));
    }
    if params.target_locale.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "target_locale must not be empty".into(),
        ));
    }

    let path = resolve_glossary_path();
    let file_store = manager.file_store().clone();
    let path_for_load = path.clone();
    let glossary =
        tokio::task::spawn_blocking(move || load_glossary(file_store.as_ref(), &path_for_load))
            .await
            .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    let entries = glossary_svc::get_entries(
        &glossary,
        &params.source_locale,
        &params.target_locale,
        params.filter.as_deref(),
    );
    let total = entries.len();

    Ok(json!({
        "source_locale": params.source_locale,
        "target_locale": params.target_locale,
        "entries": entries,
        "total": total,
        "path": path.to_string_lossy(),
    }))
}

pub(crate) async fn handle_update_glossary(
    manager: &GettextStoreManager,
    write_lock: &Mutex<()>,
    params: UpdateGlossaryParams,
) -> Result<Value, GettextError> {
    if params.source_locale.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "source_locale must not be empty".into(),
        ));
    }
    if params.target_locale.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "target_locale must not be empty".into(),
        ));
    }

    let path = resolve_glossary_path();
    let delete_list = params.delete.unwrap_or_default();
    let entries = params.entries;

    // Serialize all writes through the manager-owned lock so two MCP
    // clients can't race on a read-modify-write cycle.
    let _guard = write_lock.lock().await;

    let file_store = manager.file_store().clone();
    let source = params.source_locale.clone();
    let target = params.target_locale.clone();
    let path_for_task = path.clone();

    let (updated, deleted, total_entries_in_pair) =
        tokio::task::spawn_blocking(move || -> Result<(usize, usize, usize), GettextError> {
            let mut glossary = load_glossary(file_store.as_ref(), &path_for_task)?;
            let updated = glossary_svc::update_entries(&mut glossary, &source, &target, entries);
            let deleted = if delete_list.is_empty() {
                0
            } else {
                glossary_svc::delete_entries(&mut glossary, &source, &target, &delete_list)
            };
            save_glossary(&file_store, &path_for_task, &glossary)?;
            let total = glossary_svc::pair_total(&glossary, &source, &target);
            Ok((updated, deleted, total))
        })
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    Ok(json!({
        "updated": updated,
        "deleted": deleted,
        "total_entries_in_pair": total_entries_in_pair,
        "source_locale": params.source_locale,
        "target_locale": params.target_locale,
        "path": path.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex as StdMutex};

    /// `GETTEXT_GLOSSARY_PATH` is process-global state. Hold this mutex
    /// for the lifetime of any test that mutates it so parallel tests
    /// don't read each other's overrides.
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        _inner: std::sync::MutexGuard<'static, ()>,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(path: &Path) -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let previous = std::env::var(GLOSSARY_PATH_ENV).ok();
            std::env::set_var(GLOSSARY_PATH_ENV, path);
            Self {
                _inner: guard,
                previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var(GLOSSARY_PATH_ENV, v),
                None => std::env::remove_var(GLOSSARY_PATH_ENV),
            }
        }
    }

    fn make_manager(base: &Path) -> Arc<GettextStoreManager> {
        // Use a directory as the base so absolute paths under it are
        // accepted by `validate_path` (the glossary tool doesn't actually
        // call validate_path, but using a real manager is closer to
        // production wiring than a hand-rolled mock).
        Arc::new(GettextStoreManager::new(Some(base.to_path_buf())))
    }

    #[tokio::test]
    async fn get_glossary_missing_file_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let result = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total"], 0);
        assert!(result["entries"].as_object().unwrap().is_empty());
        assert_eq!(result["source_locale"], "en");
        assert_eq!(result["target_locale"], "fr");
    }

    #[tokio::test]
    async fn update_then_get_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let lock = Mutex::new(());

        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Paramètres".into());
        entries.insert("Cancel".into(), "Annuler".into());

        let update_result = handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                entries,
                delete: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(update_result["updated"], 2);
        assert_eq!(update_result["deleted"], 0);
        assert_eq!(update_result["total_entries_in_pair"], 2);
        assert!(glossary_path.exists(), "file must be written");

        let get_result = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(get_result["total"], 2);
        assert_eq!(get_result["entries"]["Settings"], "Paramètres");
        assert_eq!(get_result["entries"]["Cancel"], "Annuler");
    }

    #[tokio::test]
    async fn get_glossary_filter_matches_term_or_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let lock = Mutex::new(());

        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Einstellungen".into());
        entries.insert("Cancel".into(), "Abbrechen".into());
        entries.insert("Save".into(), "Speichern".into());

        handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "de".into(),
                entries,
                delete: None,
            },
        )
        .await
        .unwrap();

        // Term match.
        let res = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "de".into(),
                filter: Some("set".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(res["total"], 1);
        assert!(res["entries"]["Settings"].as_str().is_some());

        // Translation match (case-insensitive).
        let res = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "de".into(),
                filter: Some("ABBRECH".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(res["total"], 1);
        assert!(res["entries"]["Cancel"].as_str().is_some());
    }

    #[tokio::test]
    async fn update_glossary_deletes_terms() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let lock = Mutex::new(());

        let mut entries = BTreeMap::new();
        entries.insert("A".into(), "a".into());
        entries.insert("B".into(), "b".into());
        entries.insert("C".into(), "c".into());
        handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                entries,
                delete: None,
            },
        )
        .await
        .unwrap();

        let result = handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                entries: BTreeMap::new(),
                delete: Some(vec!["A".into(), "Missing".into()]),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["updated"], 0);
        assert_eq!(result["deleted"], 1);
        assert_eq!(result["total_entries_in_pair"], 2);

        let get = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(get["total"], 2);
        assert!(get["entries"].get("A").is_none());
    }

    #[tokio::test]
    async fn locale_pairs_are_isolated() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let lock = Mutex::new(());

        let mut en_fr = BTreeMap::new();
        en_fr.insert("Settings".into(), "Paramètres".into());
        handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                entries: en_fr,
                delete: None,
            },
        )
        .await
        .unwrap();

        let mut en_de = BTreeMap::new();
        en_de.insert("Settings".into(), "Einstellungen".into());
        handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "de".into(),
                entries: en_de,
                delete: None,
            },
        )
        .await
        .unwrap();

        let fr = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await
        .unwrap();
        let de = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "de".into(),
                filter: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(fr["entries"]["Settings"], "Paramètres");
        assert_eq!(de["entries"]["Settings"], "Einstellungen");
        assert_eq!(fr["total"], 1);
        assert_eq!(de["total"], 1);
    }

    #[tokio::test]
    async fn corrupt_glossary_json_surfaces_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        std::fs::write(&glossary_path, "{not valid json").unwrap();
        let _env = EnvGuard::set(&glossary_path);

        let manager = make_manager(dir.path());
        let res = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "en".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await;
        assert!(res.is_err(), "corrupt JSON must surface as an error");
    }

    #[tokio::test]
    async fn rejects_empty_locale() {
        let dir = tempfile::TempDir::new().unwrap();
        let glossary_path = dir.path().join("glossary.json");
        let _env = EnvGuard::set(&glossary_path);
        let manager = make_manager(dir.path());
        let lock = Mutex::new(());

        let res = handle_get_glossary(
            &manager,
            GetGlossaryParams {
                source_locale: "  ".into(),
                target_locale: "fr".into(),
                filter: None,
            },
        )
        .await;
        assert!(res.is_err());

        let res = handle_update_glossary(
            &manager,
            &lock,
            UpdateGlossaryParams {
                source_locale: "en".into(),
                target_locale: "".into(),
                entries: BTreeMap::new(),
                delete: None,
            },
        )
        .await;
        assert!(res.is_err());
    }
}
