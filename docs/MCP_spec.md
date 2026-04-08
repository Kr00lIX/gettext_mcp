# MCP Server Architecture Specification
## Based on xcstrings-mcp (Xcode String Catalogs)

This document outlines the architectural pattern and implementation approach of the xcstrings-mcp project, designed to be used as a reference for building similar MCP servers for other localization formats (e.g., gettext .po/.pot files).

---

## Overview

An MCP (Model Context Protocol) server is a specialized service that:
- Runs as a standalone process communicating via stdio
- Exposes "tools" (functions) that AI clients can invoke
- Manages a domain-specific file format (in this case, `.xcstrings` JSON)
- Optionally provides a web UI for human interaction
- Persists changes to disk after every modification

The architecture is designed to:
1. **Be format-agnostic**: Core patterns apply to any structured text format
2. **Support dual-mode operation**: Default path or dynamic path discovery
3. **Provide multiple interfaces**: MCP tools + optional web UI
4. **Handle concurrent access**: Thread-safe in-memory store with async I/O
5. **Maintain data integrity**: Serialize to disk after each change

---

## Core Architecture Components

### 1. **Store Layer** (`src/store.rs`)

Handles all file I/O, parsing, and in-memory caching.

**Key Responsibilities:**
- Parse format-specific file into structured data (JSON deserialization)
- Maintain in-memory representation with RwLock for thread safety
- Persist changes to disk on every modification
- Provide CRUD operations for domain entities
- Track file state and reload from disk when needed

**Key Structures:**

```rust
// Main file representation
pub struct XcStringsFile {
    raw: IndexMap<String, serde_json::Value>,  // Preserve field order
    version: String,
    source_language: String,
    strings: IndexMap<String, XcStringEntry>,  // Keys → translations
}

// Individual entry
pub struct XcStringEntry {
    localizations: IndexMap<String, Localization>,  // Language → value
    comment: Option<String>,
    extraction_state: Option<String>,
}

// Store wrapper with async I/O
pub struct XcStringsStore {
    path: PathBuf,
    data: Arc<RwLock<XcStringsFile>>,  // Thread-safe + async-aware
}

// Manager handles multiple stores (dynamic path mode)
pub struct XcStringsStoreManager {
    default_path: Option<PathBuf>,
    stores: Arc<RwLock<HashMap<PathBuf, Arc<XcStringsStore>>>>,
    discovered_paths: Arc<RwLock<Vec<PathBuf>>>,
}
```

**Key Operations:**

1. **Loading**
   - Parse format → Structured data
   - Create IndexMap to preserve field order
   - Cache parsed values for quick access
   - Return default structure if file doesn't exist

2. **Persisting**
   - Convert in-memory structure back to format
   - Write to disk (atomic if possible)
   - Use async I/O (`tokio::fs`)
   - Format preservation for field order/comments/etc

3. **Caching Strategy**
   - Keep one store per unique path in memory
   - Lock on write, unlock immediately after
   - Lazy load stores on first access
   - Support reload from disk for external changes

**Error Handling:**

```rust
pub enum StoreError {
    ReadFailed(std::io::Error),
    SerdeFailed(serde_json::Error),
    TranslationMissing { key, language },
    KeyMissing(String),
    PathRequired,
    LanguageMissing(String),
    InvalidLanguage(String),
    CannotRemoveSourceLanguage(String),
    // ... format-specific errors
}
```

---

### 2. **MCP Server Layer** (`src/mcp_server.rs`)

Defines tools exposed to MCP clients.

**Architecture:**

```rust
pub struct XcStringsMcpServer {
    stores: Arc<XcStringsStoreManager>,
    tool_router: ToolRouter<Self>,  // Handles tool dispatch
}
```

**Tool Categories:**

#### A. Translation CRUD
- `list_translations(path, query?, limit?)` - List with filtering & pagination
- `get_translation(path, key, language)` - Fetch single entry
- `upsert_translation(path, key, language, value?, state?, ...)` - Create/update
- `delete_translation(path, key, language)` - Remove by language
- `delete_key(path, key)` - Remove entirely across all languages

#### B. Metadata Management
- `set_comment(path, key, comment?)` - Set/clear comments
- `set_translation_state(path, key, language, state?)` - Update state field
- `set_extraction_state(path, key, extractionState?)` - Update extraction state

#### C. Language Management
- `list_languages(path)` - Enumerate languages
- `add_language(path, language)` - Add with placeholders
- `remove_language(path, language)` - Remove (except source)
- `update_language(path, oldLang, newLang)` - Rename language
- `list_untranslated(path)` - Find incomplete entries

