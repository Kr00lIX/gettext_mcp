//! PO-header tool handlers.
//!
//! Tools: `list_metadata`, `set_header`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListMetadataParams {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SetHeaderParams {
    pub path: Option<String>,
    pub key: String,
    pub value: Option<String>,
}

pub(crate) async fn handle_list_metadata(
    manager: &GettextStoreManager,
    params: ListMetadataParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let metadata = store.metadata().await?;
    let language = metadata.get("Language").cloned();

    Ok(json!({
        "metadata": metadata,
        "language": language,
    }))
}

pub(crate) async fn handle_set_header(
    manager: &GettextStoreManager,
    params: SetHeaderParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;

    if let Some(value) = params.value.as_deref() {
        store.set_header(&params.key, value).await?;
    } else {
        store.remove_header(&params.key).await?;
    }

    Ok(json!({
        "success": true,
        "key": params.key,
        "value": params.value,
    }))
}
