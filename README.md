# Gettext MCP Server

[![CI](https://github.com/Kr00lIX/gettext_mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/Kr00lIX/gettext_mcp/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/gettext-mcp)](https://crates.io/crates/gettext-mcp)
[![MSRV](https://img.shields.io/badge/MSRV-1.75-blue)](https://blog.rust-lang.org/)
[![License](https://img.shields.io/crates/l/gettext-mcp)](LICENSE-MIT)
[![MCP](https://img.shields.io/badge/MCP-compatible-green)](https://modelcontextprotocol.io)

A Model Context Protocol server for GNU gettext `.po` / `.pot` files. Twelve structured tools cover the full translation lifecycle: read, edit, manage fuzzy flags and metadata, handle plural forms and contexts. An optional web UI ships in the same binary.

## Why this exists

`.po` files are deceptively annoying. They look like flat text, but a real catalog mixes multi-line `msgstr` continuations, escaped quotes and newlines, plural arrays (`msgstr[0]`, `msgstr[1]`, ...), context-disambiguated duplicates (`msgctxt`), obsolete entries marked with `#~`, and a handful of comment types (`#.`, `#:`, `#,`, `#|`) that each mean something different. Asking a model to edit one with `Edit`/`Bash` means either reading the whole file into context (token-expensive for any non-trivial catalog) or hand-rolling fragile regexes that break the moment a string crosses a line. Even one stray quote in `msgstr` quietly corrupts the file for `msgfmt`.

A structured MCP server sidesteps all of that. The model calls `get_translation` / `upsert_translation` / `set_fuzzy` with typed arguments; the server parses, escapes, and serializes correctly, persists to disk, and only ships the entries the model actually asked for back across the wire.

## Features

- 12 MCP tools covering translation CRUD, fuzzy/flag management, header metadata, plural forms, and context disambiguation
- Single-file, directory, and dynamic-path operating modes
- Optional web UI for browsing and editing the catalog from a browser
- Round-trip-safe PO parser/serializer: preserves comments, source references, flag order, header ordering, and obsolete `#~` entries
- Thread-safe in-memory store (`Arc<RwLock>`), changes flushed to disk on every write
- Tested with Claude Code, Cursor, VS Code + Copilot, Windsurf, and Zed; should work with any MCP client

## Install

```sh
cargo install gettext-mcp
# or, from a checkout:
cargo install --path .
```

The binary lands at `~/.cargo/bin/gettext-mcp`.

## Configure

<details>
<summary><strong>Claude Code</strong></summary>

```sh
claude mcp add gettext-mcp -- gettext-mcp
```

To pin the server to a specific catalog, pass the path:

```sh
claude mcp add gettext-mcp -- gettext-mcp /path/to/messages.po
```
</details>

<details>
<summary><strong>Cursor</strong></summary>

Add to `.cursor/mcp.json`:
```json
{
  "mcpServers": {
    "gettext-mcp": {
      "command": "gettext-mcp",
      "args": ["/path/to/messages.po"]
    }
  }
}
```
</details>

<details>
<summary><strong>Windsurf</strong></summary>

Add to `~/.codeium/windsurf/mcp_config.json`:
```json
{
  "mcpServers": {
    "gettext-mcp": {
      "command": "gettext-mcp",
      "args": ["/path/to/messages.po"]
    }
  }
}
```
</details>

<details>
<summary><strong>VS Code + Copilot</strong></summary>

Add to `.vscode/mcp.json`:
```json
{
  "servers": {
    "gettext-mcp": {
      "type": "stdio",
      "command": "gettext-mcp",
      "args": ["/path/to/messages.po"]
    }
  }
}
```
</details>

<details>
<summary><strong>Zed</strong></summary>

Add to Zed settings (`settings.json`):
```json
{
  "context_servers": {
    "gettext-mcp": {
      "command": {
        "path": "gettext-mcp",
        "args": ["/path/to/messages.po"]
      }
    }
  }
}
```
</details>

<details>
<summary><strong>Any MCP client (generic)</strong></summary>

`gettext-mcp` speaks JSON-RPC over stdio. Point your client at the binary:
```
command: gettext-mcp
args:    [optional path to a .po file or gettext directory]
transport: stdio
```
</details>

## Modes

The first CLI argument decides how the server resolves the `path` parameter on each tool call.

- **Single-file mode** — `gettext-mcp /path/to/messages.po`. The given file becomes the default; `path` is optional on every tool call.
- **Directory mode** — `gettext-mcp /path/to/locale`. The server scans the tree at startup for all `.po` / `.pot` files (e.g. `{lang}/LC_MESSAGES/*.po`). Tools accept relative paths within the base directory; `list_files` enumerates what was discovered.
- **Dynamic mode** — `gettext-mcp` with no argument. Every tool call must supply an absolute `path`. Useful for ad-hoc, multi-project use.

Path validation rejects `..` traversal, and in directory mode every path is canonicalized to stay inside the base directory.

## Tools

| Tool | Description |
|------|-------------|
| `list_translations` | List translation entries with optional case-insensitive substring `query` and `limit`. |
| `get_translation` | Get a single translation entry by `msgid` (and optional `msgctxt`). |
| `upsert_translation` | Create or update a translation entry. Preserves existing flags/comments when updating. |
| `delete_translation` | Clear the translation (`msgstr`) for an entry without removing the key. |
| `delete_key` | Remove every entry (across all contexts) with the given `msgid`. |
| `set_comment` | Set or clear the translator comment for an entry. Pass `comment: null` to clear. |
| `set_fuzzy` | Toggle the `fuzzy` flag on a translation entry. |
| `set_flag` | Add or remove an arbitrary flag (e.g. `c-format`, `no-wrap`) on an entry. |
| `list_metadata` | List all PO header metadata entries (Language, Plural-Forms, etc.). |
| `set_header` | Set or remove a single PO header entry. Pass `value: null` to remove. |
| `list_contexts` | List all distinct `msgctxt` values used in the file. |
| `list_files` | List all `.po`/`.pot` files discovered in directory mode. |

Every tool accepts an optional `path` parameter (required in dynamic mode). `upsert_translation` additionally accepts `msgid_plural` and `msgstr_plural` for plural forms, plus `flags` for arbitrary flag arrays.

## Web UI

Set `WEB_PORT` to spawn an Axum-backed web UI alongside the MCP server:

```sh
WEB_PORT=8787 gettext-mcp /path/to/locale
# open http://127.0.0.1:8787
```

Optional `WEB_HOST` overrides the bind address (default `127.0.0.1`). CORS is restricted to localhost. The UI offers:

- Translation browser with search and translated/fuzzy/untranslated status
- Inline editor for `msgid`, `msgstr`, plural forms, and translator comments
- Fuzzy flag toggle
- File switcher and per-file coverage stats in directory mode
- Language list with add/remove operations

## Build & Test

```sh
cargo build                       # debug build
cargo build --release             # release build
cargo test                        # full suite (unit + integration)
cargo test store::tests           # parser/store tests
cargo test mcp_server::tests      # MCP tool tests
cargo test --test web_integration # web integration tests
```

Logs go to stderr — required, because stdout is the MCP stdio transport.

## PO format support

The parser and serializer round-trip everything the GNU gettext spec defines:

```
# Translator comment
#. Extracted comment
#: source/file.py:42
#, fuzzy, c-format
#| msgid "Previous original"
msgctxt "context"
msgid "Original text"
msgid_plural "Original texts"
msgstr[0] "Singular translation"
msgstr[1] "Plural translation"
```

Obsolete entries (`#~`), multi-line string continuations, header ordering, and arbitrary flag combinations are preserved on write.

## Architecture

Three layers, plus an optional web frontend:

```
src/
├── main.rs        # CLI args, spawns MCP + optional web server
├── lib.rs         # Public re-exports
├── store.rs       # PO parser, serializer, GettextStore, GettextStoreManager
├── mcp_server.rs  # 12 MCP tools + ServerHandler stdio wiring
└── web/mod.rs     # Axum REST API + embedded SPA
```

- **Store** — `Arc<RwLock<GettextFile>>` per file. Entries keyed by `(msgid, msgctxt)` in an `IndexMap` for stable order. Writes flushed to disk immediately.
- **MCP server** — Per-tool `*Params` structs derive `JsonSchema` via `schemars`; `ServerHandler` wires `tools/list` and `tools/call` over stdio.
- **Web** — Axum router with `/api/*` REST endpoints mirroring the MCP tools, plus an embedded single-page UI.

See [`CLAUDE.md`](CLAUDE.md) for deeper architectural notes (entry keying, metadata preservation rules, thread-safety model).

## Examples

Sample catalogs in `examples/`:

- `sample_fr.po` — French translations with contexts, plurals, and flags
- `sample_plurals.po` — complex plural forms

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, testing, and PR requirements.

## Security

See [SECURITY.md](SECURITY.md) for the vulnerability reporting process.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Related

- [Model Context Protocol](https://modelcontextprotocol.io) — open protocol for AI tool integration
- [GNU gettext](https://www.gnu.org/software/gettext/) — the format and reference toolchain
- [xcstrings-mcp](https://github.com/Murzav/xcstrings-mcp) — sibling MCP server for Apple String Catalogs
