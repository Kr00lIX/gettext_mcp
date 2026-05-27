//! Translation-entry CRUD tool handlers.
//!
//! Tools: `list_translations`, `get_translation`, `upsert_translation`,
//! `delete_translation`, `delete_key`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListTranslationsParams {
    /// Path to the .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Case-insensitive substring filter on msgid/msgstr/plurals.
    pub query: Option<String>,
    /// Maximum number of entries to return.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetTranslationParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UpsertTranslationParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub msgstr: Option<String>,
    pub msgid_plural: Option<String>,
    pub msgstr_plural: Option<Vec<String>>,
    pub flags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteTranslationParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeleteKeyParams {
    pub path: Option<String>,
    pub msgid: String,
}

pub(crate) async fn handle_list_translations(
    manager: &GettextStoreManager,
    params: ListTranslationsParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let entries = store.list_all().await?;

    let mut filtered: Vec<_> = entries
        .into_iter()
        .filter(|(msgid, _, entry)| match &params.query {
            None => true,
            Some(query) => {
                let q = query.to_lowercase();
                msgid.to_lowercase().contains(&q)
                    || entry.msgstr.to_lowercase().contains(&q)
                    || entry
                        .msgid_plural
                        .as_deref()
                        .is_some_and(|p| p.to_lowercase().contains(&q))
                    || entry
                        .msgstr_plural
                        .iter()
                        .any(|p| p.to_lowercase().contains(&q))
            }
        })
        .collect();

    if let Some(limit) = params.limit {
        filtered.truncate(limit);
    }

    let result: Vec<_> = filtered
        .into_iter()
        .map(|(msgid, msgctxt, entry)| {
            json!({
                "msgid": msgid,
                "msgctxt": msgctxt,
                "msgstr": entry.msgstr,
                "msgid_plural": entry.msgid_plural,
                "msgstr_plural": entry.msgstr_plural,
                "flags": entry.flags,
                "is_translated": entry.is_translated(),
                "is_fuzzy": entry.is_fuzzy(),
            })
        })
        .collect();

    Ok(json!(result))
}

pub(crate) async fn handle_get_translation(
    manager: &GettextStoreManager,
    params: GetTranslationParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let entry = store.get(&params.msgid, params.msgctxt.as_deref()).await?;

    Ok(json!({
        "msgid": params.msgid,
        "msgctxt": params.msgctxt,
        "msgstr": entry.msgstr,
        "msgid_plural": entry.msgid_plural,
        "msgstr_plural": entry.msgstr_plural,
        "flags": entry.flags,
        "extracted_comment": entry.extracted_comment,
        "translator_comment": entry.translator_comment,
        "source_locations": entry.source_locations,
        "is_translated": entry.is_translated(),
        "is_fuzzy": entry.is_fuzzy(),
    }))
}

pub(crate) async fn handle_upsert_translation(
    manager: &GettextStoreManager,
    params: UpsertTranslationParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let msgstr = params.msgstr.clone().unwrap_or_default();

    store
        .upsert_full(
            &params.msgid,
            params.msgctxt.as_deref(),
            &msgstr,
            params.msgid_plural.as_deref(),
            params.msgstr_plural,
            params.flags,
        )
        .await?;

    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "msgctxt": params.msgctxt,
        "msgstr": msgstr,
    }))
}

pub(crate) async fn handle_delete_translation(
    manager: &GettextStoreManager,
    params: DeleteTranslationParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    store
        .delete(&params.msgid, params.msgctxt.as_deref())
        .await?;
    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "msgctxt": params.msgctxt,
    }))
}

pub(crate) async fn handle_delete_key(
    manager: &GettextStoreManager,
    params: DeleteKeyParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let deleted_count = store.delete_by_msgid(&params.msgid).await?;
    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "deleted_count": deleted_count,
    }))
}
