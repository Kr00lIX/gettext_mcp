# Gettext MCP Server

A Model Context Protocol (MCP) server for managing GNU gettext .po/.pot files. Provides tools for reading, editing, and managing translations with support for contexts, plural forms, flags, and metadata.

## Features

- **MCP Tools**: Full set of tools for translation CRUD operations
- **Web UI**: Optional human-friendly web interface for browsing and editing translations
- **Plural Form Support**: Handle msgid_plural and plural translations
- **Context Support**: Disambiguate identical strings with msgctxt
- **Metadata Management**: Manage PO file headers and language settings
- **Concurrent Access**: Thread-safe operations with Arc<RwLock>
- **File Discovery**: Auto-discover .po/.pot files or use explicit paths
- **Format Preservation**: Parse and serialize PO format with structure preservation

## Installation

### Prerequisites

- Rust 1.70+ (with Cargo)
- For web UI: environment variables `WEB_PORT` and optionally `WEB_HOST`

### Install

```bash
cargo install --path .
# This will install `gettext-mcp` into `~/.cargo/bin/`
```

### Configuration

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "gettext": {
      "command": "gettext-mcp",
      "args": ["/path/to/your/priv/gettext"]
    }
  }
}
```

## Usage

### Single File Mode

Start the server with a default PO file:

```bash
./gettext-mcp /path/to/translations.po
```

Tools can omit the `path` parameter to use the default file.

### Directory Mode

Point the server at a gettext directory to auto-discover all `.po`/`.pot` files recursively:

```bash
./gettext-mcp /path/to/priv/gettext
```

This scans the directory tree (e.g., `{lang}/LC_MESSAGES/*.po`, `*.pot`) and pre-loads all files at startup. Tools accept relative paths within the directory:

```json
{ "path": "sv/LC_MESSAGES/default.po" }
```

Use the `list_files` tool to see all discovered files.

### Dynamic Path Mode

Without specifying a path, the server will accept dynamic paths from MCP tool calls:

```bash
./gettext-mcp
```

All tools will require explicit `path` parameters.

### With Web UI

Enable the optional web interface:

```bash
WEB_PORT=8787 WEB_HOST=127.0.0.1 ./gettext-mcp /path/to/priv/gettext
```

Then access the UI at `http://127.0.0.1:8787`

## MCP Tools

### Translation CRUD

#### list_translations
List translations with optional filtering and pagination.

**Parameters:**
- `path`: (optional) Path to .po file (required if not using default)
- `query`: (optional) Search query (searches msgid and msgstr)
- `limit`: (optional) Maximum number of results

**Returns:** Array of translation records

#### get_translation
Fetch a single translation entry.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID to fetch
- `msgctxt`: (optional) Message context for disambiguation

**Returns:** Translation details (msgid, msgstr, msgctxt, plural forms, flags, comments, source locations)

#### upsert_translation
Create or update a translation entry.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID
- `msgstr`: (optional) The translated string
- `msgctxt`: (optional) Message context
- `msgid_plural`: (optional) Plural form of the message ID
- `msgstr_plural`: (optional) Array of plural translations [index 0 = singular, 1 = plural, ...]
- `flags`: (optional) Array of flags (fuzzy, c-format, etc)

**Returns:** Success response

#### delete_translation
Remove a specific translation.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID
- `msgctxt`: (optional) Message context

**Returns:** Success response

#### delete_key
Remove all contexts of a message.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID

**Returns:** Success response

### Metadata Tools

#### set_comment
Set or clear translator comments on a translation.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID
- `msgctxt`: (optional) Message context for disambiguation
- `comment`: (optional) Comment text (omit to clear)

#### set_fuzzy
Toggle or set the fuzzy flag.

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID
- `msgctxt`: (optional) Message context for disambiguation
- `fuzzy`: Boolean flag state

#### set_flag
Manage specific flags (c-format, python-format, etc).

**Parameters:**
- `path`: (optional) Path to .po file
- `msgid`: The message ID
- `msgctxt`: (optional) Message context for disambiguation
- `flag`: Flag name
- `enabled`: Boolean to enable/disable flag

### File/Language Tools

#### list_files
List all discovered .po/.pot files (useful in directory mode).

**Parameters:** None

**Returns:** Array of file objects with `path` and `relative_path`, plus `count` and `base_dir`

#### list_metadata
Get file metadata (encoding, language, plural forms).

**Parameters:**
- `path`: (optional) Path to .po file

**Returns:** Metadata object with encoding, language, plural_forms

#### set_header
Update a metadata header value.

**Parameters:**
- `path`: (optional) Path to .po file
- `key`: Header key (e.g., "Language", "Content-Type")
- `value`: (optional) Header value. Omit to remove the header key.

#### list_contexts
Get all msgctxt values in the file.

**Parameters:**
- `path`: (optional) Path to .po file

**Returns:** Array of context strings

#### list_languages (Web UI only)
List languages in the file.

#### add_language (Web UI only)
Add a language to the file.

#### remove_language (Web UI only)
Remove a language from the file.

## Web UI Features

### Translation Browser
- Real-time search and filtering
- Display translation status (translated, fuzzy, untranslated)
- Support for plural forms
- Comment and metadata display

### Translation Editor
- Edit msgid and msgstr inline
- Manage fuzzy flag status
- Add/update comments
- Handle plural forms

### File Management
- Select and switch between multiple .po files
- View per-file statistics
- Progress indicator showing translation coverage

### Language Management
- View available languages
- Add new languages
- Remove languages

## PO File Format

This server supports the standard GNU gettext .po format:

```
# Translator comment
#. Extracted comment
#: source/file.py:42
#, fuzzy, c-format
msgctxt "context"
msgid "Original text"
msgid_plural "Original texts"
msgstr "Translated text"
msgstr[0] "Singular translation"
msgstr[1] "Plural translation"
```

## Examples

### Example .po Files

Sample files are included in `examples/`:

- `sample_en.pot` - English template file
- `sample_fr.po` - French translation example
- `sample_plurals.po` - Example with plural forms and contexts

### API Usage Examples

#### Using with Claude

When using the MCP server with Claude, you can ask:

```
"Translate 'Hello' to French in the translations.po file"
"Show me all untranslated strings"
"Mark the entry 'Save' as fuzzy"
"Add German language to the project"
```

#### Direct MCP Tool Calls

```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "list_translations",
    "arguments": {
      "path": "/path/to/file.po",
      "query": "hello",
      "limit": 10
    }
  }
}
```

## Architecture

The server follows a clean three-layer architecture:

1. **Store Layer** (`src/store.rs`)
   - Handles PO file parsing and serialization
   - In-memory representation with Arc<RwLock> for thread safety
   - CRUD operations and metadata management

2. **MCP Server Layer** (`src/mcp_server.rs`)
   - Tool definitions and parameter handling
   - Error mapping to MCP error types
   - JSON response formatting

3. **Web UI Layer** (`src/web/mod.rs`)
   - Axum HTTP server
   - Embedded SPA with React-like frontend
   - REST API endpoints mirroring MCP tools

## Testing

Run the test suite:

```bash
cargo test
```

Tests cover:
- PO format parsing and serialization
- Store CRUD operations
- Concurrent access patterns
- Plural form handling
- Context disambiguation
- Metadata operations
- Web UI functionality

## Performance Considerations

- **Memory**: Entire PO file loaded in memory (suitable for catalogs <1MB)
- **Disk I/O**: Changes written to disk after each modification
- **Search**: Linear scan of all translations (fine for <10k entries)
- **Concurrency**: Multiple readers OR one writer (designed for interactive use)

## Configuration

### Environment Variables

- `WEB_PORT`: Enable web UI on specified port (e.g., `8787`)
- `WEB_HOST`: Bind web server to address (default: `127.0.0.1`)
- `RUST_LOG`: Set logging level (e.g., `info`, `debug`)

### Command Line

- First argument: Path to a `.po` file or a gettext directory (optional). When a directory is given, all `.po`/`.pot` files are discovered recursively at startup.

## Troubleshooting

### Web UI won't start
Ensure `WEB_PORT` environment variable is set:
```bash
    WEB_PORT=8787 ./gettext-mcp
```

### File not found
Use absolute paths when specifying .po files:
```bash
./gettext-mcp /absolute/path/to/file.po
```

### Connection issues with Claude
Ensure the MCP server is properly registered in Claude's configuration and has read/write permissions to the PO files.

## Contributing

Contributions welcome! Areas for enhancement:
- Performance optimization for large catalogs
- Additional format support (.json, .yaml, etc)
- Fuzzy matching and suggestion features
- Git integration for version control
- CI/CD pipeline improvements

## License

MIT License

## Related Projects

- [xcstrings-mcp](https://github.com/anthropics/xcstrings-mcp) - Similar server for Xcode String Catalogs
- [GNU gettext](https://www.gnu.org/software/gettext/) - Official gettext documentation
- [MCP Specification](https://modelcontextprotocol.io/) - Model Context Protocol details

## Support

For issues or questions:
1. Check the troubleshooting section above
2. Review the test files for usage examples
3. Consult the MCP specification for protocol details
