//! Entry-metadata tool handlers: comments and flags.
//!
//! Tools: `set_comment`, `set_fuzzy`, `set_flag`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetCommentParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetFuzzyParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub fuzzy: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetFlagParams {
    pub path: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub flag: String,
    pub enabled: bool,
}

pub(crate) async fn handle_set_comment(
    manager: &GettextStoreManager,
    params: SetCommentParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let mut entry = store.get(&params.msgid, params.msgctxt.as_deref()).await?;

    if let Some(comment) = &params.comment {
        entry.translator_comment = comment.lines().map(|l| l.to_string()).collect();
    } else {
        entry.translator_comment.clear();
    }

    store
        .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
        .await?;

    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "comment": params.comment,
    }))
}

pub(crate) async fn handle_set_fuzzy(
    manager: &GettextStoreManager,
    params: SetFuzzyParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let mut entry = store.get(&params.msgid, params.msgctxt.as_deref()).await?;

    if params.fuzzy {
        if !entry.flags.contains(&"fuzzy".to_string()) {
            entry.flags.push("fuzzy".to_string());
        }
    } else {
        entry.flags.retain(|f| f != "fuzzy");
    }

    store
        .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
        .await?;

    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "fuzzy": params.fuzzy,
    }))
}

pub(crate) async fn handle_set_flag(
    manager: &GettextStoreManager,
    params: SetFlagParams,
) -> Result<Value, GettextError> {
    if params.flag.is_empty()
        || !params
            .flag
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(GettextError::InvalidInput(
            "Invalid flag: only alphanumeric characters, hyphens, and underscores are allowed"
                .into(),
        ));
    }

    let store = manager.store_for(params.path.as_deref()).await?;
    let mut entry = store.get(&params.msgid, params.msgctxt.as_deref()).await?;

    if params.enabled {
        if !entry.flags.contains(&params.flag) {
            entry.flags.push(params.flag.clone());
        }
    } else {
        entry.flags.retain(|f| f != &params.flag);
    }

    store
        .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
        .await?;

    Ok(json!({
        "success": true,
        "msgid": params.msgid,
        "flag": params.flag,
        "enabled": params.enabled,
    }))
}