**Tool Implementation Pattern:**

```rust
#[tool(description = "Tool description")]
async fn tool_name(
    &self,
    params: Parameters<ToolParams>,
) -> Result<CallToolResult, McpError> {
    let params = params.0;
    let store = self.store_for(Some(params.path.as_str())).await?;

    // Perform operation
    store.operation(&params).await.map_err(Self::error_to_mcp)?;

    // Return JSON response
    Ok(render_response(data))
}
```

**Error Mapping:**

```rust
fn error_to_mcp(err: StoreError) -> McpError {
    match err {
        StoreError::KeyMissing(key) =>
            McpError::resource_not_found(format!("Key '{key}' not found"), None),
        StoreError::TranslationMissing { key, language } =>
            McpError::resource_not_found(format!("Translation not found"), None),
        StoreError::PathRequired =>
            McpError::invalid_params("Path must be provided", None),
        _ => McpError::internal_error(err.to_string(), None),
    }
}
```

**Response Format:**

All tools return JSON-encoded text:
```rust
fn render_translations(data: Vec<TranslationRecord>) -> CallToolResult {
    CallToolResult {
        content: vec![Content::text(serde_json::to_string(&data).unwrap())],
    }
}
```

---

### 3. **Web UI Layer** (`src/web/mod.rs`)

Optional HTTP server for human-friendly interface.

**Framework:** Axum (async, lightweight)

**Architecture:**

```rust
pub async fn serve(addr: SocketAddr, manager: Arc<XcStringsStoreManager>) {
    let app = Router::new()
        .route("/", get(index_html))
        .route("/api/files", get(list_files))
        .route("/api/translations", get(get_translations))
        .route("/api/translations", post(upsert_translation))
        .route("/api/languages", get(list_languages))
        // ... more routes

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?
}
```

**Key Features:**
- SPA (Single Page Application) served as embedded HTML
- REST API for CRUD operations
- Real-time search/filtering
- File selector for dynamic path mode
- Progress tracking (% translated)

**Routes:**
- `GET /` - Serve index.html
- `GET /api/files` - List discovered .xcstrings files
- `GET /api/translations?file=...&query=...` - List translations
- `POST /api/translations` - Upsert translation
- `GET /api/languages?file=...` - List languages
- `POST/DELETE /api/languages?file=...` - Manage languages

---

### 4. **Main Entrypoint** (`src/main.rs`)

Orchestrates all components.

**Flow:**

```
Config::from_env()
    ├── Parse CLI args and env vars
    └── Determine: default_path, web_addr

XcStringsStoreManager::new(default_path)
    ├── Auto-discover files if no default_path
    └── Create store cache

Spawn web server (if WEB_PORT/WEB_HOST set)
    └── Listen on configured address

Spawn MCP server
    ├── Create router with all tools
    └── Read from stdin, write to stdout

Wait for Ctrl+C or task failure
    └── Graceful shutdown
```

**Configuration:**

```rust
struct Config {
    path: Option<PathBuf>,      // From CLI arg or STRINGS_PATH env var
    web_addr: Option<SocketAddr>, // From WEB_HOST/WEB_PORT env vars
}
```

---

## Data Flow Patterns

### Read Path
```
MCP Tool Call (stdin)
    ↓
ToolRouter::dispatch()
    ↓
tool_handler receives Parameters
    ↓
store_for(path) → Arc<XcStringsStore>
    ↓
Store::operation() reads Arc<RwLock<T>>
    ↓
serialize() to JSON
    ↓
render_response() → CallToolResult
    ↓
Write to stdout (JSON text)
```

### Write Path
```
MCP Tool Call (stdin)
    ↓
ToolRouter::dispatch()
    ↓
tool_handler receives Parameters
    ↓
store_for(path) → Arc<XcStringsStore>
    ↓
Store::operation() writes to Arc<RwLock<T>>
    ↓
serialize() to format
    ↓
fs::write() to disk (atomic)
    ↓
render_response() → CallToolResult
    ↓
Write to stdout (JSON text)
```

---

## Deployment Modes

### Mode 1: Default Path (Single File)
```bash
cargo run -- /path/to/Localizable.xcstrings
```
- Tools can omit `path` parameter
- Web UI shows single file
- Faster startup (no discovery)

### Mode 2: Dynamic Path (Multiple Files)
```bash
cargo run
```
- Tools must provide `path` parameter
- Web UI auto-discovers and shows file selector
- Supports switching between files at runtime
- Slightly slower startup (discovery scan)

