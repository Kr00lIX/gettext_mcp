use axum::{
    extract::{Query, State, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, delete},
    Router, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use std::net::SocketAddr;

use crate::service::GettextStoreManager;

/// Web server configuration
pub struct WebConfig {
    pub addr: SocketAddr,
    pub manager: Arc<GettextStoreManager>,
}

/// Request/Response types for REST API

#[derive(Debug, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub name: String,
    pub language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranslationRecord {
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub msgstr: String,
    pub msgid_plural: Option<String>,
    pub msgstr_plural: Vec<String>,
    pub is_translated: bool,
    pub is_fuzzy: bool,
    pub flags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranslationDetail {
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub msgstr: String,
    pub msgid_plural: Option<String>,
    pub msgstr_plural: Vec<String>,
    pub is_translated: bool,
    pub is_fuzzy: bool,
    pub flags: Vec<String>,
    pub extracted_comment: Option<String>,
    pub translator_comment: Option<String>,
    pub source_locations: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListTranslationsQuery {
    pub file: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub msgctxt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub file: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TranslationQuery {
    pub file: Option<String>,
    pub msgid: String,
    pub msgctxt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertTranslationPayload {
    pub msgid: String,
    pub msgctxt: Option<String>,
    pub msgstr: String,
    pub msgid_plural: Option<String>,
    pub msgstr_plural: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMetadataPayload {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct Metadata {
    pub encoding: Option<String>,
    pub language: Option<String>,
    pub plural_forms: Option<String>,
}

#[derive(Clone)]
struct AppState {
    manager: Arc<GettextStoreManager>,
}

/// Start the web server
pub async fn serve(config: WebConfig) -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        manager: config.manager,
    };

    let app = Router::new()
        .route("/", get(index_html))
        .route("/api/files", get(list_files))
        .route("/api/translations", get(list_translations))
        .route("/api/translations", post(upsert_translation))
        .route("/api/translations/detail", get(get_translation))
        .route("/api/translations/detail", delete(delete_translation))
        .route("/api/metadata", get(get_metadata))
        .route("/api/metadata", post(update_metadata))
        .route("/api/languages", get(list_languages))
        .route("/api/languages", post(add_language))
        .route("/api/languages/{language}", delete(remove_language))
        .layer(CorsLayer::new()
            .allow_origin([
                "http://localhost".parse::<axum::http::HeaderValue>().unwrap(),
                "http://127.0.0.1".parse::<axum::http::HeaderValue>().unwrap(),
                format!("http://localhost:{}", config.addr.port()).parse::<axum::http::HeaderValue>().unwrap(),
                format!("http://127.0.0.1:{}", config.addr.port()).parse::<axum::http::HeaderValue>().unwrap(),
            ])
            .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::DELETE])
            .allow_headers([axum::http::header::CONTENT_TYPE]))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    tracing::info!("Web server listening on {}", config.addr);
    axum::serve(listener, app).await?;

    Ok(())
}

// ==================== Route Handlers ====================

/// Serve the SPA HTML
async fn index_html() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        SPA_HTML,
    )
}

/// List discovered .po files
async fn list_files(
    State(state): State<AppState>,
) -> Result<Json<Vec<FileInfo>>, (StatusCode, String)> {
    let discovered = state.manager.discovered_paths().await;
    let base_dir = state.manager.base_dir().map(|p| p.to_path_buf());

    let files: Vec<FileInfo> = discovered
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            let language = extract_language(path, base_dir.as_deref());
            Some(FileInfo {
                path: path.to_string_lossy().to_string(),
                name,
                language,
            })
        })
        .collect();

    Ok(Json(files))
}

