# Gettext MCP Server

Rust MCP server for managing GNU Gettext `.po`/`.pot` translation files. Exposes CRUD operations, fuzzy flag management, plural form handling, and an optional web UI over the Model Context Protocol.

## Build & Test

```bash
cargo build                    # Build debug binary
cargo build --release          # Build optimized binary
cargo test                     # Run all tests (unit + integration)
cargo test store::tests        # Run store/parser tests only
cargo test mcp_server::tests   # Run MCP server tests only
cargo test --test web_integration  # Run web integration tests only
```

### Running

```bash
# Single file mode (path param optional in MCP calls)
cargo run -- /path/to/messages.po

# Dynamic mode (path required in every MCP call)
cargo run

# With web UI
WEB_PORT=8787 cargo run -- /path/to/messages.po
# Then open http://127.0.0.1:8787
```

Logs go to stderr (required to avoid interfering with MCP stdio transport).

## Architecture

```
src/
├── main.rs          # Entry point, CLI args, spawns MCP + optional web server
├── lib.rs           # Public re-exports
├── store.rs         # Parser, serializer, GettextStore, GettextStoreManager
├── mcp_server.rs    # 11 MCP tool implementations + param types
└── web/
    └── mod.rs       # Axum REST API (10 endpoints) + embedded SPA

tests/
└── web_integration.rs  # Integration tests for store manager

examples/
├── sample_fr.po        # French translations with contexts, plurals, flags
└── sample_plurals.po   # Complex plural forms
```

### Layers

1. **Parser/Serializer** (`store.rs::parser`) — Line-by-line PO parsing via `classify_line()` state machine. `unescape_po_string()` on read, `escape_po_string()` on write. Handles all comment types (`#.`, `#:`, `#,`, `#|`, `#~`), multiline strings, and plural forms (`msgstr[n]`).

2. **Store** (`store.rs`) — `GettextStore` wraps a single PO file with `Arc<RwLock<GettextFile>>`. CRUD via `get()`, `upsert()`, `upsert_full()`, `delete()`, `delete_by_msgid()`, `update_entry()`. `GettextStoreManager` manages multiple files with path validation and caching.

3. **MCP Server** (`mcp_server.rs`) — 11 tools: `list_translations`, `get_translation`, `upsert_translation`, `delete_translation`, `delete_key`, `set_comment`, `set_fuzzy`, `set_flag`, `list_metadata`, `set_header`, `list_contexts`. All return `Result<serde_json::Value, String>`.

4. **Web UI** (`web/mod.rs`) — Axum HTTP server with REST API under `/api/` and embedded SPA at `/`. CORS restricted to localhost.

## Key Design Decisions

### Entry Keys

Entries are keyed by `(msgid: String, msgctxt: Option<String>)` tuple in an `IndexMap` to preserve insertion order. The header entry has key `("", None)`.

### Metadata Preservation

Use `update_entry()` (not `upsert()`) when modifying comments, flags, or other metadata on existing entries. `upsert()` only preserves msgstr and flags — it will discard comments, source locations, and other fields.

```rust
let mut entry = store.get(&msgid, msgctxt).await?;
entry.flags.push("fuzzy".to_string());
store.update_entry(&msgid, msgctxt, entry).await?;
```

### Path Validation

`validate_path()` in `GettextStoreManager` rejects `..` components and, when a default path is set, ensures paths stay within the base directory via canonicalization. Without a default path, absolute paths are rejected.

### Thread Safety

- `Arc<RwLock<GettextFile>>` — many concurrent readers OR one writer
- Every write persists to disk immediately
- Store cache in `GettextStoreManager` is unbounded (no LRU eviction yet)

### String Escaping

PO format escapes: `\\`, `\"`, `\n`, `\r`, `\t`. Parser unescapes on read, serializer escapes on write. Round-trip fidelity is tested.

## Testing

81 tests total across three test suites:

- **`store::tests`** (37 tests) — Parser edge cases (escapes, plurals, comments, obsolete lines), serialization round-trips, store CRUD, metadata/header ops, path validation, language management, is_translated semantics
- **`mcp_server::tests`** (23 tests) — All 11 MCP tools covered: CRUD, fuzzy/flag management, comments, metadata, plurals, contexts, query/limit filtering, error paths
- **`web_integration`** (10 tests) — Store manager, concurrency, round-trip persistence, plural forms, context handling, language management

### Test Patterns

- Parser tests: inline PO string literals → `parse_po()` → assert fields
- Store tests: `#[tokio::test]`, `tempfile::TempDir` for isolation, direct store API calls
- MCP tests: Create `GettextStoreManager` + `GettextMcpServer`, call tool methods with param structs, assert JSON values
- Round-trip tests: Build `GettextFile` → serialize → reparse → assert equality

## Dependencies

| Crate | Purpose |
|-------|---------|
| `rmcp` | Rust MCP protocol SDK (server, transport) |
| `tokio` | Async runtime (multi-thread, fs, signal) |
| `axum` | HTTP framework for web UI |
| `tower` / `tower-http` | CORS middleware |
| `serde` / `serde_json` | Serialization |
| `schemars` | JSON schema generation for MCP tool params |
| `indexmap` | Order-preserving map for PO entries |
| `thiserror` | Error type derivation |
| `tracing` / `tracing-subscriber` | Structured logging to stderr |
| `async-trait` | Async trait support |
| `tempfile` (dev) | Temp directories for tests |

## Known Limitations

- MCP stdio transport loop in `main.rs` is not yet connected via `rmcp` — currently waits on ctrl_c
- Store cache is unbounded; needs LRU eviction for long-running servers
- File writes are not atomic (should write to `.po.tmp` then rename)
- Web API endpoints have no dedicated test suite (only tested indirectly through store integration tests)
