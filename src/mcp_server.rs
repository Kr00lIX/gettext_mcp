use crate::store::GettextStoreManager;
use rmcp::handler::server::tool::schema_for_type;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// MCP Server for Gettext PO files
pub struct GettextMcpServer {
    manager: Arc<GettextStoreManager>,
}

impl GettextMcpServer {
    pub fn new(manager: Arc<GettextStoreManager>) -> Self {
        Self { manager }
    }

    // ==================== CRUD Operations ====================

    /// List all translations with optional filtering
    pub async fn list_translations(
        &self,
        params: ListTranslationsParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let entries = store.list_all().await.map_err(|e| e.to_string())?;

        // Filter by query if provided
        let mut filtered: Vec<_> = entries
            .into_iter()
            .filter(|(msgid, _, entry)| {
                if let Some(ref query) = params.query {
                    let q_lower = query.to_lowercase();
                    msgid.to_lowercase().contains(&q_lower)
                        || entry.msgstr.to_lowercase().contains(&q_lower)
                        || entry.msgid_plural.as_deref().map_or(false, |p| p.to_lowercase().contains(&q_lower))
                        || entry.msgstr_plural.iter().any(|p| p.to_lowercase().contains(&q_lower))
                } else {
                    true
                }
            })
            .collect();

        // Apply limit
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

    /// Get a single translation entry
    pub async fn get_translation(
        &self,
        params: GetTranslationParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let entry = store
            .get(&params.msgid, params.msgctxt.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let result = json!({
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
        });

        Ok(result)
    }

    /// Create or update a translation
    pub async fn upsert_translation(
        &self,
        params: UpsertTranslationParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let msgstr = params.msgstr.unwrap_or_default();

        store
            .upsert_full(
                &params.msgid,
                params.msgctxt.as_deref(),
                &msgstr,
                params.msgid_plural.as_deref(),
                params.msgstr_plural,
                params.flags,
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "msgctxt": params.msgctxt,
            "msgstr": msgstr,
        }))
    }