/// Extract language code from a PO file path.
/// Looks for the standard gettext layout: `{lang}/LC_MESSAGES/*.po`
/// Falls back to checking path components against the base dir.
fn extract_language(path: &std::path::Path, base_dir: Option<&std::path::Path>) -> Option<String> {
    // .pot files are templates, not language-specific
    if path.extension().and_then(|e| e.to_str()) == Some("pot") {
        return None;
    }

    // Standard layout: .../{lang}/LC_MESSAGES/file.po
    let components: Vec<_> = path.components().collect();
    for (i, comp) in components.iter().enumerate() {
        if let std::path::Component::Normal(s) = comp {
            if s.to_str() == Some("LC_MESSAGES") && i > 0 {
                if let std::path::Component::Normal(lang) = &components[i - 1] {
                    return lang.to_str().map(|s| s.to_string());
                }
            }
        }
    }

    // Fallback: first path component relative to the base dir
    if let Some(base) = base_dir {
        if let Ok(rel) = path.strip_prefix(base) {
            let first = rel.components().next()?;
            if let std::path::Component::Normal(s) = first {
                let s = s.to_str()?;
                // Only return if it looks like a language dir (not the file itself)
                if rel.components().count() > 1 {
                    return Some(s.to_string());
                }
            }
        }
    }

    None
}

/// List translations with optional filtering
async fn list_translations(
    State(state): State<AppState>,
    Query(params): Query<ListTranslationsQuery>,
) -> Result<Json<Vec<TranslationRecord>>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let entries = store
        .list_all()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut filtered: Vec<_> = entries
        .into_iter()
        .filter(|(_, ctx, _)| {
            if let Some(ref filter_ctx) = params.msgctxt {
                ctx.as_deref() == Some(filter_ctx.as_str())
            } else {
                true
            }
        })
        .filter(|(msgid, _, entry)| {
            if let Some(ref query) = params.query {
                let q_lower = query.to_lowercase();
                msgid.to_lowercase().contains(&q_lower)
                    || entry.msgstr.to_lowercase().contains(&q_lower)
                    || entry.msgid_plural.as_deref().is_some_and(|p| p.to_lowercase().contains(&q_lower))
                    || entry.msgstr_plural.iter().any(|p| p.to_lowercase().contains(&q_lower))
            } else {
                true
            }
        })
        .collect();

    if let Some(limit) = params.limit {
        filtered.truncate(limit);
    }

    let records: Vec<_> = filtered
        .into_iter()
        .map(|(msgid, msgctxt, entry)| {
            let is_translated = entry.is_translated();
            let is_fuzzy = entry.is_fuzzy();
            TranslationRecord {
                msgid,
                msgctxt,
                msgstr: entry.msgstr,
                msgid_plural: entry.msgid_plural,
                msgstr_plural: entry.msgstr_plural,
                is_translated,
                is_fuzzy,
                flags: entry.flags,
            }
        })
        .collect();

    Ok(Json(records))
}

/// Get a single translation entry
async fn get_translation(
    State(state): State<AppState>,
    Query(params): Query<TranslationQuery>,
) -> Result<Json<TranslationDetail>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let entry = store
        .get(&params.msgid, params.msgctxt.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let detail = TranslationDetail {
        msgid: params.msgid,
        msgctxt: params.msgctxt,
        msgstr: entry.msgstr.clone(),
        msgid_plural: entry.msgid_plural.clone(),
        msgstr_plural: entry.msgstr_plural.clone(),
        is_translated: entry.is_translated(),
        is_fuzzy: entry.is_fuzzy(),
        flags: entry.flags.clone(),
        extracted_comment: if entry.extracted_comment.is_empty() { None } else { Some(entry.extracted_comment.join("\n")) },
        translator_comment: if entry.translator_comment.is_empty() { None } else { Some(entry.translator_comment.join("\n")) },
        source_locations: entry.source_locations.clone(),
    };

    Ok(Json(detail))
}

/// Create or update a translation
async fn upsert_translation(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
    Json(payload): Json<UpsertTranslationPayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    store
        .upsert_full(
            &payload.msgid,
            payload.msgctxt.as_deref(),
            &payload.msgstr,
            payload.msgid_plural.as_deref(),
            payload.msgstr_plural,
            None,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

/// Delete a translation
async fn delete_translation(
    State(state): State<AppState>,
    Query(params): Query<TranslationQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    store
        .delete(&params.msgid, params.msgctxt.as_deref())
        .await
        .map_err(|e| {
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, e.to_string())
        })?;

    Ok(Json(json!({ "success": true })))
}

/// Get file metadata (encoding, language, plural forms)
async fn get_metadata(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
) -> Result<Json<Metadata>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let metadata = store
        .metadata()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(Metadata {
        encoding: metadata.get("Content-Type").and_then(|ct| {
            ct.split("charset=").nth(1).map(|s| s.trim().to_string())
        }),
        language: metadata.get("Language").cloned(),
        plural_forms: metadata.get("Plural-Forms").cloned(),
    }))
}

