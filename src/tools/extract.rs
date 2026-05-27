//! Tools that extract subsets of entries for human/agent review.
//!
//! Tools: `get_untranslated` (paginated, returns empty-msgstr or fuzzy
//! entries) and `get_stale` (paginated, returns obsolete `#~` entries).

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
pub struct GetUntranslatedParams {
    /// Path to the .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Maximum entries per page (default 30, clamped to 1..=100).
    pub batch_size: Option<usize>,
    /// Number of entries to skip (default 0).
    pub offset: Option<usize>,
    /// Include fuzzy entries even if their msgstr is non-empty (default true).
    pub include_fuzzy: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct GetStaleParams {
    pub path: Option<String>,
    pub batch_size: Option<usize>,
    pub offset: Option<usize>,
}

pub(crate) async fn handle_get_untranslated(
    manager: &GettextStoreManager,
    params: GetUntranslatedParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let metadata = store.metadata().await?;
    let language = metadata.get("Language").cloned().unwrap_or_default();
    let nplurals = parse_nplurals(metadata.get("Plural-Forms").map(String::as_str));
    let plural_categories = required_plural_categories(&language, nplurals);

    let include_fuzzy = params.include_fuzzy.unwrap_or(true);
    let entries = store.list_all().await?;

    let filtered: Vec<&(String, Option<String>, MessageEntry)> = entries
        .iter()
        .filter(|(_, _, entry)| {
            let empty = if entry.msgid_plural.is_some() {
                entry.msgstr_plural.is_empty()
                    || entry.msgstr_plural.iter().any(|s| s.is_empty())
            } else {
                entry.msgstr.is_empty()
            };
            empty || (include_fuzzy && entry.is_fuzzy())
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
            let needs = if entry.msgid_plural.is_some() {
                plural_categories.clone()
            } else {
                Vec::new()
            };
            json!({
                "msgid": msgid,
                "msgctxt": msgctxt,
                "msgid_plural": entry.msgid_plural,
                "comment": comment_text(entry),
                "source_locations": entry.source_locations,
                "is_fuzzy": entry.is_fuzzy(),
                "needs_plural_forms": needs,
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

pub(crate) async fn handle_get_stale(
    manager: &GettextStoreManager,
    params: GetStaleParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let obsolete = store.obsolete_entries().await?;
    let total = obsolete.len();
    let offset = params.offset.unwrap_or(0);
    let batch_size = clamp_batch(params.batch_size);

    let page: Vec<Value> = obsolete
        .iter()
        .skip(offset)
        .take(batch_size)
        .map(|entry| {
            json!({
                "msgid": entry.msgid,
                "msgctxt": entry.msgctxt,
                "msgstr": entry.msgstr,
                "msgid_plural": entry.msgid_plural,
                "msgstr_plural": entry.msgstr_plural,
                "comment": comment_text(entry),
                "flags": entry.flags,
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

fn comment_text(entry: &MessageEntry) -> Option<String> {
    let mut buf = String::new();
    for line in &entry.translator_comment {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    for line in &entry.extracted_comment {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

/// Extract the `nplurals=N;` integer from a `Plural-Forms` header.
fn parse_nplurals(plural_forms: Option<&str>) -> Option<usize> {
    let pf = plural_forms?;
    let needle = "nplurals=";
    let start = pf.find(needle)? + needle.len();
    let tail = &pf[start..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    if end == 0 {
        return None;
    }
    tail[..end].parse::<usize>().ok()
}

/// CLDR plural category names required for `language`. Falls back to a
/// short denylist of languages and finally to `["one", "other"]`. When
/// the file declares `nplurals` we use that to size the output (only the
/// number of categories matters for the schema; the names are still drawn
/// from the language table).
fn required_plural_categories(language: &str, nplurals: Option<usize>) -> Vec<String> {
    // Strip region suffix (`fr_FR`, `pt-BR`, etc.).
    let lang = language
        .split(['_', '-'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase();

    let by_name: Vec<&'static str> = match lang.as_str() {
        // East Asian: single form.
        "ja" | "ko" | "zh" | "vi" | "th" | "ms" | "id" => vec!["other"],
        // Slavic four-form.
        "uk" | "ru" | "pl" | "be" | "hr" | "sr" | "bs" => {
            vec!["one", "few", "many", "other"]
        }
        // Czech / Slovak four-form.
        "cs" | "sk" => vec!["one", "few", "many", "other"],
        // Romanian three-form.
        "ro" => vec!["one", "few", "other"],
        // Latvian three-form (zero, one, other).
        "lv" => vec!["zero", "one", "other"],
        // Arabic six-form.
        "ar" => vec!["zero", "one", "two", "few", "many", "other"],
        // Welsh six-form.
        "cy" => vec!["zero", "one", "two", "few", "many", "other"],
        // Irish five-form.
        "ga" => vec!["one", "two", "few", "many", "other"],
        // Lithuanian four-form.
        "lt" => vec!["one", "few", "many", "other"],
        // Germanic / Romance / and unknown locales — safe default.
        _ => vec!["one", "other"],
    };

    // If the file's Plural-Forms declares a different nplurals count,
    // trust the file: pad with "other" or truncate.
    match nplurals {
        Some(n) if n != by_name.len() => {
            let mut out: Vec<String> = by_name.iter().map(|s| s.to_string()).collect();
            if n < out.len() {
                out.truncate(n);
            } else {
                while out.len() < n {
                    out.push("other".into());
                }
            }
            out
        }
        _ => by_name.iter().map(|s| s.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn make_manager(path: &std::path::Path) -> Arc<GettextStoreManager> {
        Arc::new(GettextStoreManager::new(Some(path.to_path_buf())))
    }

    #[test]
    fn nplurals_parsing() {
        assert_eq!(parse_nplurals(None), None);
        assert_eq!(parse_nplurals(Some("nplurals=2; plural=(n != 1);")), Some(2));
        assert_eq!(
            parse_nplurals(Some("nplurals=4; plural=(n==1 ? 0 : n);")),
            Some(4)
        );
        assert_eq!(parse_nplurals(Some("nope")), None);
    }

    #[test]
    fn plural_categories_known_languages() {
        assert_eq!(required_plural_categories("fr", Some(2)), vec!["one", "other"]);
        assert_eq!(
            required_plural_categories("uk", Some(4)),
            vec!["one", "few", "many", "other"]
        );
        assert_eq!(required_plural_categories("ja", Some(1)), vec!["other"]);
        // Unknown locale defaults to one/other.
        assert_eq!(
            required_plural_categories("xx", Some(2)),
            vec!["one", "other"]
        );
        // Region suffix is stripped.
        assert_eq!(
            required_plural_categories("pt-BR", Some(2)),
            vec!["one", "other"]
        );
    }

    #[test]
    fn plural_categories_nplurals_overrides_count() {
        // Header says 3 forms but language table only has 2 — should pad.
        let cats = required_plural_categories("fr", Some(3));
        assert_eq!(cats.len(), 3);
        assert_eq!(cats.last().unwrap(), "other");
    }

    #[tokio::test]
    async fn untranslated_returns_empty_and_fuzzy() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Translated", None, "Traduit", None).await.unwrap();
        store.upsert("Empty", None, "", None).await.unwrap();
        store
            .upsert("Fuzzy", None, "Flou", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();

        let result = handle_get_untranslated(
            &manager,
            GetUntranslatedParams {
                path: Some(path.to_str().unwrap().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total"], 2);
        let entries = result["entries"].as_array().unwrap();
        let msgids: Vec<&str> = entries.iter().map(|e| e["msgid"].as_str().unwrap()).collect();
        assert!(msgids.contains(&"Empty"));
        assert!(msgids.contains(&"Fuzzy"));
        assert!(!msgids.contains(&"Translated"));
    }

    #[tokio::test]
    async fn untranslated_pagination_and_has_more() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        for i in 0..5 {
            store.upsert(&format!("k{i}"), None, "", None).await.unwrap();
        }

        let result = handle_get_untranslated(
            &manager,
            GetUntranslatedParams {
                path: Some(path.to_str().unwrap().into()),
                batch_size: Some(2),
                offset: Some(0),
                include_fuzzy: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total"], 5);
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);
        assert_eq!(result["has_more"], true);

        let result = handle_get_untranslated(
            &manager,
            GetUntranslatedParams {
                path: Some(path.to_str().unwrap().into()),
                batch_size: Some(2),
                offset: Some(4),
                include_fuzzy: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["entries"].as_array().unwrap().len(), 1);
        assert_eq!(result["has_more"], false);
    }

    #[tokio::test]
    async fn untranslated_include_fuzzy_false_omits_fuzzy() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Fuzzy", None, "Flou", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();
        store.upsert("Empty", None, "", None).await.unwrap();

        let result = handle_get_untranslated(
            &manager,
            GetUntranslatedParams {
                path: Some(path.to_str().unwrap().into()),
                include_fuzzy: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total"], 1);
        assert_eq!(result["entries"][0]["msgid"], "Empty");
    }

    #[tokio::test]
    async fn untranslated_needs_plural_forms_from_language() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "uk").await.unwrap();
        store
            .set_header(
                "Plural-Forms",
                "nplurals=4; plural=(n%10==1 && n%100!=11 ? 0 : ...);",
            )
            .await
            .unwrap();
        store
            .upsert_full("%d file", None, "", Some("%d files"), Some(vec![]), None)
            .await
            .unwrap();

        let result = handle_get_untranslated(
            &manager,
            GetUntranslatedParams {
                path: Some(path.to_str().unwrap().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let entries = result["entries"].as_array().unwrap();
        let needs = entries[0]["needs_plural_forms"].as_array().unwrap();
        let names: Vec<&str> = needs.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(names, vec!["one", "few", "many", "other"]);
    }

    #[tokio::test]
    async fn stale_returns_obsolete_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\"Language: fr\\n\"\n\n\
             msgid \"Active\"\nmsgstr \"Actif\"\n\n\
             #~ msgid \"Removed\"\n\
             #~ msgstr \"Supprimé\"\n\n\
             #~ msgid \"AlsoOld\"\n\
             #~ msgstr \"Aussi vieux\"\n",
        )
        .unwrap();

        let manager = make_manager(&path).await;
        let result = handle_get_stale(
            &manager,
            GetStaleParams {
                path: Some(path.to_str().unwrap().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(result["total"], 2);
        let entries = result["entries"].as_array().unwrap();
        let msgids: Vec<&str> = entries.iter().map(|e| e["msgid"].as_str().unwrap()).collect();
        assert!(msgids.contains(&"Removed"));
        assert!(msgids.contains(&"AlsoOld"));
    }

    #[tokio::test]
    async fn stale_pagination() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let mut content = String::from("msgid \"\"\nmsgstr \"\"\n\n");
        for i in 0..3 {
            content.push_str(&format!("#~ msgid \"old{i}\"\n#~ msgstr \"vieux{i}\"\n\n"));
        }
        std::fs::write(&path, content).unwrap();

        let manager = make_manager(&path).await;
        let result = handle_get_stale(
            &manager,
            GetStaleParams {
                path: Some(path.to_str().unwrap().into()),
                batch_size: Some(2),
                offset: Some(0),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 3);
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);
        assert_eq!(result["has_more"], true);
    }

    #[tokio::test]
    async fn stale_empty_when_no_obsolete() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hi", None, "Salut", None).await.unwrap();

        let result = handle_get_stale(
            &manager,
            GetStaleParams {
                path: Some(path.to_str().unwrap().into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result["total"], 0);
        assert!(result["entries"].as_array().unwrap().is_empty());
    }
}