    /// Delete a specific translation entry
    pub async fn delete_translation(
        &self,
        params: DeleteTranslationParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        store
            .delete(&params.msgid, params.msgctxt.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "msgctxt": params.msgctxt,
        }))
    }

    /// Delete all contexts of a msgid
    pub async fn delete_key(&self, params: DeleteKeyParams) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let deleted_count = store
            .delete_by_msgid(&params.msgid)
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "deleted_count": deleted_count,
        }))
    }

    // ==================== Metadata Operations ====================

    /// Set or clear a comment for a translation
    pub async fn set_comment(
        &self,
        params: SetCommentParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let mut entry = store
            .get(&params.msgid, params.msgctxt.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        // Update translator comment (split multi-line comments into separate lines)
        if let Some(comment) = &params.comment {
            entry.translator_comment = comment.lines().map(|l| l.to_string()).collect();
        } else {
            entry.translator_comment.clear();
        }

        store
            .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "comment": params.comment,
        }))
    }

    /// Toggle or set the fuzzy flag
    pub async fn set_fuzzy(
        &self,
        params: SetFuzzyParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let mut entry = store
            .get(&params.msgid, params.msgctxt.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        // Update fuzzy flag
        if params.fuzzy {
            if !entry.flags.contains(&"fuzzy".to_string()) {
                entry.flags.push("fuzzy".to_string());
            }
        } else {
            entry.flags.retain(|f| f != "fuzzy");
        }

        store
            .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "fuzzy": params.fuzzy,
        }))
    }

    /// Set or manage a flag (c-format, python-format, etc)
    pub async fn set_flag(
        &self,
        params: SetFlagParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let mut entry = store
            .get(&params.msgid, params.msgctxt.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        // Validate flag content (only allow non-empty alphanumeric, hyphens, underscores)
        if params.flag.is_empty() || !params.flag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err("Invalid flag: only alphanumeric characters, hyphens, and underscores are allowed".to_string());
        }

        // Update flag
        if params.enabled {
            if !entry.flags.contains(&params.flag) {
                entry.flags.push(params.flag.clone());
            }
        } else {
            entry.flags.retain(|f| f != &params.flag);
        }

        store
            .update_entry(&params.msgid, params.msgctxt.as_deref(), entry)
            .await
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "success": true,
            "msgid": params.msgid,
            "flag": params.flag,
            "enabled": params.enabled,
        }))
    }

    // ==================== File & Language Management ====================

    /// Get file metadata (encoding, plural forms, language, etc)
    pub async fn list_metadata(
        &self,
        params: ListMetadataParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let metadata = store.metadata().await.map_err(|e| e.to_string())?;
        let language = metadata.get("Language").cloned();
        let metadata_map: std::collections::BTreeMap<String, String> = metadata.into_iter().collect();

        Ok(json!({
            "metadata": metadata_map,
            "language": language,
        }))
    }

    /// Update a header metadata entry
    pub async fn set_header(
        &self,
        params: SetHeaderParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        if let Some(ref value) = params.value {
            store
                .set_header(&params.key, value)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            store
                .remove_header(&params.key)
                .await
                .map_err(|e| e.to_string())?;
        }

        Ok(json!({
            "success": true,
            "key": params.key,
            "value": params.value,
        }))
    }

    /// List all discovered .po/.pot files
    pub async fn list_files(&self) -> Result<Value, String> {
        let paths = self.manager.discovered_paths().await;
        let base_dir = self.manager.base_dir();

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

    /// List all unique msgctxt values
    pub async fn list_contexts(
        &self,
        params: ListContextsParams,
    ) -> Result<Value, String> {
        let store = self
            .manager
            .store_for(params.path.as_deref())
            .await
            .map_err(|e| e.to_string())?;

        let entries = store.list_all().await.map_err(|e| e.to_string())?;

        let mut contexts: Vec<String> = entries
            .iter()
            .filter_map(|(_, msgctxt, _)| msgctxt.clone())
            .collect();

        contexts.sort();
        contexts.dedup();

        Ok(json!(contexts))
    }
}

// ==================== Parameter Types ====================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListTranslationsParams {
    pub path: Option<String>,
    pub query: Option<String>,
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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListContextsParams {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListFilesParams {}

// ==================== MCP ServerHandler wiring ====================

fn tool<T: JsonSchema + std::any::Any>(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, Arc::new(schema_for_type::<T>()))
}

fn parse_args<T: DeserializeOwned>(
    name: &str,
    arguments: Option<Map<String, Value>>,
) -> Result<T, McpError> {
    let value = Value::Object(arguments.unwrap_or_default());
    serde_json::from_value(value).map_err(|e| {
        McpError::invalid_params(
            format!("invalid arguments for `{name}`: {e}"),
            None,
        )
    })
}

fn ok(value: Value) -> CallToolResult {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
    CallToolResult::success(vec![Content::text(text)])
}

fn err(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message.into())])
}

