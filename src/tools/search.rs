//! Paginated entry-search tool.
//!
//! Tool: `search_keys`. Same case-insensitive substring search as
//! `list_translations` but with offset/batch_size pagination and a
//! `match_in` parameter that controls which fields are searched.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::model::MessageEntry;
use crate::service::GettextStoreManager;

const DEFAULT_BATCH: usize = 30;
const MAX_BATCH: usize = 100;

fn clamp_batch(size: Option<usize>) -> usize {
    size.unwrap_or(DEFAULT_BATCH).clamp(1, MAX_BATCH)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SearchKeysParams {
    /// Path to the .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Case-insensitive substring to match. Empty string returns all
    /// entries.
    pub pattern: String,
    /// Maximum entries per page (default 30, clamped to 1..=100).
    pub batch_size: Option<usize>,
    /// Number of entries to skip (default 0).
    pub offset: Option<usize>,
    /// Subset of fields to search. Defaults to all four. Unknown field
    /// names are silently ignored.
    pub match_in: Option<Vec<String>>,
}

pub(crate) async fn handle_search_keys(
    manager: &GettextStoreManager,
    params: SearchKeysParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let entries = store.list_all().await?;

    let fields = normalize_fields(params.match_in.as_deref());
    let needle = params.pattern.to_lowercase();
    let match_all = needle.is_empty();

    let filtered: Vec<&(String, Option<String>, MessageEntry)> = entries
        .iter()
        .filter(|(msgid, msgctxt, entry)| {
            if match_all {
                return true;
            }
            matches_in_fields(msgid, msgctxt.as_deref(), entry, &needle, &fields)
        })
        .collect();

    let total = filtered.len();
    let offset = params.offset.unwrap_or(0);
    let batch_size = clamp_batch(params.batch_size);

    let page: Vec<Value> = filtered
        .iter()
        .skip(offset)
        .take(batch_size)
        .map(|(msgid, msgctxt, entry)| {
            json!({
                "msgid": msgid,
                "msgctxt": msgctxt,
                "msgstr": entry.msgstr,
                "msgid_plural": entry.msgid_plural,
                "msgstr_plural": entry.msgstr_plural,
                "flags": entry.flags,
                "is_fuzzy": entry.is_fuzzy(),
                "is_translated": entry.is_translated(),
            })
        })
        .collect();

    let returned = page.len();
    Ok(json!({
        "entries": page,
        "total": total,
        "offset": offset,
        "batch_size": batch_size,
        "has_more": offset + returned < total,
    }))
}

#[derive(Debug, Clone, Copy)]
struct SearchFields {
    msgid: bool,
    msgstr: bool,
    msgctxt: bool,
    comment: bool,
}

fn normalize_fields(fields: Option<&[String]>) -> SearchFields {
    match fields {
        None => SearchFields {
            msgid: true,
            msgstr: true,
            msgctxt: true,
            comment: true,
        },
        Some(list) => {
            let mut out = SearchFields {
                msgid: false,
                msgstr: false,
                msgctxt: false,
                comment: false,
            };
            for f in list {
                match f.to_ascii_lowercase().as_str() {
                    "msgid" => out.msgid = true,
                    "msgstr" => out.msgstr = true,
                    "msgctxt" => out.msgctxt = true,
                    "comment" => out.comment = true,
                    _ => {}
                }
            }
            out
        }
    }
}

fn matches_in_fields(
    msgid: &str,
    msgctxt: Option<&str>,
    entry: &MessageEntry,
    needle: &str,
    fields: &SearchFields,
) -> bool {
    if fields.msgid {
        if msgid.to_lowercase().contains(needle) {
            return true;
        }
        if let Some(plural) = &entry.msgid_plural {
            if plural.to_lowercase().contains(needle) {
                return true;
            }
        }
    }
    if fields.msgstr
        && (entry.msgstr.to_lowercase().contains(needle)
            || entry
                .msgstr_plural
                .iter()
                .any(|s| s.to_lowercase().contains(needle)))
    {
        return true;
    }
    if fields.msgctxt {
        if let Some(ctx) = msgctxt {
            if ctx.to_lowercase().contains(needle) {
                return true;
            }
        }
    }
    if fields.comment {
        for c in entry
            .translator_comment
            .iter()
            .chain(entry.extracted_comment.iter())
        {
            if c.to_lowercase().contains(needle) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn make_manager(path: &std::path::Path) -> Arc<GettextStoreManager> {
        Arc::new(GettextStoreManager::new(Some(path.to_path_buf())))
    }

    #[tokio::test]
    async fn search_empty_pattern_returns_all() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("A", None, "a", None).await.unwrap();
        store.upsert("B", None, "b", None).await.unwrap();

        let result = handle_search_keys(
            &manager,
            SearchKeysParams {
                path: Some(path.to_str().unwrap().into()),
                pattern: String::new(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 2);
    }

    #[tokio::test]
    async fn search_case_insensitive_msgid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let result = handle_search_keys(
            &manager,
            SearchKeysParams {
                path: Some(path.to_str().unwrap().into()),
                pattern: "hello".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["entries"][0]["msgid"], "Hello");
    }

    #[tokio::test]
    async fn search_pagination() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        for i in 0..5 {
            store
                .upsert(&format!("key{i}"), None, &format!("v{i}"), None)
                .await
                .unwrap();
        }

        let result = handle_search_keys(
            &manager,
            SearchKeysParams {
                path: Some(path.to_str().unwrap().into()),
                pattern: "key".into(),
                batch_size: Some(2),
                offset: Some(2),
                match_in: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 5);
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);
        assert_eq!(result["offset"], 2);
        assert_eq!(result["has_more"], true);
    }

    #[tokio::test]
    async fn search_match_in_restricts_to_msgstr_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Salut", None).await.unwrap();
        store.upsert("Salut", None, "Hola", None).await.unwrap();

        // Searching "salut" in msgstr only returns first entry.
        let result = handle_search_keys(
            &manager,
            SearchKeysParams {
                path: Some(path.to_str().unwrap().into()),
                pattern: "salut".into(),
                match_in: Some(vec!["msgstr".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["entries"][0]["msgid"], "Hello");
    }

    #[tokio::test]
    async fn search_match_in_msgctxt() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Open", Some("menu"), "Ouvrir", None)
            .await
            .unwrap();
        store
            .upsert("Open", Some("button"), "Ouvrir", None)
            .await
            .unwrap();

        let result = handle_search_keys(
            &manager,
            SearchKeysParams {
                path: Some(path.to_str().unwrap().into()),
                pattern: "menu".into(),
                match_in: Some(vec!["msgctxt".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["entries"][0]["msgctxt"], "menu");
    }
}
