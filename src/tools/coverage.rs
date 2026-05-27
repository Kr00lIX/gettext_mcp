//! Coverage statistics tool handler.
//!
//! Tool: `get_coverage`. Reports how many entries are translated,
//! untranslated, fuzzy, and obsolete for a single PO file. Fuzzy entries
//! that also have a non-empty msgstr are counted as both fuzzy AND
//! translated (the translation exists, it's just flagged for review).
//! Obsolete (`#~`) entries are excluded from `total_entries` and the
//! percentage calculations — they're reported in their own field.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GetCoverageParams {
    /// Path to the .po file (required in directory/dynamic mode).
    pub path: Option<String>,
}

pub(crate) async fn handle_get_coverage(
    manager: &GettextStoreManager,
    params: GetCoverageParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let entries = store.list_all().await?;
    let metadata = store.metadata().await?;
    let language = metadata.get("Language").cloned().unwrap_or_default();
    let obsolete = store.obsolete_entries().await?;

    let total_entries = entries.len();
    let plural_entries = entries
        .iter()
        .filter(|(_, _, e)| e.msgid_plural.is_some())
        .count();

    let mut translated = 0usize;
    let mut untranslated = 0usize;
    let mut fuzzy = 0usize;

    for (_, _, entry) in &entries {
        let has_msgstr = if entry.msgid_plural.is_some() {
            !entry.msgstr_plural.is_empty() && entry.msgstr_plural.iter().all(|s| !s.is_empty())
        } else {
            !entry.msgstr.is_empty()
        };
        let is_fuzzy = entry.is_fuzzy();

        if is_fuzzy {
            fuzzy += 1;
        }

        // Fuzzy entries with non-empty msgstr count as both fuzzy AND
        // translated. An entry is "untranslated" only when the msgstr is
        // genuinely empty (regardless of the fuzzy flag).
        if has_msgstr {
            translated += 1;
        } else {
            untranslated += 1;
        }
    }

    let (translated_percentage, fuzzy_percentage) = if total_entries == 0 {
        (0.0, 0.0)
    } else {
        let total_f = total_entries as f64;
        (
            (translated as f64 / total_f * 1000.0).round() / 10.0,
            (fuzzy as f64 / total_f * 1000.0).round() / 10.0,
        )
    };

    let path = store.path().to_string_lossy().to_string();

    Ok(json!({
        "path": path,
        "language": language,
        "total_entries": total_entries,
        "translated": translated,
        "untranslated": untranslated,
        "fuzzy": fuzzy,
        "obsolete": obsolete.len(),
        "plural_entries": plural_entries,
        "translated_percentage": translated_percentage,
        "fuzzy_percentage": fuzzy_percentage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn make_manager(path: &std::path::Path) -> Arc<GettextStoreManager> {
        Arc::new(GettextStoreManager::new(Some(path.to_path_buf())))
    }

    #[tokio::test]
    async fn coverage_counts_translated_and_untranslated() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();
        store.upsert("Untranslated", None, "", None).await.unwrap();

        let result = handle_get_coverage(
            &manager,
            GetCoverageParams {
                path: Some(path.to_str().unwrap().into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["language"], "fr");
        assert_eq!(result["total_entries"], 3);
        assert_eq!(result["translated"], 2);
        assert_eq!(result["untranslated"], 1);
        assert_eq!(result["fuzzy"], 0);
    }

    #[tokio::test]
    async fn coverage_fuzzy_counts_as_translated_when_msgstr_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();

        store
            .upsert("Hello", None, "Bonjour", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let result = handle_get_coverage(
            &manager,
            GetCoverageParams {
                path: Some(path.to_str().unwrap().into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total_entries"], 2);
        assert_eq!(result["translated"], 2);
        assert_eq!(result["fuzzy"], 1);
        assert_eq!(result["untranslated"], 0);
    }

    #[tokio::test]
    async fn coverage_reports_plural_and_obsolete() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\"Language: fr\\n\"\n\n\
             msgid \"%d cat\"\n\
             msgid_plural \"%d cats\"\n\
             msgstr[0] \"%d chat\"\n\
             msgstr[1] \"%d chats\"\n\n\
             #~ msgid \"Old\"\n\
             #~ msgstr \"Ancien\"\n",
        )
        .unwrap();

        let manager = make_manager(&path).await;
        let result = handle_get_coverage(
            &manager,
            GetCoverageParams {
                path: Some(path.to_str().unwrap().into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["plural_entries"], 1);
        assert_eq!(result["obsolete"], 1);
        assert_eq!(result["translated"], 1);
    }

    #[tokio::test]
    async fn coverage_percentage_is_zero_for_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let _ = manager.store_for(None).await.unwrap();

        let result = handle_get_coverage(
            &manager,
            GetCoverageParams {
                path: Some(path.to_str().unwrap().into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total_entries"], 0);
        assert_eq!(result["translated_percentage"], 0.0);
        assert_eq!(result["fuzzy_percentage"], 0.0);
    }

    #[tokio::test]
    async fn coverage_language_empty_when_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hi", None, "Salut", None).await.unwrap();

        let result = handle_get_coverage(
            &manager,
            GetCoverageParams {
                path: Some(path.to_str().unwrap().into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["language"], "");
    }
}