impl ServerHandler for GettextMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "MCP server for GNU gettext .po/.pot files. \
                 In dynamic mode (no path given on launch) every tool requires a `path` argument; \
                 in single-file mode `path` is optional and defaults to the file passed at startup."
                    .to_string(),
            ),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = vec![
            tool::<ListTranslationsParams>(
                "list_translations",
                "List translation entries with optional case-insensitive substring `query` and `limit`.",
            ),
            tool::<GetTranslationParams>(
                "get_translation",
                "Get a single translation entry by `msgid` (and optional `msgctxt`).",
            ),
            tool::<UpsertTranslationParams>(
                "upsert_translation",
                "Create or update a translation entry. Preserves existing flags/comments when updating.",
            ),
            tool::<DeleteTranslationParams>(
                "delete_translation",
                "Clear the translation (`msgstr`) for an entry without removing the key.",
            ),
            tool::<DeleteKeyParams>(
                "delete_key",
                "Remove every entry (across all contexts) with the given `msgid`.",
            ),
            tool::<SetCommentParams>(
                "set_comment",
                "Set or clear the translator comment for an entry. Pass `comment: null` to clear.",
            ),
            tool::<SetFuzzyParams>(
                "set_fuzzy",
                "Toggle the `fuzzy` flag on a translation entry.",
            ),
            tool::<SetFlagParams>(
                "set_flag",
                "Add or remove an arbitrary flag (e.g. `c-format`, `no-wrap`) on an entry.",
            ),
            tool::<ListMetadataParams>(
                "list_metadata",
                "List all PO header metadata entries (Language, Plural-Forms, etc.).",
            ),
            tool::<SetHeaderParams>(
                "set_header",
                "Set or remove a single PO header entry. Pass `value: null` to remove.",
            ),
            tool::<ListContextsParams>(
                "list_contexts",
                "List all distinct `msgctxt` values used in the file.",
            ),
            tool::<ListFilesParams>(
                "list_files",
                "List all .po/.pot files discovered in directory mode.",
            ),
        ];
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let CallToolRequestParam { name, arguments } = request;
        let result: Result<Value, String> = match name.as_ref() {
            "list_translations" => {
                self.list_translations(parse_args(&name, arguments)?).await
            }
            "get_translation" => {
                self.get_translation(parse_args(&name, arguments)?).await
            }
            "upsert_translation" => {
                self.upsert_translation(parse_args(&name, arguments)?).await
            }
            "delete_translation" => {
                self.delete_translation(parse_args(&name, arguments)?).await
            }
            "delete_key" => self.delete_key(parse_args(&name, arguments)?).await,
            "set_comment" => self.set_comment(parse_args(&name, arguments)?).await,
            "set_fuzzy" => self.set_fuzzy(parse_args(&name, arguments)?).await,
            "set_flag" => self.set_flag(parse_args(&name, arguments)?).await,
            "list_metadata" => self.list_metadata(parse_args(&name, arguments)?).await,
            "set_header" => self.set_header(parse_args(&name, arguments)?).await,
            "list_contexts" => self.list_contexts(parse_args(&name, arguments)?).await,
            "list_files" => {
                let _: ListFilesParams = parse_args(&name, arguments)?;
                self.list_files().await
            }
            other => {
                return Err(McpError::invalid_params(
                    format!("unknown tool: `{other}`"),
                    None,
                ));
            }
        };

        Ok(match result {
            Ok(value) => ok(value),
            Err(message) => err(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GettextStoreManager;

    #[tokio::test]
    async fn test_list_translations() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();

        // Add a test entry
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let params = ListTranslationsParams {
            path: Some(path.to_str().unwrap().to_string()),
            query: None,
            limit: None,
        };

        let result = server.list_translations(params).await.unwrap();
        assert!(result.is_array());

        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "Hello");
        assert_eq!(arr[0]["msgstr"], "Bonjour");
    }

    #[tokio::test]
    async fn test_get_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();

        // Add a test entry
        store.upsert("World", None, "Monde", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let params = GetTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "World".to_string(),
            msgctxt: None,
        };

        let result = server.get_translation(params).await.unwrap();
        assert_eq!(result["msgid"], "World");
        assert_eq!(result["msgstr"], "Monde");
    }

    #[tokio::test]
    async fn test_upsert_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        let params = UpsertTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "Test".to_string(),
            msgctxt: None,
            msgstr: Some("Tester".to_string()),
            msgid_plural: None,
            msgstr_plural: None,
            flags: None,
        };

        let result = server.upsert_translation(params).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["msgid"], "Test");

        // Verify it was stored
        let get_params = GetTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "Test".to_string(),
            msgctxt: None,
        };

        let get_result = server.get_translation(get_params).await.unwrap();
        assert_eq!(get_result["msgstr"], "Tester");
    }

    #[tokio::test]
    async fn test_set_fuzzy() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();

        // Add a test entry
        store.upsert("Fuzzy Test", None, "Test Fuzzy", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let params = SetFuzzyParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "Fuzzy Test".to_string(),
            msgctxt: None,
            fuzzy: true,
        };

        let result = server.set_fuzzy(params).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["fuzzy"], true);

        // Verify fuzzy flag was set
        let get_params = GetTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "Fuzzy Test".to_string(),
            msgctxt: None,
        };

        let get_result = server.get_translation(get_params).await.unwrap();
        assert_eq!(get_result["is_fuzzy"], true);
    }

    #[tokio::test]
    async fn test_list_contexts() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();

        // Add entries with different contexts
        store.upsert("Save", Some("menu"), "Enregistrer", None).await.unwrap();
        store.upsert("Save", Some("toolbar"), "Enregistrer", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let params = ListContextsParams {
            path: Some(path.to_str().unwrap().to_string()),
        };

        let result = server.list_contexts(params).await.unwrap();
        assert!(result.is_array());

        let contexts = result.as_array().unwrap();
        assert_eq!(contexts.len(), 2);
        assert!(contexts.iter().any(|c| c == "menu"));
        assert!(contexts.iter().any(|c| c == "toolbar"));
    }

    #[tokio::test]
    async fn test_delete_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let result = server.delete_translation(DeleteTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["msgid"], "Hello");

        // Verify deleted
        let err = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await;
        assert!(err.is_err());

        // Verify other entry still exists
        let world = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "World".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(world["msgstr"], "Monde");
    }

    #[tokio::test]
    async fn test_delete_translation_nonexistent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        let result = server.delete_translation(DeleteTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "nonexistent".to_string(),
            msgctxt: None,
        }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Save", Some("menu"), "Enregistrer", None).await.unwrap();
        store.upsert("Save", Some("toolbar"), "Sauvegarder", None).await.unwrap();
        store.upsert("Other", None, "Autre", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let result = server.delete_key(DeleteKeyParams {
            path: Some(path_str.clone()),
            msgid: "Save".to_string(),
        }).await.unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["deleted_count"], 2);

        // Verify both contexts deleted
        let err = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Save".to_string(),
            msgctxt: Some("menu".to_string()),
        }).await;
        assert!(err.is_err());

        // Verify other entry untouched
        let other = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Other".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(other["msgstr"], "Autre");
    }

    #[tokio::test]
    async fn test_set_comment() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Set a comment
        let result = server.set_comment(SetCommentParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
            comment: Some("A greeting message".to_string()),
        }).await.unwrap();

        assert_eq!(result["success"], true);

        // Verify comment was set
        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        let comments = entry["translator_comment"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0], "A greeting message");

        // Clear the comment
        let result = server.set_comment(SetCommentParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
            comment: None,
        }).await.unwrap();
        assert_eq!(result["success"], true);

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        let comments = entry["translator_comment"].as_array().unwrap();
        assert!(comments.is_empty());
    }

    #[tokio::test]
    async fn test_set_comment_multiline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        let result = server.set_comment(SetCommentParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
            comment: Some("Line 1\nLine 2\nLine 3".to_string()),
        }).await.unwrap();
        assert_eq!(result["success"], true);

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        let comments = entry["translator_comment"].as_array().unwrap();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0], "Line 1");
        assert_eq!(comments[1], "Line 2");
        assert_eq!(comments[2], "Line 3");
    }

    #[tokio::test]
    async fn test_set_comment_preserves_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", Some(vec!["c-format".to_string()])).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        server.set_comment(SetCommentParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
            comment: Some("New comment".to_string()),
        }).await.unwrap();

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(entry["msgstr"], "Bonjour");
        assert!(entry["flags"].as_array().unwrap().contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn test_set_flag_add_and_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello %s", None, "Bonjour %s", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Add c-format flag
        let result = server.set_flag(SetFlagParams {
            path: Some(path_str.clone()),
            msgid: "Hello %s".to_string(),
            msgctxt: None,
            flag: "c-format".to_string(),
            enabled: true,
        }).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["enabled"], true);

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Hello %s".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert!(entry["flags"].as_array().unwrap().contains(&json!("c-format")));

        // Remove c-format flag
        server.set_flag(SetFlagParams {
            path: Some(path_str.clone()),
            msgid: "Hello %s".to_string(),
            msgctxt: None,
            flag: "c-format".to_string(),
            enabled: false,
        }).await.unwrap();

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Hello %s".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert!(!entry["flags"].as_array().unwrap().contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn test_set_flag_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Test", None, "Test", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Add same flag twice
        server.set_flag(SetFlagParams {
            path: Some(path_str.clone()),
            msgid: "Test".to_string(),
            msgctxt: None,
            flag: "python-format".to_string(),
            enabled: true,
        }).await.unwrap();

        server.set_flag(SetFlagParams {
            path: Some(path_str.clone()),
            msgid: "Test".to_string(),
            msgctxt: None,
            flag: "python-format".to_string(),
            enabled: true,
        }).await.unwrap();

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Test".to_string(),
            msgctxt: None,
        }).await.unwrap();
        let flags: Vec<_> = entry["flags"].as_array().unwrap().iter()
            .filter(|f| f.as_str() == Some("python-format"))
            .collect();
        assert_eq!(flags.len(), 1, "Flag should not be duplicated");
    }

    #[tokio::test]
    async fn test_set_flag_rejects_invalid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Test", None, "Test", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Empty flag
        let result = server.set_flag(SetFlagParams {
            path: Some(path_str.clone()),
            msgid: "Test".to_string(),
            msgctxt: None,
            flag: "".to_string(),
            enabled: true,
        }).await;
        assert!(result.is_err());

        // Flag with spaces
        let result = server.set_flag(SetFlagParams {
            path: Some(path_str),
            msgid: "Test".to_string(),
            msgctxt: None,
            flag: "invalid flag".to_string(),
            enabled: true,
        }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.set_header("Content-Type", "text/plain; charset=UTF-8").await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));
        let result = server.list_metadata(ListMetadataParams {
            path: Some(path_str),
        }).await.unwrap();

        assert_eq!(result["language"], "fr");
        let metadata = result["metadata"].as_object().unwrap();
        assert_eq!(metadata["Language"], "fr");
        assert_eq!(metadata["Content-Type"], "text/plain; charset=UTF-8");
    }

    #[tokio::test]
    async fn test_set_header() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        // Set a header
        let result = server.set_header(SetHeaderParams {
            path: Some(path_str.clone()),
            key: "Language".to_string(),
            value: Some("de".to_string()),
        }).await.unwrap();
        assert_eq!(result["success"], true);

        // Verify via list_metadata
        let meta = server.list_metadata(ListMetadataParams {
            path: Some(path_str.clone()),
        }).await.unwrap();
        assert_eq!(meta["language"], "de");

        // Remove the header
        let result = server.set_header(SetHeaderParams {
            path: Some(path_str.clone()),
            key: "Language".to_string(),
            value: None,
        }).await.unwrap();
        assert_eq!(result["success"], true);

        let meta = server.list_metadata(ListMetadataParams {
            path: Some(path_str),
        }).await.unwrap();
        assert!(meta["language"].is_null());
    }

    #[tokio::test]
    async fn test_upsert_with_plurals() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        let result = server.upsert_translation(UpsertTranslationParams {
            path: Some(path_str.clone()),
            msgid: "%d file".to_string(),
            msgctxt: None,
            msgstr: Some("".to_string()),
            msgid_plural: Some("%d files".to_string()),
            msgstr_plural: Some(vec!["%d fichier".to_string(), "%d fichiers".to_string()]),
            flags: Some(vec!["c-format".to_string()]),
        }).await.unwrap();
        assert_eq!(result["success"], true);

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "%d file".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(entry["msgid_plural"], "%d files");
        let plurals = entry["msgstr_plural"].as_array().unwrap();
        assert_eq!(plurals.len(), 2);
        assert_eq!(plurals[0], "%d fichier");
        assert_eq!(plurals[1], "%d fichiers");
        assert!(entry["flags"].as_array().unwrap().contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn test_list_translations_with_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("Goodbye", None, "Au revoir", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Search by msgid
        let result = server.list_translations(ListTranslationsParams {
            path: Some(path_str.clone()),
            query: Some("hello".to_string()),
            limit: None,
        }).await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "Hello");

        // Search by msgstr
        let result = server.list_translations(ListTranslationsParams {
            path: Some(path_str.clone()),
            query: Some("monde".to_string()),
            limit: None,
        }).await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "World");
    }

    #[tokio::test]
    async fn test_list_translations_with_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("A", None, "a", None).await.unwrap();
        store.upsert("B", None, "b", None).await.unwrap();
        store.upsert("C", None, "c", None).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        let result = server.list_translations(ListTranslationsParams {
            path: Some(path_str),
            query: None,
            limit: Some(2),
        }).await.unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn test_set_fuzzy_clear() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", Some(vec!["fuzzy".to_string()])).await.unwrap();

        let server = GettextMcpServer::new(Arc::new(manager));

        // Verify initially fuzzy
        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(entry["is_fuzzy"], true);
        assert_eq!(entry["is_translated"], false); // fuzzy entries are not "translated"

        // Clear fuzzy
        server.set_fuzzy(SetFuzzyParams {
            path: Some(path_str.clone()),
            msgid: "Hello".to_string(),
            msgctxt: None,
            fuzzy: false,
        }).await.unwrap();

        let entry = server.get_translation(GetTranslationParams {
            path: Some(path_str),
            msgid: "Hello".to_string(),
            msgctxt: None,
        }).await.unwrap();
        assert_eq!(entry["is_fuzzy"], false);
        assert_eq!(entry["is_translated"], true);
    }

    #[tokio::test]
    async fn test_get_translation_nonexistent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        let result = server.get_translation(GetTranslationParams {
            path: Some(path.to_str().unwrap().to_string()),
            msgid: "nonexistent".to_string(),
            msgctxt: None,
        }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_upsert_with_context() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let path_str = path.to_str().unwrap().to_string();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let server = GettextMcpServer::new(Arc::new(manager));

        // Upsert same msgid with different contexts
        server.upsert_translation(UpsertTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Open".to_string(),
            msgctxt: Some("menu".to_string()),
            msgstr: Some("Ouvrir".to_string()),
            msgid_plural: None,
            msgstr_plural: None,
            flags: None,
        }).await.unwrap();

        server.upsert_translation(UpsertTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Open".to_string(),
            msgctxt: Some("button".to_string()),
            msgstr: Some("Ouvrir".to_string()),
            msgid_plural: None,
            msgstr_plural: None,
            flags: None,
        }).await.unwrap();

        // Get with specific context
        let menu = server.get_translation(GetTranslationParams {
            path: Some(path_str.clone()),
            msgid: "Open".to_string(),
            msgctxt: Some("menu".to_string()),
        }).await.unwrap();
        assert_eq!(menu["msgstr"], "Ouvrir");
        assert_eq!(menu["msgctxt"], "menu");

        // List all should have both
        let list = server.list_translations(ListTranslationsParams {
            path: Some(path_str),
            query: Some("Open".to_string()),
            limit: None,
        }).await.unwrap();
        assert_eq!(list.as_array().unwrap().len(), 2);
    }
}
