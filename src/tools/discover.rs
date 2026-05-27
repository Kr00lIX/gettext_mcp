//! File-discovery and context-listing tool handlers.
//!
//! Tools: `list_files`, `list_contexts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListFilesParams {}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListContextsParams {
    pub path: Option<String>,
}

pub(crate) async fn handle_list_files(
    manager: &GettextStoreManager,
) -> Result<Value, GettextError> {
    let paths = manager.discovered_paths().await;
    let base_dir = manager.base_dir();

    let files: Vec<Value> = paths
        .iter()
        .map(|p| {
            let relative = base_dir
                .and_then(|base| p.strip_prefix(base).ok())
                .map(|r| r.to_string_lossy().to_string());
            json!({
                "path": p.to_string_lossy(),
                "relative_path": relative,
            })
        })
        .collect();

    Ok(json!({
        "files": files,
        "count": files.len(),
        "base_dir": base_dir.map(|p| p.to_string_lossy().to_string()),
    }))
}

pub(crate) async fn handle_list_contexts(
    manager: &GettextStoreManager,
    params: ListContextsParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let entries = store.list_all().await?;

    let mut contexts: Vec<String> = entries
        .iter()
        .filter_map(|(_, msgctxt, _)| msgctxt.clone())
        .collect();
    contexts.sort();
    contexts.dedup();

    Ok(json!(contexts))
}