### Mode 3: With Web UI
```bash
WEB_PORT=8787 cargo run -- /path/to/file.xcstrings
```
- MCP + HTTP server run concurrently
- Web UI at `http://127.0.0.1:8787`
- Both interfaces access same store
- State is shared

---

---

## Context

This specification outlines the architecture and implementation steps for building a Gettext MCP (Model Context Protocol) server in Rust, following the patterns established by xcstrings-mcp.

### Success Criteria
- [x] Gettext MCP server fully functional
- [x] All MCP tools implemented and tested
- [x] Optional web UI (Nice to have)
- [x] Documentation and examples provided

---

### Task 1: Core Data Structures & PO Format Parser

Implement the foundational data structures for representing PO files in memory and create a parser that can read/write PO format while preserving structure.

- [x] Define PO file data structures (src/store.rs)
  - Message entry struct (msgid, msgstr, msgctxt, flags, comments)
  - Header entry struct
  - Comment types (developer, translator, extracted)
  - Plural form support (msgid_plural, msgstr[n])
  - Flag enum (fuzzy, c-format, python-format, etc)

- [x] Implement PO format parser
  - Parse message entries from PO text format
  - Parse header metadata
  - Handle comment prefixes (#, #., #:, #,, #|)
  - Extract plural rules from headers
  - Preserve formatting on write-back

- [x] Implement GettextStore equivalent (src/store.rs)
  - Load/parse PO file into in-memory IndexMap
  - Persist changes back to PO format
  - Thread-safe access with Arc<RwLock<T>>
  - Support reload from disk for external changes

- [x] Define error types matching PO semantics (src/store.rs)
  - StoreError enum for format/parsing errors
  - Map to MCP error types in server layer

---

### Task 2: MCP Server Tools - Core CRUD & Metadata

Implement the MCP server layer with tools for translation management, metadata operations, and file handling.

- [x] Implement core CRUD tools (src/mcp_server.rs)
  - `list_translations(path, query?, limit?)` - List with filtering
  - `get_translation(path, msgid, msgctxt?)` - Fetch single entry
  - `upsert_translation(path, msgid, msgstr, msgctxt?, state?)` - Create/update
  - `delete_translation(path, msgid, msgctxt?)` - Remove entry
  - `delete_key(path, msgid)` - Remove all contexts

- [x] Implement metadata tools (src/mcp_server.rs)
  - `set_comment(path, msgid, comment?)` - Set/clear comments
  - `set_fuzzy(path, msgid, fuzzy: bool)` - Toggle fuzzy flag
  - `set_flag(path, msgid, flag, enabled: bool)` - Manage c-format, etc

- [x] Implement file/language management tools (src/mcp_server.rs)
  - `list_metadata(path)` - Return encoding, plural forms, language
  - `set_header(path, key, value)` - Update metadata headers
  - `list_contexts(path)` - Get all msgctxt values

- [x] Error mapping and response formatting (src/mcp_server.rs)
  - Error types mapped to user-friendly messages
  - JSON response serialization for all tools
  - Consistent error messages

---

### Task 3: Web UI (Optional)

Build a human-friendly web interface for browsing and editing translations.

- [x] Create SPA structure (src/web/mod.rs)
  - Embedded HTML/CSS/JS
  - File selector for multiple .po files
  - Search/filter interface
  - Translation progress indicator

- [x] Implement REST API routes (src/web/mod.rs)
  - GET /api/files - List discovered files
  - GET /api/translations - List with filtering
  - POST /api/translations - Upsert translation
  - GET /api/metadata - File metadata
  - POST/DELETE /api/languages - Manage languages

- [x] Frontend features
  - Real-time search of translations
  - Display fuzzy/untranslated status
  - Plural form editing
  - Comment display and editing

---

### Task 4: Integration, Testing & Documentation

Complete the implementation with comprehensive tests, examples, and documentation.

- [x] Unit tests
  - Parser tests (round-trip format preservation)
  - Store CRUD operations
  - Concurrent access/thread safety
  - Error handling

- [x] Integration tests
  - Full MCP tool lifecycle
  - Web UI API endpoints
  - File discovery and caching
  - Multi-file operations

- [x] Example files and documentation
  - Sample .po/.pot files
  - README with setup instructions
  - Tool API documentation
  - Architecture notes
  - Example usage in Claude

- [x] Project setup & build
  - Cargo.toml with dependencies
  - build.rs if needed
  - GitHub Actions CI (optional)
  - Release checklist

---

## Key Design Patterns

### 1. **Arc<RwLock<T>> for Async Sharing**
- Thread-safe by default
- Multiple readers OR one writer
- Async-aware (`.await` on lock)

### 2. **IndexMap for Order Preservation**
- Maintains insertion order (like Python dicts)
- Serializes in order (matches original file format)
- Better for diffs and human readability

### 3. **Option<Option<T>> for Patch Operations**
```rust
pub struct UpsertTranslationParams {
    pub value: Option<Option<String>>,  // Three states:
    // None = don't touch
    // Some(None) = clear/delete
    // Some(Some(s)) = set to s
}
```

### 4. **Manager Pattern for Multi-File**
```rust
pub struct XcStringsStoreManager {
    stores: HashMap<PathBuf, Arc<XcStringsStore>>,
    // ↑ Each path gets its own store
    // Reuse same store for repeated calls
    // Lazy-load on first access
}
```

### 5. **Error Mapping to MCP Errors**
```rust
fn error_to_mcp(err: StoreError) -> McpError {
    // Map domain errors to MCP error types
    // Preserves semantic meaning across boundary
}
```

### 6. **JSON-First Responses**
```rust
// Every tool returns JSON-encoded text
// Not raw structs — makes debugging easier
// Clients can parse or inspect raw JSON
```

---

## Dependencies & Their Roles

```toml
rmcp = "0.5"                    # MCP protocol implementation
tokio = "1.37"                  # Async runtime
serde = "1"                     # Serialization
serde_json = "1"                # JSON encoding (with preserve_order)
indexmap = "2"                  # Order-preserving map
schemars = "1"                  # JSON schema generation for tools
thiserror = "1"                 # Error type derivation
axum = "0.7"                    # HTTP framework
tower = "0.4"                   # Middleware/service composition
tracing = "0.1"                 # Structured logging
async-trait = "0.1"             # Async trait support
```

---

## Testing Strategy

```
tests/
├── format_preservation.rs      # Verify format is unchanged
├── store/
│   ├── load_create.rs         # Store initialization
│   ├── crud.rs                # Create/read/update/delete
│   └── concurrency.rs         # Thread safety
└── mcp_server/
    ├── tools.rs               # Tool behavior
    └── error_handling.rs      # Error cases
```

**Key Test Patterns:**
```rust
#[tokio::test]
async fn test_name() {
    let path = fresh_store_path("test_name");
    let manager = XcStringsStoreManager::new(Some(path.clone())).await.unwrap();
    let store = manager.store_for(None).await.unwrap();

    // Perform operation
    store.operation(...).await.unwrap();

    // Reload and verify persistence
    let store2 = manager.store_for(None).await.unwrap();
    assert!(store2.operation(...).await.is_ok());
}
```

---

## Performance Considerations

1. **Memory**: Store entire file in memory
   - Fine for catalogs <1MB
   - For large catalogs: consider lazy-loading

2. **Disk I/O**: Writes on every change
   - Fine for interactive use (MCP tools)
   - For bulk operations: batch + single write

3. **Search**: Linear scan of all translations
   - Fine for <10k entries
   - For larger: implement indexing

4. **Concurrency**: RwLock with async
   - Allows multiple concurrent readers
   - Serializes writers
   - Designed for human-paced operations (not high-throughput)

---

## Appendix: File Format Considerations

### XCStrings Format Characteristics
- JSON-based (parsed by serde)
- Preserves field order (use IndexMap)
- Comments and metadata stored as JSON keys
- Supports nested variations (plurals, device types)
- Source language acts as fallback

### For Gettext .PO Format:
- Text-based (requires custom parser or existing crate)
- Comments with special prefixes (translator, extracted, etc.)
- Headers in first message (msgid "")
- Plural forms specified in headers
- Context (msgctxt) for disambiguation
- Flags (fuzzy, c-format, etc.)
- Line-based, whitespace-sensitive

**Recommended approach:**
- Use existing PO parser crate (e.g., `po` or `polib`)
- OR implement minimal parser (PO is simple)
- Store as intermediate struct (similar to XcStringsFile)
- Convert to/from struct on load/save
- Use same patterns as xcstrings-mcp

---

## Summary

The xcstrings-mcp architecture demonstrates a clean separation of concerns:

1. **Store** = File I/O + in-memory representation
2. **MCP Server** = Tool definitions + error mapping
3. **Web UI** = Optional HTTP server for UX
4. **Main** = Orchestration + configuration

This pattern is applicable to any structured file format with proper adaptation for format-specific parsing, serialization, and domain concepts.
