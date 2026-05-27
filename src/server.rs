//! Gettext MCP server struct and tool routing.
//!
//! Tool implementations are `#[tool]` methods that delegate to the
//! `handle_*` functions in [`crate::tools`]. The `#[tool_router]` /
//! `#[tool_handler]` macros from rmcp 1.7 generate the dispatch table
//! and the `ServerHandler::list_tools` / `call_tool` glue.

use std::sync::Arc;

use rmcp::{
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::{
        GetPromptRequestParams, GetPromptResult, ListPromptsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    prompt_handler,
    service::RequestContext,
    tool, tool_handler, tool_router, RoleServer, ServerHandler,
};
use tokio::sync::Mutex;
use tracing::error;

use crate::service::GettextStoreManager;
use crate::tools::{
    coverage::{handle_get_coverage, GetCoverageParams},
    crud::{
        handle_delete_key, handle_delete_translation, handle_get_translation,
        handle_list_translations, handle_upsert_translation, DeleteKeyParams,
        DeleteTranslationParams, GetTranslationParams, ListTranslationsParams,
        UpsertTranslationParams,
    },
    discover::{handle_list_contexts, handle_list_files, ListContextsParams, ListFilesParams},
    discover_files::{handle_discover_files, DiscoverFilesParams},
    extract::{handle_get_stale, handle_get_untranslated, GetStaleParams, GetUntranslatedParams},
    glossary::{
        handle_get_glossary, handle_update_glossary, GetGlossaryParams, UpdateGlossaryParams,
    },
    header::{handle_list_metadata, handle_set_header, ListMetadataParams, SetHeaderParams},
    metadata::{
        handle_set_comment, handle_set_flag, handle_set_fuzzy, SetCommentParams, SetFlagParams,
        SetFuzzyParams,
    },
    search::{handle_search_keys, SearchKeysParams},
    validate::{handle_validate_translations, ValidateTranslationsParams},
    xliff::{handle_export_xliff, handle_import_xliff, ExportXliffParams, ImportXliffParams},
};

/// MCP server for GNU gettext `.po`/`.pot` files. Holds the shared store
/// manager and the macro-generated tool/prompt routers.
#[derive(Clone)]
pub struct GettextMcpServer {
    manager: Arc<GettextStoreManager>,
    /// Serializes glossary read-modify-write cycles so concurrent MCP
    /// clients can't lose each other's updates. The PO store has its own
    /// per-file locking; the glossary file does not, so we centralize it
    /// here on the server.
    glossary_write_lock: Arc<Mutex<()>>,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl GettextMcpServer {
    pub fn new(manager: Arc<GettextStoreManager>) -> Self {
        Self {
            manager,
            glossary_write_lock: Arc::new(Mutex::new(())),
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    /// Read-only access to the underlying store manager, used by tests
    /// and the web layer to share the same cache.
    pub fn manager(&self) -> &Arc<GettextStoreManager> {
        &self.manager
    }
}

#[tool_router]
impl GettextMcpServer {
    #[tool(
        name = "list_translations",
        description = "List translation entries with optional case-insensitive substring `query` and `limit`."
    )]
    async fn list_translations(
        &self,
        Parameters(params): Parameters<ListTranslationsParams>,
    ) -> Result<String, String> {
        match handle_list_translations(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "list_translations failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "get_translation",
        description = "Get a single translation entry by `msgid` (and optional `msgctxt`)."
    )]
    async fn get_translation(
        &self,
        Parameters(params): Parameters<GetTranslationParams>,
    ) -> Result<String, String> {
        match handle_get_translation(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "get_translation failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "upsert_translation",
        description = "Create or update a translation entry. Preserves existing comments and source locations when updating."
    )]
    async fn upsert_translation(
        &self,
        Parameters(params): Parameters<UpsertTranslationParams>,
    ) -> Result<String, String> {
        match handle_upsert_translation(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "upsert_translation failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "delete_translation",
        description = "Delete a single translation entry by `msgid` and optional `msgctxt`."
    )]
    async fn delete_translation(
        &self,
        Parameters(params): Parameters<DeleteTranslationParams>,
    ) -> Result<String, String> {
        match handle_delete_translation(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "delete_translation failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "delete_key",
        description = "Remove every entry (across all contexts) with the given `msgid`."
    )]
    async fn delete_key(
        &self,
        Parameters(params): Parameters<DeleteKeyParams>,
    ) -> Result<String, String> {
        match handle_delete_key(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "delete_key failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "set_comment",
        description = "Set or clear the translator comment for an entry. Pass `comment: null` to clear."
    )]
    async fn set_comment(
        &self,
        Parameters(params): Parameters<SetCommentParams>,
    ) -> Result<String, String> {
        match handle_set_comment(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "set_comment failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "set_fuzzy",
        description = "Toggle the `fuzzy` flag on a translation entry."
    )]
    async fn set_fuzzy(
        &self,
        Parameters(params): Parameters<SetFuzzyParams>,
    ) -> Result<String, String> {
        match handle_set_fuzzy(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "set_fuzzy failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "set_flag",
        description = "Add or remove an arbitrary flag (e.g. `c-format`, `no-wrap`) on an entry."
    )]
    async fn set_flag(
        &self,
        Parameters(params): Parameters<SetFlagParams>,
    ) -> Result<String, String> {
        match handle_set_flag(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "set_flag failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "list_metadata",
        description = "List all PO header metadata entries (Language, Plural-Forms, etc.)."
    )]
    async fn list_metadata(
        &self,
        Parameters(params): Parameters<ListMetadataParams>,
    ) -> Result<String, String> {
        match handle_list_metadata(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "list_metadata failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "set_header",
        description = "Set or remove a single PO header entry. Pass `value: null` to remove."
    )]
    async fn set_header(
        &self,
        Parameters(params): Parameters<SetHeaderParams>,
    ) -> Result<String, String> {
        match handle_set_header(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "set_header failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "list_contexts",
        description = "List all distinct `msgctxt` values used in the file."
    )]
    async fn list_contexts(
        &self,
        Parameters(params): Parameters<ListContextsParams>,
    ) -> Result<String, String> {
        match handle_list_contexts(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "list_contexts failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "list_files",
        description = "List all .po/.pot files discovered in directory mode."
    )]
    async fn list_files(
        &self,
        Parameters(_params): Parameters<ListFilesParams>,
    ) -> Result<String, String> {
        match handle_list_files(&self.manager).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "list_files failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "get_coverage",
        description = "Compute translation coverage stats (translated/untranslated/fuzzy/obsolete counts and percentages). Fuzzy entries with non-empty msgstr count as both fuzzy and translated. Obsolete entries are excluded from total and percentages."
    )]
    async fn get_coverage(
        &self,
        Parameters(params): Parameters<GetCoverageParams>,
    ) -> Result<String, String> {
        match handle_get_coverage(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "get_coverage failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "get_untranslated",
        description = "Paginated list of entries with empty msgstr or fuzzy flag. Each entry includes `needs_plural_forms` (CLDR plural categories) when msgid_plural is set."
    )]
    async fn get_untranslated(
        &self,
        Parameters(params): Parameters<GetUntranslatedParams>,
    ) -> Result<String, String> {
        match handle_get_untranslated(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "get_untranslated failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "get_stale",
        description = "Paginated list of obsolete (`#~`) entries kept in the file but no longer used by source code."
    )]
    async fn get_stale(
        &self,
        Parameters(params): Parameters<GetStaleParams>,
    ) -> Result<String, String> {
        match handle_get_stale(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "get_stale failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "validate_translations",
        description = "Run validation checks (format specifier mismatches, plural form count, empty translations, identical translations) and return findings grouped by severity (error/warning/info)."
    )]
    async fn validate_translations(
        &self,
        Parameters(params): Parameters<ValidateTranslationsParams>,
    ) -> Result<String, String> {
        match handle_validate_translations(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "validate_translations failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "search_keys",
        description = "Paginated case-insensitive substring search across msgid/msgstr/msgctxt/comment fields. Empty pattern returns all entries."
    )]
    async fn search_keys(
        &self,
        Parameters(params): Parameters<SearchKeysParams>,
    ) -> Result<String, String> {
        match handle_search_keys(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "search_keys failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "discover_files",
        description = "Recursively scan a directory for .po/.pot files. Independent of directory mode; skips hidden and well-known build directories (.git, node_modules, target, ...)."
    )]
    async fn discover_files(
        &self,
        Parameters(params): Parameters<DiscoverFilesParams>,
    ) -> Result<String, String> {
        match handle_discover_files(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "discover_files failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "export_xliff",
        description = "Export a PO file to an XLIFF 1.2 document. Skips plural entries (XLIFF 1.2 has no clean plural model) and obsolete entries. By default only untranslated/fuzzy entries are exported; set `include_translated=true` to emit every non-plural entry. The `output` path must end in .xliff, .xlf, or .xml."
    )]
    async fn export_xliff(
        &self,
        Parameters(params): Parameters<ExportXliffParams>,
    ) -> Result<String, String> {
        match handle_export_xliff(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "export_xliff failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "import_xliff",
        description = "Import translations from an XLIFF 1.2 document into a PO file. Matches `<trans-unit>` entries to PO entries by msgid (and msgctxt when carried as a `gettext-msgctxt` note). Units with mismatched format specifiers are rejected; units that don't match any PO entry are reported as `unmatched`. Set `dry_run=true` to preview without writing; set `mark_fuzzy=true` to flag imported translations for review."
    )]
    async fn import_xliff(
        &self,
        Parameters(params): Parameters<ImportXliffParams>,
    ) -> Result<String, String> {
        match handle_import_xliff(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "import_xliff failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "get_glossary",
        description = "Look up preferred-translation terms for a `(source_locale, target_locale)` pair. The glossary lives at $GETTEXT_GLOSSARY_PATH (default: ./glossary.json) and is independent of any PO file. Optional `filter` is a case-insensitive substring matched against either the source term or its translation; a missing file yields an empty result rather than an error."
    )]
    async fn get_glossary(
        &self,
        Parameters(params): Parameters<GetGlossaryParams>,
    ) -> Result<String, String> {
        match handle_get_glossary(&self.manager, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "get_glossary failed");
                Err(e.to_string())
            }
        }
    }

    #[tool(
        name = "update_glossary",
        description = "Upsert and/or remove glossary terms for a `(source_locale, target_locale)` pair. `entries` is a term→translation map (existing terms are overwritten); `delete` lists terms to remove (missing terms are ignored). The file is read, patched, and written atomically through the same FileStore the PO tools use."
    )]
    async fn update_glossary(
        &self,
        Parameters(params): Parameters<UpdateGlossaryParams>,
    ) -> Result<String, String> {
        match handle_update_glossary(&self.manager, &self.glossary_write_lock, params).await {
            Ok(value) => serde_json::to_string_pretty(&value)
                .map_err(|e| format!("serialization error: {e}")),
            Err(e) => {
                error!(error = %e, "update_glossary failed");
                Err(e.to_string())
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for GettextMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .with_instructions(
            "MCP server for GNU gettext .po/.pot files. \
             In dynamic mode (no path given on launch) every tool requires a `path` argument; \
             in single-file mode `path` is optional and defaults to the file passed at startup. \
             Additional tools: `get_coverage` (stats), `get_untranslated` and `get_stale` \
             (paginated review queues), `validate_translations` (format/plural/empty checks), \
             `search_keys` (paginated search), `discover_files` (scan a directory for \
             .po/.pot files), `export_xliff`/`import_xliff` (XLIFF 1.2 interchange — \
             plurals and obsolete entries are skipped), and `get_glossary`/`update_glossary` \
             (shared term bank at $GETTEXT_GLOSSARY_PATH, default ./glossary.json).",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::crud::{
        DeleteKeyParams, DeleteTranslationParams, GetTranslationParams, ListTranslationsParams,
        UpsertTranslationParams,
    };
    use crate::tools::discover::ListContextsParams;
    use crate::tools::header::{ListMetadataParams, SetHeaderParams};
    use crate::tools::metadata::{SetCommentParams, SetFlagParams, SetFuzzyParams};
    use serde_json::json;

    async fn make_server(path: &std::path::Path) -> (GettextMcpServer, String) {
        let manager = Arc::new(GettextStoreManager::new(Some(path.to_path_buf())));
        let server = GettextMcpServer::new(manager);
        (server, path.to_str().unwrap().to_string())
    }

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("server returned non-JSON")
    }

    #[tokio::test]
    async fn list_translations() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                msgstr: Some("Bonjour".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();

        let raw = server
            .list_translations(Parameters(ListTranslationsParams {
                path: Some(path_str),
                query: None,
                limit: None,
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "Hello");
        assert_eq!(arr[0]["msgstr"], "Bonjour");
    }

    #[tokio::test]
    async fn get_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "World".into(),
                msgctxt: None,
                msgstr: Some("Monde".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "World".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert_eq!(entry["msgid"], "World");
        assert_eq!(entry["msgstr"], "Monde");
    }

    #[tokio::test]
    async fn upsert_translation_persists() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let raw = server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Test".into(),
                msgctxt: None,
                msgstr: Some("Tester".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["success"], true);

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Test".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["msgstr"], "Tester");
    }

    #[tokio::test]
    async fn set_fuzzy_toggle() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Fuzzy Test".into(),
                msgctxt: None,
                msgstr: Some("Test Fuzzy".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();

        let raw = server
            .set_fuzzy(Parameters(SetFuzzyParams {
                path: Some(path_str.clone()),
                msgid: "Fuzzy Test".into(),
                msgctxt: None,
                fuzzy: true,
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["success"], true);
        assert_eq!(result["fuzzy"], true);

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Fuzzy Test".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["is_fuzzy"], true);
    }

    #[tokio::test]
    async fn list_contexts() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store
            .upsert("Save", Some("menu"), "Enregistrer", None)
            .await
            .unwrap();
        store
            .upsert("Save", Some("toolbar"), "Enregistrer", None)
            .await
            .unwrap();

        let raw = server
            .list_contexts(Parameters(ListContextsParams {
                path: Some(path_str),
            }))
            .await
            .unwrap();
        let contexts = parse(&raw);
        assert!(contexts.is_array());
        let arr = contexts.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().any(|c| c == "menu"));
        assert!(arr.iter().any(|c| c == "toolbar"));
    }

    #[tokio::test]
    async fn delete_translation() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let raw = server
            .delete_translation(Parameters(DeleteTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["success"], true);

        let err = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await;
        assert!(err.is_err());

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "World".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["msgstr"], "Monde");
    }

    #[tokio::test]
    async fn delete_translation_nonexistent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let result = server
            .delete_translation(Parameters(DeleteTranslationParams {
                path: Some(path_str),
                msgid: "nonexistent".into(),
                msgctxt: None,
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_key_clears_all_contexts() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store
            .upsert("Save", Some("menu"), "Enregistrer", None)
            .await
            .unwrap();
        store
            .upsert("Save", Some("toolbar"), "Sauvegarder", None)
            .await
            .unwrap();
        store.upsert("Other", None, "Autre", None).await.unwrap();

        let raw = server
            .delete_key(Parameters(DeleteKeyParams {
                path: Some(path_str.clone()),
                msgid: "Save".into(),
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["success"], true);
        assert_eq!(result["deleted_count"], 2);

        let err = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Save".into(),
                msgctxt: Some("menu".into()),
            }))
            .await;
        assert!(err.is_err());

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Other".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["msgstr"], "Autre");
    }

    #[tokio::test]
    async fn set_comment_then_clear() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let raw = server
            .set_comment(Parameters(SetCommentParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                comment: Some("A greeting message".into()),
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["success"], true);

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        let comments = entry["translator_comment"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0], "A greeting message");

        server
            .set_comment(Parameters(SetCommentParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                comment: None,
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert!(entry["translator_comment"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_comment_multiline() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        server
            .set_comment(Parameters(SetCommentParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                comment: Some("Line 1\nLine 2\nLine 3".into()),
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        let comments = entry["translator_comment"].as_array().unwrap();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[2], "Line 3");
    }

    #[tokio::test]
    async fn set_comment_preserves_flags_and_msgstr() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store
            .upsert("Hello", None, "Bonjour", Some(vec!["c-format".into()]))
            .await
            .unwrap();

        server
            .set_comment(Parameters(SetCommentParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                comment: Some("New comment".into()),
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert_eq!(entry["msgstr"], "Bonjour");
        assert!(entry["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn set_flag_add_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store
            .upsert("Hello %s", None, "Bonjour %s", None)
            .await
            .unwrap();

        let raw = server
            .set_flag(Parameters(SetFlagParams {
                path: Some(path_str.clone()),
                msgid: "Hello %s".into(),
                msgctxt: None,
                flag: "c-format".into(),
                enabled: true,
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["success"], true);
        assert_eq!(result["enabled"], true);

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello %s".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert!(parse(&raw)["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("c-format")));

        server
            .set_flag(Parameters(SetFlagParams {
                path: Some(path_str.clone()),
                msgid: "Hello %s".into(),
                msgctxt: None,
                flag: "c-format".into(),
                enabled: false,
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Hello %s".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        assert!(!parse(&raw)["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn set_flag_idempotent_add() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Test", None, "Test", None).await.unwrap();

        for _ in 0..2 {
            server
                .set_flag(Parameters(SetFlagParams {
                    path: Some(path_str.clone()),
                    msgid: "Test".into(),
                    msgctxt: None,
                    flag: "python-format".into(),
                    enabled: true,
                }))
                .await
                .unwrap();
        }

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Test".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        let count = entry["flags"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f.as_str() == Some("python-format"))
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn set_flag_rejects_invalid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Test", None, "Test", None).await.unwrap();

        let result = server
            .set_flag(Parameters(SetFlagParams {
                path: Some(path_str.clone()),
                msgid: "Test".into(),
                msgctxt: None,
                flag: "".into(),
                enabled: true,
            }))
            .await;
        assert!(result.is_err());

        let result = server
            .set_flag(Parameters(SetFlagParams {
                path: Some(path_str),
                msgid: "Test".into(),
                msgctxt: None,
                flag: "invalid flag".into(),
                enabled: true,
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store
            .set_header("Content-Type", "text/plain; charset=UTF-8")
            .await
            .unwrap();

        let raw = server
            .list_metadata(Parameters(ListMetadataParams {
                path: Some(path_str),
            }))
            .await
            .unwrap();
        let result = parse(&raw);
        assert_eq!(result["language"], "fr");
        let metadata = result["metadata"].as_object().unwrap();
        assert_eq!(metadata["Language"], "fr");
        assert_eq!(metadata["Content-Type"], "text/plain; charset=UTF-8");
    }

    #[tokio::test]
    async fn set_header_set_then_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .set_header(Parameters(SetHeaderParams {
                path: Some(path_str.clone()),
                key: "Language".into(),
                value: Some("de".into()),
            }))
            .await
            .unwrap();

        let raw = server
            .list_metadata(Parameters(ListMetadataParams {
                path: Some(path_str.clone()),
            }))
            .await
            .unwrap();
        assert_eq!(parse(&raw)["language"], "de");

        server
            .set_header(Parameters(SetHeaderParams {
                path: Some(path_str.clone()),
                key: "Language".into(),
                value: None,
            }))
            .await
            .unwrap();

        let raw = server
            .list_metadata(Parameters(ListMetadataParams {
                path: Some(path_str),
            }))
            .await
            .unwrap();
        assert!(parse(&raw)["language"].is_null());
    }

    #[tokio::test]
    async fn upsert_with_plurals() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "%d file".into(),
                msgctxt: None,
                msgstr: Some("".into()),
                msgid_plural: Some("%d files".into()),
                msgstr_plural: Some(vec!["%d fichier".into(), "%d fichiers".into()]),
                flags: Some(vec!["c-format".into()]),
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "%d file".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert_eq!(entry["msgid_plural"], "%d files");
        let plurals = entry["msgstr_plural"].as_array().unwrap();
        assert_eq!(plurals.len(), 2);
        assert_eq!(plurals[0], "%d fichier");
        assert_eq!(plurals[1], "%d fichiers");
        assert!(entry["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("c-format")));
    }

    #[tokio::test]
    async fn list_translations_with_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store
            .upsert("Goodbye", None, "Au revoir", None)
            .await
            .unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let raw = server
            .list_translations(Parameters(ListTranslationsParams {
                path: Some(path_str.clone()),
                query: Some("hello".into()),
                limit: None,
            }))
            .await
            .unwrap();
        let arr = parse(&raw);
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "Hello");

        let raw = server
            .list_translations(Parameters(ListTranslationsParams {
                path: Some(path_str),
                query: Some("monde".into()),
                limit: None,
            }))
            .await
            .unwrap();
        let arr = parse(&raw);
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["msgid"], "World");
    }

    #[tokio::test]
    async fn list_translations_with_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store.upsert("A", None, "a", None).await.unwrap();
        store.upsert("B", None, "b", None).await.unwrap();
        store.upsert("C", None, "c", None).await.unwrap();

        let raw = server
            .list_translations(Parameters(ListTranslationsParams {
                path: Some(path_str),
                query: None,
                limit: Some(2),
            }))
            .await
            .unwrap();
        let arr = parse(&raw);
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn set_fuzzy_clear_makes_translated() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let store = server.manager.store_for(None).await.unwrap();
        store
            .upsert("Hello", None, "Bonjour", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert_eq!(entry["is_fuzzy"], true);
        assert_eq!(entry["is_translated"], false);

        server
            .set_fuzzy(Parameters(SetFuzzyParams {
                path: Some(path_str.clone()),
                msgid: "Hello".into(),
                msgctxt: None,
                fuzzy: false,
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "Hello".into(),
                msgctxt: None,
            }))
            .await
            .unwrap();
        let entry = parse(&raw);
        assert_eq!(entry["is_fuzzy"], false);
        assert_eq!(entry["is_translated"], true);
    }

    #[tokio::test]
    async fn get_translation_nonexistent_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        let result = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str),
                msgid: "nonexistent".into(),
                msgctxt: None,
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn upsert_with_context() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let (server, path_str) = make_server(&path).await;

        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Open".into(),
                msgctxt: Some("menu".into()),
                msgstr: Some("Ouvrir".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();
        server
            .upsert_translation(Parameters(UpsertTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Open".into(),
                msgctxt: Some("button".into()),
                msgstr: Some("Ouvrir".into()),
                msgid_plural: None,
                msgstr_plural: None,
                flags: None,
            }))
            .await
            .unwrap();

        let raw = server
            .get_translation(Parameters(GetTranslationParams {
                path: Some(path_str.clone()),
                msgid: "Open".into(),
                msgctxt: Some("menu".into()),
            }))
            .await
            .unwrap();
        let menu = parse(&raw);
        assert_eq!(menu["msgstr"], "Ouvrir");
        assert_eq!(menu["msgctxt"], "menu");

        let raw = server
            .list_translations(Parameters(ListTranslationsParams {
                path: Some(path_str),
                query: Some("Open".into()),
                limit: None,
            }))
            .await
            .unwrap();
        let arr = parse(&raw);
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }
}