/// Update metadata
async fn update_metadata(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
    Json(payload): Json<UpdateMetadataPayload>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    store
        .set_header(&payload.key, &payload.value)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

/// List all languages
async fn list_languages(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let languages = store
        .list_languages()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(languages))
}

/// Add a new language
async fn add_language(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
    Json(payload): Json<serde_json::Map<String, Value>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let language = payload
        .get("language")
        .and_then(|v| v.as_str())
        .ok_or((StatusCode::BAD_REQUEST, "Missing language".to_string()))?;

    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    store
        .add_language(language)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

/// Remove a language
async fn remove_language(
    State(state): State<AppState>,
    Query(params): Query<FileQuery>,
    Path(language): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let store = state
        .manager
        .store_for(params.file.as_deref())
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    store
        .remove_language(&language)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "success": true })))
}

// ==================== Embedded SPA HTML ====================

const SPA_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Gettext Translation Manager</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 20px;
        }

        .container {
            max-width: 1400px;
            margin: 0 auto;
        }

        header {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
            margin-bottom: 20px;
        }

        h1 {
            color: #667eea;
            margin-bottom: 10px;
        }

        .controls {
            display: flex;
            gap: 15px;
            flex-wrap: wrap;
            align-items: center;
        }

        input[type="text"], select {
            padding: 8px 12px;
            border: 1px solid #ddd;
            border-radius: 4px;
            font-size: 14px;
        }

        input[type="text"] {
            flex: 1;
            min-width: 200px;
        }

        button {
            padding: 8px 16px;
            background: #667eea;
            color: white;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            font-weight: 500;
            transition: background 0.3s;
        }

        button:hover {
            background: #5568d3;
        }

        button.secondary {
            background: #f0f0f0;
            color: #333;
        }

        button.secondary:hover {
            background: #e0e0e0;
        }

        button.danger {
            background: #e74c3c;
        }

        button.danger:hover {
            background: #c0392b;
        }

        .main-content {
            display: grid;
            grid-template-columns: 1fr 2fr;
            gap: 20px;
        }

        .sidebar {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
            height: fit-content;
        }

        .sidebar h2 {
            color: #667eea;
            margin-bottom: 15px;
            font-size: 16px;
        }

        .file-list {
            list-style: none;
        }

        .file-group {
            margin-bottom: 8px;
        }

        .file-group-header {
            padding: 8px 10px;
            background: #667eea;
            color: white;
            border-radius: 4px;
            cursor: pointer;
            font-weight: 600;
            font-size: 13px;
            display: flex;
            justify-content: space-between;
            align-items: center;
            user-select: none;
        }

        .file-group-header:hover {
            background: #5568d3;
        }

        .file-group-header .arrow {
            transition: transform 0.2s;
            font-size: 10px;
        }

        .file-group-header.collapsed .arrow {
            transform: rotate(-90deg);
        }

        .file-group-items {
            list-style: none;
            padding-left: 0;
        }

        .file-group-items.hidden {
            display: none;
        }

        .file-list li {
            padding: 8px 10px 8px 20px;
            margin-top: 2px;
            background: #f5f5f5;
            border-radius: 4px;
            cursor: pointer;
            transition: background 0.2s;
            font-size: 13px;
        }

        .file-list li:hover {
            background: #e8e8ff;
        }

        .file-list li.active {
            background: #e8e8ff;
            color: #667eea;
            font-weight: 600;
        }

        .language-list {
            list-style: none;
            margin-top: 15px;
        }

        .language-list li {
            padding: 8px;
            background: #f5f5f5;
            border-radius: 4px;
            margin-bottom: 5px;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }

        .language-list li button {
            padding: 4px 8px;
            font-size: 12px;
        }

        .content-area {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }

        .stats {
            display: flex;
            gap: 20px;
            margin-bottom: 20px;
            flex-wrap: wrap;
        }

        .stat-card {
            background: #f8f9fa;
            padding: 15px;
            border-radius: 4px;
            border-left: 4px solid #667eea;
        }

        .stat-card strong {
            display: block;
            color: #667eea;
            margin-bottom: 5px;
        }

        .translations-table {
            width: 100%;
            border-collapse: collapse;
            margin-top: 20px;
        }

        .translations-table thead {
            background: #f5f5f5;
        }

        .translations-table th {
            padding: 12px;
            text-align: left;
            font-weight: 600;
            color: #333;
            border-bottom: 2px solid #ddd;
        }

        .translations-table td {
            padding: 12px;
            border-bottom: 1px solid #eee;
        }

        .translations-table tr:hover {
            background: #f9f9f9;
        }

        .status-badge {
            display: inline-block;
            padding: 4px 8px;
            border-radius: 3px;
            font-size: 12px;
            font-weight: 500;
        }

        .status-translated {
            background: #d4edda;
            color: #155724;
        }

        .status-untranslated {
            background: #f8d7da;
            color: #721c24;
        }

        .status-fuzzy {
            background: #fff3cd;
            color: #856404;
        }

        .action-buttons {
            display: flex;
            gap: 8px;
        }

        .action-buttons button {
            padding: 4px 8px;
            font-size: 12px;
        }

        .modal {
            display: none;
            position: fixed;
            top: 0;
            left: 0;
            width: 100%;
            height: 100%;
            background: rgba(0,0,0,0.5);
            z-index: 1000;
            align-items: center;
            justify-content: center;
        }

        .modal.active {
            display: flex;
        }

        .modal-content {
            background: white;
            padding: 30px;
            border-radius: 8px;
            max-width: 600px;
            width: 90%;
            max-height: 80vh;
            overflow-y: auto;
        }

        .modal-content h2 {
            color: #667eea;
            margin-bottom: 20px;
        }

        .form-group {
            margin-bottom: 15px;
        }

        .form-group label {
            display: block;
            margin-bottom: 5px;
            color: #333;
            font-weight: 500;
        }

        .form-group input,
        .form-group textarea {
            width: 100%;
            padding: 8px;
            border: 1px solid #ddd;
            border-radius: 4px;
            font-family: inherit;
        }

        .form-group textarea {
            resize: vertical;
            min-height: 60px;
        }

        .form-actions {
            display: flex;
            gap: 10px;
            margin-top: 20px;
            justify-content: flex-end;
        }

        .progress-bar {
            width: 100%;
            height: 20px;
            background: #e0e0e0;
            border-radius: 10px;
            overflow: hidden;
            margin-top: 10px;
        }

        .progress-fill {
            height: 100%;
            background: linear-gradient(90deg, #667eea, #764ba2);
            display: flex;
            align-items: center;
            justify-content: center;
            color: white;
            font-size: 12px;
            transition: width 0.3s;
        }

        .empty-state {
            text-align: center;
            padding: 40px;
            color: #999;
        }

        .empty-state p {
            margin-top: 10px;
        }
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📚 Gettext Translation Manager</h1>
            <div class="controls">
                <select id="fileSelector" onchange="loadFile(this.value)">
                    <option value="">Select a file...</option>
                </select>
                <input type="text" id="searchInput" placeholder="Search translations..." onkeyup="filterTranslations()">
                <button onclick="openNewTranslationModal()">+ New Translation</button>
            </div>
        </header>

        <div class="main-content">
            <aside class="sidebar">
                <h2>Files</h2>
                <ul class="file-list" id="fileList"></ul>

                <h2>Languages</h2>
                <ul class="language-list" id="languageList"></ul>
                <button class="secondary" onclick="openAddLanguageModal()">+ Add Language</button>

                <h2>Stats</h2>
                <div id="sidebarStats"></div>
            </aside>

            <main class="content-area">
                <div class="stats" id="stats"></div>
                <table class="translations-table" id="translationsTable">
                    <thead>
                        <tr>
                            <th>Message ID</th>
                            <th>Translation</th>
                            <th>Status</th>
                            <th>Actions</th>
                        </tr>
                    </thead>
                    <tbody id="translationsBody">
                    </tbody>
                </table>
                <div class="empty-state" id="emptyState" style="display: none;">
                    <p>📭 No translations found</p>
                    <p>Start by selecting a file or creating a new translation</p>
                </div>
            </main>
        </div>
    </div>

    <!-- Edit Translation Modal -->
    <div class="modal" id="editModal">
        <div class="modal-content">
            <h2>Edit Translation</h2>
            <form onsubmit="saveTranslation(event)">
                <div class="form-group">
                    <label>Message ID</label>
                    <input type="text" id="editMsgid" readonly>
                </div>
                <div class="form-group">
                    <label>Translation</label>
                    <textarea id="editMsgstr" required></textarea>
                </div>
                <div class="form-group">
                    <label>Plural Form (if applicable)</label>
                    <textarea id="editMsgidPlural"></textarea>
                </div>
                <div class="form-actions">
                    <button type="button" onclick="closeModal('editModal')" class="secondary">Cancel</button>
                    <button type="submit">Save</button>
                </div>
            </form>
        </div>
    </div>

    <!-- New Translation Modal -->
    <div class="modal" id="newTranslationModal">
        <div class="modal-content">
            <h2>New Translation</h2>
            <form onsubmit="createTranslation(event)">
                <div class="form-group">
                    <label>Message ID</label>
                    <input type="text" id="newMsgid" required>
                </div>
                <div class="form-group">
                    <label>Translation</label>
                    <textarea id="newMsgstr" required></textarea>
                </div>
                <div class="form-actions">
                    <button type="button" onclick="closeModal('newTranslationModal')" class="secondary">Cancel</button>
                    <button type="submit">Create</button>
                </div>
            </form>
        </div>
    </div>

    <!-- Add Language Modal -->
    <div class="modal" id="addLanguageModal">
        <div class="modal-content">
            <h2>Add Language</h2>
            <form onsubmit="addLanguage(event)">
                <div class="form-group">
                    <label>Language Code</label>
                    <input type="text" id="languageCode" placeholder="e.g., fr, es, de" required>
                </div>
                <div class="form-actions">
                    <button type="button" onclick="closeModal('addLanguageModal')" class="secondary">Cancel</button>
                    <button type="submit">Add</button>
                </div>
            </form>
        </div>
    </div>

    <script>
        let currentFile = null;
        let allTranslations = [];

        // Initialize
        document.addEventListener('DOMContentLoaded', () => {
            loadFiles();
        });

        async function loadFiles() {
            try {
                const response = await fetch('/api/files');
                const files = await response.json();
                const fileList = document.getElementById('fileList');
                fileList.innerHTML = '';

                // Group files by language
                const groups = {};
                files.forEach(file => {
                    const group = file.language || 'Templates';
                    if (!groups[group]) groups[group] = [];
                    groups[group].push(file);
                });

                // Sort group keys: Templates first, then languages alphabetically
                const sortedKeys = Object.keys(groups).sort((a, b) => {
                    if (a === 'Templates') return -1;
                    if (b === 'Templates') return 1;
                    return a.localeCompare(b);
                });

                // Also populate the file selector dropdown
                const selector = document.getElementById('fileSelector');
                selector.innerHTML = '<option value="">Select a file...</option>';

                sortedKeys.forEach(group => {
                    const div = document.createElement('div');
                    div.className = 'file-group';

                    const header = document.createElement('div');
                    header.className = 'file-group-header';
                    header.innerHTML = '<span>' + group + ' (' + groups[group].length + ')</span><span class="arrow">&#9660;</span>';
                    header.onclick = () => {
                        header.classList.toggle('collapsed');
                        items.classList.toggle('hidden');
                    };

                    const items = document.createElement('ul');
                    items.className = 'file-group-items';

                    // Add optgroup to selector
                    const optgroup = document.createElement('optgroup');
                    optgroup.label = group;

                    groups[group].forEach(file => {
                        const li = document.createElement('li');
                        li.textContent = file.name;
                        li.dataset.path = file.path;
                        li.onclick = () => loadFile(file.path);
                        items.appendChild(li);

                        const opt = document.createElement('option');
                        opt.value = file.path;
                        opt.textContent = file.name;
                        optgroup.appendChild(opt);
                    });

                    div.appendChild(header);
                    div.appendChild(items);
                    fileList.appendChild(div);
                    selector.appendChild(optgroup);
                });

                if (files.length > 0) {
                    loadFile(files[0].path);
                }
            } catch (err) {
                console.error('Error loading files:', err);
            }
        }

        async function loadFile(path) {
            if (!path) return;
            currentFile = path;
            document.getElementById('fileSelector').value = path;

            // Update active file in sidebar
            document.querySelectorAll('.file-group-items li').forEach(li => {
                li.classList.remove('active');
                if (li.dataset.path === path) {
                    li.classList.add('active');
                }
            });

            // Load translations and languages
            await loadTranslations();
            await loadLanguages();
            await updateStats();
        }

        async function loadTranslations() {
            try {
                const response = await fetch(`/api/translations?file=${encodeURIComponent(currentFile)}`);
                allTranslations = await response.json();
                renderTranslations(allTranslations);
            } catch (err) {
                console.error('Error loading translations:', err);
            }
        }

        async function loadLanguages() {
            try {
                const response = await fetch(`/api/languages?file=${encodeURIComponent(currentFile)}`);
                const languages = await response.json();
                const languageList = document.getElementById('languageList');
                languageList.innerHTML = '';

                languages.forEach(lang => {
                    const li = document.createElement('li');
                    li.appendChild(document.createTextNode(lang + ' '));
                    const btn = document.createElement('button');
                    btn.className = 'danger';
                    btn.type = 'button';
                    btn.textContent = 'Remove';
                    btn.addEventListener('click', () => removeLanguage(lang));
                    li.appendChild(btn);
                    languageList.appendChild(li);
                });
            } catch (err) {
                console.error('Error loading languages:', err);
            }
        }

        async function updateStats() {
            const total = allTranslations.length;
            const translated = allTranslations.filter(t => t.is_translated).length;
            const fuzzy = allTranslations.filter(t => t.is_fuzzy).length;
            const percentage = total > 0 ? Math.round((translated / total) * 100) : 0;

            const statsHtml = `
                <div class="stat-card">
                    <strong>Total</strong>
                    ${total}
                </div>
                <div class="stat-card">
                    <strong>Translated</strong>
                    ${translated}
                </div>
                <div class="stat-card">
                    <strong>Fuzzy</strong>
                    ${fuzzy}
                </div>
                <div class="stat-card">
                    <strong>Progress</strong>
                    <div class="progress-bar">
                        <div class="progress-fill" style="width: ${percentage}%">${percentage}%</div>
                    </div>
                </div>
            `;
            document.getElementById('stats').innerHTML = statsHtml;
            document.getElementById('sidebarStats').innerHTML = `
                <strong>Total:</strong> ${total}<br>
                <strong>Translated:</strong> ${translated}<br>
                <strong>Coverage:</strong> ${percentage}%
            `;
        }

        function renderTranslations(translations) {
            const tbody = document.getElementById('translationsBody');
            const emptyState = document.getElementById('emptyState');

            if (translations.length === 0) {
                tbody.innerHTML = '';
                emptyState.style.display = 'block';
                return;
            }

            emptyState.style.display = 'none';
            tbody.innerHTML = '';
            translations.forEach((t, idx) => {
                const tr = document.createElement('tr');

                const tdId = document.createElement('td');
                const code = document.createElement('code');
                code.textContent = t.msgid;
                tdId.appendChild(code);

                const tdStr = document.createElement('td');
                tdStr.textContent = t.msgstr || '(untranslated)';

                const tdStatus = document.createElement('td');
                if (t.is_translated) {
                    const span = document.createElement('span');
                    span.className = 'status-badge status-translated';
                    span.textContent = 'Translated';
                    tdStatus.appendChild(span);
                }
                if (t.is_fuzzy) {
                    const span = document.createElement('span');
                    span.className = 'status-badge status-fuzzy';
                    span.textContent = 'Fuzzy';
                    tdStatus.appendChild(span);
                }
                if (!t.is_translated && !t.is_fuzzy) {
                    const span = document.createElement('span');
                    span.className = 'status-badge status-untranslated';
                    span.textContent = 'Untranslated';
                    tdStatus.appendChild(span);
                }

                const tdActions = document.createElement('td');
                const div = document.createElement('div');
                div.className = 'action-buttons';
                const editBtn = document.createElement('button');
                editBtn.textContent = 'Edit';
                editBtn.addEventListener('click', () => openEditModal(t.msgid, t.msgstr, t.msgctxt));
                const deleteBtn = document.createElement('button');
                deleteBtn.className = 'danger';
                deleteBtn.textContent = 'Delete';
                deleteBtn.addEventListener('click', () => deleteTranslation(t.msgid, t.msgctxt));
                div.appendChild(editBtn);
                div.appendChild(deleteBtn);
                tdActions.appendChild(div);

                tr.appendChild(tdId);
                tr.appendChild(tdStr);
                tr.appendChild(tdStatus);
                tr.appendChild(tdActions);
                tbody.appendChild(tr);
            });
        }

        function filterTranslations() {
            const query = document.getElementById('searchInput').value;
            const filtered = allTranslations.filter(t =>
                (t.msgid || '').toLowerCase().includes(query.toLowerCase()) ||
                (t.msgstr || '').toLowerCase().includes(query.toLowerCase())
            );
            renderTranslations(filtered);
        }

        let editMsgctxt = null;
        function openEditModal(msgid, msgstr, msgctxt) {
            document.getElementById('editMsgid').value = msgid;
            document.getElementById('editMsgstr').value = msgstr;
            editMsgctxt = msgctxt || null;
            document.getElementById('editModal').classList.add('active');
        }

        function openNewTranslationModal() {
            if (!currentFile) {
                alert('Please select a file first');
                return;
            }
            document.getElementById('newTranslationModal').classList.add('active');
        }

        function openAddLanguageModal() {
            if (!currentFile) {
                alert('Please select a file first');
                return;
            }
            document.getElementById('addLanguageModal').classList.add('active');
        }

        function closeModal(modalId) {
            document.getElementById(modalId).classList.remove('active');
        }

        async function saveTranslation(event) {
            event.preventDefault();
            const msgid = document.getElementById('editMsgid').value;
            const msgstr = document.getElementById('editMsgstr').value;

            try {
                const response = await fetch(`/api/translations?file=${encodeURIComponent(currentFile)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        msgid,
                        msgstr,
                        msgctxt: editMsgctxt
                    })
                });

                if (response.ok) {
                    closeModal('editModal');
                    await loadTranslations();
                    await updateStats();
                } else {
                    alert('Error saving translation');
                }
            } catch (err) {
                console.error('Error:', err);
                alert('Error saving translation');
            }
        }

        async function createTranslation(event) {
            event.preventDefault();
            const msgid = document.getElementById('newMsgid').value;
            const msgstr = document.getElementById('newMsgstr').value;

            try {
                const response = await fetch(`/api/translations?file=${encodeURIComponent(currentFile)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        msgid,
                        msgstr
                    })
                });

                if (response.ok) {
                    closeModal('newTranslationModal');
                    document.getElementById('newMsgid').value = '';
                    document.getElementById('newMsgstr').value = '';
                    await loadTranslations();
                    await updateStats();
                } else {
                    alert('Error creating translation');
                }
            } catch (err) {
                console.error('Error:', err);
                alert('Error creating translation');
            }
        }

        async function deleteTranslation(msgid, msgctxt) {
            if (!confirm(`Delete translation for "${msgid}"?`)) return;

            try {
                let url = `/api/translations/detail?file=${encodeURIComponent(currentFile)}&msgid=${encodeURIComponent(msgid)}`;
                if (msgctxt) url += `&msgctxt=${encodeURIComponent(msgctxt)}`;
                const response = await fetch(url, {
                    method: 'DELETE'
                });

                if (response.ok) {
                    await loadTranslations();
                    await updateStats();
                } else {
                    alert('Error deleting translation');
                }
            } catch (err) {
                console.error('Error:', err);
                alert('Error deleting translation');
            }
        }

        async function addLanguage(event) {
            event.preventDefault();
            const language = document.getElementById('languageCode').value;

            try {
                const response = await fetch(`/api/languages?file=${encodeURIComponent(currentFile)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        language
                    })
                });

                if (response.ok) {
                    closeModal('addLanguageModal');
                    document.getElementById('languageCode').value = '';
                    await loadLanguages();
                } else {
                    alert('Error adding language');
                }
            } catch (err) {
                console.error('Error:', err);
                alert('Error adding language');
            }
        }

        async function removeLanguage(language) {
            if (!confirm(`Remove language "${language}"?`)) return;

            try {
                const response = await fetch(`/api/languages/${encodeURIComponent(language)}?file=${encodeURIComponent(currentFile)}`, {
                    method: 'DELETE'
                });

                if (response.ok) {
                    await loadLanguages();
                } else {
                    alert('Error removing language');
                }
            } catch (err) {
                console.error('Error:', err);
                alert('Error removing language');
            }
        }

    </script>
</body>
</html>"#;
