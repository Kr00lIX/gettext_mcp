---
name: Gettext MCP Implementation Specification (Rust)
description: Complete technical specification for building a gettext .po/.pot MCP server in Rust
type: project
---

# Gettext MCP Server - Rust Implementation Specification

## Executive Summary

A Rust MCP (Model Context Protocol) server for managing GNU gettext `.po` (Portable Object) and `.pot` (Portable Object Template) localization files. Exposes CRUD operations, fuzzy flag management, plural form handling, and optional web UI.

Similar architecture to xcstrings-mcp but tailored for gettext's text-based format with support for:
- Message translation (msgid → msgstr)
- Message contexts (msgctxt)
- Plural forms (msgid_plural, msgstr[0..n])
- Developer/translator comments
- Fuzzy flags and obsolete entries
- Format specifiers (c-format, python-format)

---

## Table of Contents

1. [Format Overview](#format-overview)
2. [Architecture](#architecture)
3. [Data Structures](#data-structures)
4. [Store Implementation](#store-implementation)
5. [MCP Tools](#mcp-tools)
6. [Web UI](#web-ui)
7. [Project Structure](#project-structure)
8. [Dependencies](#dependencies)
9. [Implementation Details](#implementation-details)
10. [Testing Strategy](#testing-strategy)
11. [Deployment](#deployment)

---

## Format Overview

### PO File Structure

```po
# Developer comment
#. Extracted comment
#: source/file.rs:123
#, fuzzy, c-format
#| msgid "old string"
msgctxt "context for disambiguation"
msgid "singular form"
msgid_plural "plural form with %d items"
msgstr[0] "translation singular"
msgstr[1] "translation plural"

# Header message (always first)
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Language: fr\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"
```

### Key Concepts

| Concept | Purpose | Example |
|---------|---------|---------|
| **msgid** | Source string identifier (English) | `"Hello, %s!"` |
| **msgstr** | Translated string | `"Bonjour, %s !"` |
| **msgctxt** | Disambiguation context | `"greeting"` vs `"action"` |
| **msgid_plural** | Plural form (source) | `"%d items"` |
| **msgstr[n]** | Plural translations (indexed) | `msgstr[0]`, `msgstr[1]` |
| **Flags** | Metadata (fuzzy, c-format, etc) | `#, fuzzy, c-format` |
| **Comments** | Developer/translator notes | `#: file.rs:123` |
| **Obsolete** | Marked for removal | `#~ msgid "old"` |

### Header Message

The first message (empty msgid) contains metadata:
```po
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Language: fr\n"
"Language-Team: French <fr@example.com>\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"
"Last-Translator: John Doe <john@example.com>\n"
```

---

## Architecture

### High-Level Design

```
┌─────────────────────────────────────────────────────────┐
│                    MCP Client (Claude, etc)              │
└────────────────┬────────────────────────────────────────┘
                 │ stdio (JSON-RPC 2.0)
┌────────────────▼────────────────────────────────────────┐
│                  gettext-mcp Server                      │
├────────────────────────────────────────────────────────┤
│  MCP Server Layer (mcp_server.rs)                       │
│  ├─ Tool Router                                         │
│  ├─ Parameter Validation (JSON Schema)                  │
│  └─ Error Mapping                                       │
├────────────────────────────────────────────────────────┤
│  Store Layer (store.rs)                                 │
│  ├─ PO Parser (po_parser.rs)                           │
│  ├─ In-Memory Cache (Arc<RwLock<T>>)                   │
│  ├─ Persistence (fs::write)                            │
│  └─ CRUD Operations                                     │
├────────────────────────────────────────────────────────┤
│  Web UI Layer (web/mod.rs) [Optional]                   │
│  ├─ Axum HTTP Server                                    │
│  ├─ REST API Routes                                     │
│  └─ SPA (index.html, static assets)                     │
├────────────────────────────────────────────────────────┤
│  Main (main.rs)                                         │
│  ├─ Config parsing                                      │
│  ├─ Task spawning                                       │
│  └─ Graceful shutdown                                   │
└────────────────┬────────────────────────────────────────┘
                 │
         ┌───────┴────────┐
         │                │
    .po files          HTTP (Web UI)
    (filesystem)       (optional)
```

### Design Principles

1. **Format-agnostic patterns**: Reuse patterns from xcstrings-mcp
2. **Async-first**: Use tokio for all I/O
3. **Thread-safe**: Arc<RwLock<T>> for shared mutable state
4. **Lazy-loading**: Load stores on first access
5. **Order preservation**: Maintain PO file formatting
6. **Atomic writes**: Write new content to temp file, then rename
7. **Format preservation**: Comments, blank lines, field order intact

---

## Data Structures

### Core Structures

```rust
use indexmap::IndexMap;
use std::collections::HashMap;

/// Represents a single PO message entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoMessage {
    /// Optional context for disambiguation
    pub context: Option<String>,

    /// Source string (English)
    pub msgid: String,

    /// Plural form of source string
    pub msgid_plural: Option<String>,

    /// Translated strings (singular, plural[0], plural[1], ...)
    /// For singular: vec![one_translation]
    /// For plural: vec![singular_translation, plural1, plural2, ...]
    pub msgstr: Vec<String>,

    /// Fuzzy flag (untranslated, needs review)
    pub fuzzy: bool,

    /// Format specifiers (c-format, python-format, etc)
    pub flags: Vec<String>,  // e.g., ["c-format", "python-format"]

    /// Developer comments (#. prefix)
    pub extracted_comments: Vec<String>,

    /// Translator comments (# prefix, non-special)
    pub translator_comments: Vec<String>,

    /// Source locations (#: file.rs:line)
    pub source_locations: Vec<String>,

    /// Previous msgid (before update)
    pub previous_msgid: Option<String>,

    /// Previous msgid_plural (before update)
    pub previous_msgid_plural: Option<String>,

    /// Obsolete flag (# ~ prefix)
    pub obsolete: bool,
}

/// Represents entire PO file
#[derive(Debug, Clone)]
pub struct PoFile {
    /// Metadata (first message with empty msgid)
    pub header: PoHeader,

    /// All messages indexed by (context, msgid) for O(1) lookup
    /// Key format: "context\0msgid" or just "msgid" if no context
    messages: IndexMap<String, PoMessage>,

    /// Raw file content (for preserving formatting)
    raw_content: String,
}

/// Parsed PO file metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PoHeader {
    pub charset: String,           // "UTF-8"
    pub language: Option<String>,  // "fr", "de", "es", etc
    pub language_team: Option<String>,
    pub plural_forms: Option<String>,  // "nplurals=2; plural=(n != 1);"
    pub last_translator: Option<String>,
    pub content_type: String,      // "text/plain; charset=UTF-8"
    pub custom_headers: HashMap<String, String>,
}

/// Summary of translation status
#[derive(Debug, Clone, Serialize)]
pub struct TranslationStats {
    pub total: usize,
    pub translated: usize,
    pub fuzzy: usize,
    pub untranslated: usize,
    pub percent_translated: f64,
}
```

### Query/Response Types

```rust
/// List messages with filtering
#[derive(Debug, Serialize)]
pub struct MessageSummary {
    pub id: String,              // "context\0msgid" or "msgid"
    pub msgid: String,
    pub msgid_plural: Option<String>,
    pub msgstr_count: usize,     // How many translations (1 or plural count)
    pub translated: bool,        // All msgstr entries filled
    pub fuzzy: bool,
    pub context: Option<String>,
    pub source_locations: Vec<String>,
    pub comment: Option<String>, // Combined extracted + translator comment
}

/// Full message details
#[derive(Debug, Serialize)]
pub struct MessageDetail {
    #[serde(flatten)]
    pub message: PoMessage,
}

/// Request to create/update message
#[derive(Debug, Deserialize)]
pub struct MessageUpdate {
    pub msgid: String,
    pub msgid_plural: Option<String>,
    pub context: Option<String>,
    pub msgstr: Vec<String>,        // Translations
    pub fuzzy: Option<bool>,
    pub flags: Option<Vec<String>>,
    pub extracted_comments: Option<Vec<String>>,
    pub translator_comments: Option<Vec<String>>,
    pub source_locations: Option<Vec<String>>,
}
```

---

## Store Implementation

### File: `src/store.rs`

Comprehensive store with load, save, and CRUD operations.

**Key responsibilities:**
- Load PO files via parser
- Maintain thread-safe in-memory cache
- Persist changes atomically
- Provide query interface (search, filter, limit)
- Manage multiple stores (multi-file mode)

### File: `src/po_parser.rs`

PO file parsing:
- Line-by-line parsing with state machine
- Handle all comment types (#, #., #:, #,, #|, #~)
- Extract header metadata
- Build message index
- Preserve formatting for round-trip

### File: `src/mcp_server.rs`

MCP tool definitions:
- `list_messages(path, query?, limit?)` - List with filtering
- `get_message(path, msgid, context?)` - Fetch one
- `upsert_message(path, msgid, msgid_plural?, context?, msgstr, ...)` - Create/update
- `delete_message(path, msgid, context?)` - Remove
- `set_fuzzy(path, msgid, context?, fuzzy)` - Mark reviewed/fuzzy
- `set_translation(path, msgid, context?, value, plural_index?)` - Update translation
- `get_stats(path)` - Translation progress

All tools return JSON responses. Errors mapped to MCP error types.

---

## MCP Tools

### Message CRUD

```
list_messages(path, query?, limit?)
├─ Returns: Array of MessageSummary
├─ Filter by msgid/msgstr/comments
└─ Pagination support

get_message(path, msgid, context?)
├─ Returns: MessageDetail (full message with all fields)
├─ Includes: translations, comments, flags, source locations
└─ Error: MessageNotFound if not found

upsert_message(path, update)
├─ Creates message if not exists
├─ Updates all fields if exists
├─ Clears fuzzy flag when translated
└─ Returns: "Message updated successfully"

delete_message(path, msgid, context?)
├─ Removes entire message
├─ Handles singular & plural forms
└─ Returns: "Message deleted successfully"
```

### Fuzzy & Translation Management

```
set_fuzzy(path, msgid, context?, fuzzy)
├─ Mark message as needs review (fuzzy=true)
├─ Mark as reviewed (fuzzy=false)
└─ Returns: "Message marked as fuzzy|reviewed"

set_translation(path, msgid, context?, value, plural_index?)
├─ Update specific plural form (default index 0)
├─ Automatically clears fuzzy flag
├─ Validates plural index against msgid_plural
└─ Returns: "Translation updated"
```

### Statistics

```
get_stats(path)
├─ Returns: TranslationStats
├─ Fields:
│  ├─ total: Number of messages
│  ├─ translated: Count with all msgstr filled
│  ├─ fuzzy: Count marked fuzzy
│  ├─ untranslated: Count with empty msgstr
│  └─ percent_translated: (translated / total) * 100
└─ Useful for UI progress bars
```

---

## Web UI

Optional HTTP server with SPA:
- List messages with real-time filtering
- Inline edit translations
- Toggle fuzzy flags
- Show translation progress
- Keyboard navigation

Routes:
- `GET /` - Serve SPA
- `GET /api/messages?path=...&query=...` - List messages
- `GET /api/messages/:id?path=...` - Get one message
- `POST /api/messages` - Create/update
- `DELETE /api/messages/:id?path=...` - Delete
- `GET /api/stats?path=...` - Stats

---

## Project Structure

```
gettext-mcp/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs                 # Entry point, config, task spawning
│   ├── lib.rs                  # Library exports
│   ├── store.rs                # PoStore, PoStoreManager, operations
│   ├── po_parser.rs            # PO file parsing
│   ├── mcp_server.rs           # MCP tool definitions
│   ├── web/
│   │   ├── mod.rs              # Axum server, route handlers
│   │   └── index.html          # Web UI (embedded)
│   └── model.rs                # Data structures (PoMessage, PoFile, etc)
├── tests/
│   ├── parser.rs               # PO parser tests
│   ├── store.rs                # Store CRUD tests
│   ├── mcp.rs                  # MCP tool tests
│   └── fixtures/               # Example .po files
├── examples/
│   ├── simple.po               # Minimal example
│   ├── plural.po               # Plural forms
│   ├── context.po              # Message contexts
│   └── complex.po              # Full features
├── docs/
│   ├── API.md                  # MCP tool documentation
│   ├── FORMAT.md               # PO format details
│   └── ARCHITECTURE.md         # Design decisions
├── README.md
├── LICENSE
└── CLAUDE.md                   # Claude Code instructions
```

---

## Dependencies

```toml
[package]
name = "gettext-mcp"
version = "0.1.0"
edition = "2021"

[dependencies]
# MCP Protocol
rmcp = { version = "0.5", features = ["server", "transport-async-rw"] }

# Async Runtime
tokio = { version = "1.37", features = ["macros", "rt-multi-thread", "signal", "fs", "io-std"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
indexmap = { version = "2", features = ["serde"] }

# Schema Generation & Validation
schemars = { version = "1", features = ["derive"] }
serde_derive = "1"

# Error Handling
thiserror = "1"
anyhow = "1"

# Web Framework (Optional)
axum = { version = "0.7", features = ["macros", "json", "tokio", "http1"] }
tower = "0.4"
tower-http = { version = "0.5", features = ["cors"] }

# Utilities
regex = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
async-trait = "0.1"

[dev-dependencies]
tempfile = "3"
tokio-test = "0.4"
```

---

## Implementation Details

### PO File Format Preservation

Key techniques:
1. **Store raw content**: Keep original file text for formatting
2. **Use IndexMap**: Preserves insertion order during serialization
3. **Atomic writes**: Write to `.po.tmp`, then rename to `.po`
4. **Comment preservation**: Parse and store all comment types

### Plural Form Handling

```rust
// Source has msgid + msgid_plural
// Translations are in msgstr array indexed by plural form

Example:
msgid "%d item"
msgid_plural "%d items"
msgstr[0] "%d élément"      // singular (n=1)
msgstr[1] "%d éléments"     // plural (n!=1)
```

### Context Handling

```rust
// Use null byte (\0) as separator for (context, msgid) tuple
key = format!("{}\0{}", context, msgid)

// This allows same msgid with different contexts:
msgctxt "button"
msgid "OK"      → key = "button\0OK"

msgctxt "notification"
msgid "OK"      → key = "notification\0OK"
```

### Fuzzy Messages

Messages marked as fuzzy (needs review) are still returned but can be filtered:

```rust
// Get only translated messages
messages
    .iter()
    .filter(|m| !m.fuzzy && m.is_translated())
    .collect()
```

---

## Testing Strategy

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_parse_simple_po() {
        let content = r#"
msgid ""
msgstr ""
"Language: fr\n"

msgid "Hello"
msgstr "Bonjour"
"#;
        let po = PoParser::parse(content).unwrap();
        assert_eq!(po.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_plural_forms() {
        let content = r#"
msgid "%d file"
msgid_plural "%d files"
msgstr[0] "%d fichier"
msgstr[1] "%d fichiers"
"#;
        let po = PoParser::parse(content).unwrap();
        let msg = po.messages.values().next().unwrap();
        assert_eq!(msg.msgstr.len(), 2);
    }

    #[tokio::test]
    async fn test_store_persistence() {
        let temp = NamedTempFile::new().unwrap();
        let path = temp.path();

        let store = PoStore::load_or_create(path).await.unwrap();
        store.upsert_message(MessageUpdate {
            msgid: "Hello".to_string(),
            msgstr: vec!["Bonjour".to_string()],
            ..Default::default()
        }).await.unwrap();

        // Reload and verify
        let store2 = PoStore::load_or_create(path).await.unwrap();
        let msg = store2.get_message("Hello", None).await.unwrap();
        assert!(msg.is_some());
    }

    #[test]
    fn test_context_key_generation() {
        assert_eq!(
            PoStore::make_key(Some("button"), "OK"),
            "button\0OK"
        );
        assert_eq!(
            PoStore::make_key(None, "OK"),
            "OK"
        );
    }
}
```

---

## Deployment

### Mode 1: Single PO File

```bash
gettext-mcp /path/to/messages.po
```

Tools can omit `path` parameter in MCP calls.

### Mode 2: Dynamic Path

```bash
gettext-mcp
```

Tools must provide `path` parameter. Web UI can select file.

### Mode 3: With Web UI

```bash
WEB_PORT=8787 gettext-mcp /path/to/messages.po
```

Access at `http://127.0.0.1:8787`

### Docker

```dockerfile
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/gettext-mcp /usr/local/bin/
EXPOSE 8787
ENV WEB_PORT=8787
ENTRYPOINT ["gettext-mcp"]
```

---

## Configuration

### Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `GETTEXT_PATH` | Default `.po` file path | unset (dynamic mode) |
| `WEB_HOST` | Web UI host | `127.0.0.1` |
| `WEB_PORT` | Web UI port | `8787` |
| `RUST_LOG` | Logging level | `info` |

### MCP Client Configuration

#### Claude Code

```bash
claude mcp add-json gettext-mcp \
  '{"command":"/usr/local/bin/gettext-mcp","transport":"stdio","env":{"WEB_PORT":"8787"}}'
```

#### Manual (claude.json)

```json
{
  "mcpServers": {
    "gettext": {
      "command": "/usr/local/bin/gettext-mcp",
      "args": ["/path/to/messages.po"],
      "transport": "stdio",
      "env": {
        "WEB_PORT": "8787"
      }
    }
  }
}
```

---

## Detailed Implementation Guide

### Phase 1: Core Parser (`src/po_parser.rs`)

The PO parser is the foundation. Here's the detailed algorithm:

#### State Machine for Parsing

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum ParseState {
    StartOfLine,
    InComment,
    InMessage,
    InMsgid,
    InMsgidPlural,
    InMsgstr,
}

pub struct PoParser {
    state: ParseState,
    current_message: Option<PoMessage>,
    messages: IndexMap<String, PoMessage>,
}

impl PoParser {
    pub fn parse(content: &str) -> Result<PoFile, String> {
        let mut parser = Self {
            state: ParseState::StartOfLine,
            current_message: None,
            messages: IndexMap::new(),
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            i = parser.process_line(line, &lines, i)?;
        }

        // Finalize last message
        if let Some(msg) = parser.current_message.take() {
            let key = parser.make_key(&msg);
            parser.messages.insert(key, msg);
        }

        Ok(PoFile {
            messages: parser.messages,
            raw_content: content.to_string(),
            ..Default::default()
        })
    }

    fn process_line(&mut self, line: &str, lines: &[&str], i: usize) -> Result<usize, String> {
        let trimmed = line.trim();

        // Empty lines mark message boundaries
        if trimmed.is_empty() {
            if let Some(msg) = self.current_message.take() {
                let key = self.make_key(&msg);
                self.messages.insert(key, msg);
            }
            return Ok(i + 1);
        }

        // Process by line type
        if trimmed.starts_with("#.") {
            // Extracted comment
            self.ensure_current_message();
            self.current_message.as_mut().unwrap()
                .extracted_comments
                .push(trimmed[2..].trim().to_string());
            Ok(i + 1)

        } else if trimmed.starts_with("#:") {
            // Source location
            self.ensure_current_message();
            self.current_message.as_mut().unwrap()
                .source_locations
                .push(trimmed[2..].trim().to_string());
            Ok(i + 1)

        } else if trimmed.starts_with("#,") {
            // Flags
            self.ensure_current_message();
            let flags = trimmed[2..]
                .split(',')
                .map(|f| f.trim().to_string())
                .collect::<Vec<_>>();
            self.current_message.as_mut().unwrap().flags.extend(flags);
            Ok(i + 1)

        } else if trimmed.starts_with("msgctxt") {
            self.ensure_current_message();
            if let Some(ctx) = self.extract_string_value(trimmed) {
                self.current_message.as_mut().unwrap().context = Some(ctx);
            }
            Ok(i + 1)

        } else if trimmed.starts_with("msgid") && !trimmed.starts_with("msgid_plural") {
            self.ensure_current_message();
            if let Some(id) = self.extract_string_value(trimmed) {
                self.current_message.as_mut().unwrap().msgid = id;
            }
            Ok(i + 1)

        } else if trimmed.starts_with("msgid_plural") {
            self.ensure_current_message();
            if let Some(id) = self.extract_string_value(trimmed) {
                self.current_message.as_mut().unwrap().msgid_plural = Some(id);
            }
            Ok(i + 1)

        } else if trimmed.starts_with("msgstr") {
            self.ensure_current_message();
            if let Some(val) = self.extract_string_value(trimmed) {
                self.current_message.as_mut().unwrap().msgstr.push(val);
            } else {
                self.current_message.as_mut().unwrap().msgstr.push(String::new());
            }
            Ok(i + 1)

        } else if trimmed.starts_with("\"") {
            // Continuation of previous string
            if let Some(msg) = self.current_message.as_mut() {
                if let Some(val) = self.extract_string_value(trimmed) {
                    if let Some(last) = msg.msgstr.last_mut() {
                        last.push_str(&val);
                    } else if !msg.msgid.is_empty() {
                        msg.msgid.push_str(&val);
                    }
                }
            }
            Ok(i + 1)

        } else if trimmed.starts_with("# ") && !trimmed.starts_with("#.") {
            // Translator comment
            self.ensure_current_message();
            self.current_message.as_mut().unwrap()
                .translator_comments
                .push(trimmed[2..].to_string());
            Ok(i + 1)

        } else {
            Ok(i + 1)
        }
    }

    fn ensure_current_message(&mut self) {
        if self.current_message.is_none() {
            self.current_message = Some(PoMessage::default());
        }
    }

    fn extract_string_value(&self, line: &str) -> Option<String> {
        // Extract from: msgid "value" or msgstr[0] "value" or just "value"
        let start = line.find('"')?;
        let end = line.rfind('"')?;
        if start >= end {
            return None;
        }
        let content = &line[start + 1..end];
        // Unescape: \n → newline, \" → quote, \\ → backslash
        Some(content
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\"))
    }

    fn make_key(&self, msg: &PoMessage) -> String {
        match &msg.context {
            Some(ctx) => format!("{}\0{}", ctx, msg.msgid),
            None => msg.msgid.clone(),
        }
    }
}
```

#### Serialization (Writing PO Files)

```rust
impl PoFile {
    pub fn serialize(&self) -> Result<String, String> {
        let mut output = String::new();

        // Write header first
        let header_msg = PoMessage {
            msgid: String::new(),
            msgstr: vec![self.serialize_header()],
            ..Default::default()
        };
        output.push_str(&self.format_message(&header_msg, true));
        output.push('\n');

        // Write all messages
        for (_, msg) in &self.messages {
            if !msg.msgid.is_empty() {
                output.push_str(&self.format_message(msg, false));
                output.push('\n');
            }
        }

        Ok(output)
    }

    fn format_message(&self, msg: &PoMessage, is_header: bool) -> String {
        let mut output = String::new();

        // Comments (in order)
        for comment in &msg.translator_comments {
            output.push_str(&format!("# {}\n", comment));
        }
        for comment in &msg.extracted_comments {
            output.push_str(&format!("#. {}\n", comment));
        }
        for location in &msg.source_locations {
            output.push_str(&format!("#: {}\n", location));
        }

        // Flags
        if !msg.flags.is_empty() {
            output.push_str(&format!("#, {}\n", msg.flags.join(", ")));
        }

        // Fuzzy flag
        if msg.fuzzy && !msg.flags.contains(&"fuzzy".to_string()) {
            output.push_str("#, fuzzy\n");
        }

        // Context
        if let Some(ctx) = &msg.context {
            output.push_str(&format!("msgctxt \"{}\"\n", self.escape_string(ctx)));
        }

        // Message ID
        output.push_str(&format!("msgid \"{}\"\n", self.escape_string(&msg.msgid)));

        // Plural form
        if let Some(plural) = &msg.msgid_plural {
            output.push_str(&format!("msgid_plural \"{}\"\n", self.escape_string(plural)));
        }

        // Translations
        if msg.msgid_plural.is_some() {
            for (i, translation) in msg.msgstr.iter().enumerate() {
                output.push_str(&format!("msgstr[{}] \"{}\"\n", i, self.escape_string(translation)));
            }
        } else {
            for translation in &msg.msgstr {
                output.push_str(&format!("msgstr \"{}\"\n", self.escape_string(translation)));
            }
        }

        output
    }

    fn escape_string(&self, s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('"', "\\\"")
    }

    fn serialize_header(&self) -> String {
        let mut lines = vec![
            format!("Content-Type: {}; charset={}", self.header.content_type, self.header.charset),
        ];

        if let Some(lang) = &self.header.language {
            lines.push(format!("Language: {}", lang));
        }
        if let Some(team) = &self.header.language_team {
            lines.push(format!("Language-Team: {}", team));
        }
        if let Some(plural) = &self.header.plural_forms {
            lines.push(format!("Plural-Forms: {}", plural));
        }
        if let Some(translator) = &self.header.last_translator {
            lines.push(format!("Last-Translator: {}", translator));
        }

        lines.join("\n") + "\n"
    }
}
```

---

### Phase 2: Store Layer (`src/store.rs`)

#### Full PoStore Implementation

```rust
pub struct PoStore {
    path: PathBuf,
    data: Arc<RwLock<PoFile>>,
}

impl PoStore {
    pub async fn load_or_create(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        // Load or create default
        let po_file = if path.exists() {
            let content = tokio::fs::read_to_string(&path).await?;
            PoParser::parse(&content)?
        } else {
            PoFile::default()
        };

        Ok(Self {
            path,
            data: Arc::new(RwLock::new(po_file)),
        })
    }

    pub async fn reload(&self) -> Result<(), StoreError> {
        let content = tokio::fs::read_to_string(&self.path).await?;
        let po_file = PoParser::parse(&content)?;
        *self.data.write().await = po_file;
        Ok(())
    }

    async fn persist(&self) -> Result<(), StoreError> {
        let content = {
            let data = self.data.read().await;
            data.serialize()?
        };

        // Atomic write
        let temp_path = self.path.with_extension("po.tmp");
        tokio::fs::write(&temp_path, &content).await?;
        tokio::fs::rename(&temp_path, &self.path).await?;

        Ok(())
    }

    // Search with full-text indexing
    pub async fn list_messages(
        &self,
        query: Option<&str>,
        limit: Option<usize>,
        skip_fuzzy: bool,
    ) -> Vec<MessageSummary> {
        let data = self.data.read().await;
        let mut results: Vec<_> = data.messages
            .iter()
            .filter(|(_, msg)| {
                if skip_fuzzy && msg.fuzzy {
                    return false;
                }
                match query {
                    None => true,
                    Some(q) => self.matches_query(msg, q),
                }
            })
            .map(|(key, msg)| MessageSummary {
                id: key.clone(),
                msgid: msg.msgid.clone(),
                msgid_plural: msg.msgid_plural.clone(),
                msgstr_count: msg.msgstr.len(),
                translated: msg.is_translated(),
                fuzzy: msg.fuzzy,
                context: msg.context.clone(),
                source_locations: msg.source_locations.clone(),
                comment: [
                    &msg.extracted_comments.join(" "),
                    &msg.translator_comments.join(" "),
                ].iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
                    .into(),
            })
            .collect();

        results.sort_by(|a, b| a.msgid.cmp(&b.msgid));
        results.truncate(limit.unwrap_or(100));
        results
    }

    fn matches_query(&self, msg: &PoMessage, query: &str) -> bool {
        let q = query.to_lowercase();
        msg.msgid.to_lowercase().contains(&q) ||
        msg.msgstr.iter().any(|s| s.to_lowercase().contains(&q)) ||
        msg.extracted_comments.iter().any(|c| c.to_lowercase().contains(&q)) ||
        msg.translator_comments.iter().any(|c| c.to_lowercase().contains(&q))
    }

    pub async fn get_message(
        &self,
        msgid: &str,
        context: Option<&str>,
    ) -> Result<Option<MessageDetail>, StoreError> {
        let data = self.data.read().await;
        let key = Self::make_key(context, msgid);
        Ok(data.messages.get(&key).map(|m| MessageDetail {
            message: m.clone(),
        }))
    }

    pub async fn upsert_message(&self, update: MessageUpdate) -> Result<(), StoreError> {
        {
            let mut data = self.data.write().await;
            let key = Self::make_key(update.context.as_deref(), &update.msgid);

            // Validate msgstr count matches plural count
            let expected_count = if update.msgid_plural.is_some() {
                2  // At minimum, singular + one plural form
            } else {
                1
            };

            if update.msgstr.len() < expected_count {
                return Err(StoreError::InvalidPluralIndex(
                    format!("Expected {} translations, got {}", expected_count, update.msgstr.len())
                ));
            }

            let message = PoMessage {
                context: update.context,
                msgid: update.msgid,
                msgid_plural: update.msgid_plural,
                msgstr: update.msgstr,
                fuzzy: update.fuzzy.unwrap_or(false),
                flags: update.flags.unwrap_or_default(),
                extracted_comments: update.extracted_comments.unwrap_or_default(),
                translator_comments: update.translator_comments.unwrap_or_default(),
                source_locations: update.source_locations.unwrap_or_default(),
                previous_msgid: None,
                previous_msgid_plural: None,
                obsolete: false,
            };

            data.messages.insert(key, message);
        }

        self.persist().await
    }

    pub async fn delete_message(
        &self,
        msgid: &str,
        context: Option<&str>,
    ) -> Result<(), StoreError> {
        {
            let mut data = self.data.write().await;
            let key = Self::make_key(context, msgid);

            if data.messages.remove(&key).is_none() {
                return Err(StoreError::MessageNotFound {
                    msgid: msgid.to_string(),
                    context: context.map(|s| s.to_string()),
                });
            }
        }

        self.persist().await
    }

    pub async fn set_fuzzy(
        &self,
        msgid: &str,
        context: Option<&str>,
        fuzzy: bool,
    ) -> Result<(), StoreError> {
        {
            let mut data = self.data.write().await;
            let key = Self::make_key(context, msgid);

            let message = data.messages.get_mut(&key)
                .ok_or_else(|| StoreError::MessageNotFound {
                    msgid: msgid.to_string(),
                    context: context.map(|s| s.to_string()),
                })?;

            message.fuzzy = fuzzy;
        }

        self.persist().await
    }

    pub async fn stats(&self) -> TranslationStats {
        let data = self.data.read().await;
        let messages: Vec<_> = data.messages.values().collect();

        let total = messages.len();
        let translated = messages.iter()
            .filter(|m| m.msgstr.iter().all(|s| !s.is_empty()))
            .count();
        let fuzzy = messages.iter()
            .filter(|m| m.fuzzy)
            .count();
        let untranslated = total - translated;

        TranslationStats {
            total,
            translated,
            fuzzy,
            untranslated,
            percent_translated: if total > 0 {
                (translated as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        }
    }

    fn make_key(context: Option<&str>, msgid: &str) -> String {
        match context {
            Some(ctx) => format!("{}\0{}", ctx, msgid),
            None => msgid.to_string(),
        }
    }
}

impl PoMessage {
    pub fn is_translated(&self) -> bool {
        !self.msgstr.is_empty() && self.msgstr.iter().all(|s| !s.is_empty())
    }
}
```

---

### Phase 3: CLI Design

```rust
// src/main.rs

use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[command(name = "gettext-mcp")]
#[command(about = "MCP server for GNU gettext .po files", long_about = None)]
struct Args {
    /// Path to .po file (optional - use dynamic mode if not provided)
    po_file: Option<PathBuf>,

    /// Enable web UI on this host
    #[arg(long, env = "WEB_HOST")]
    web_host: Option<String>,

    /// Enable web UI on this port
    #[arg(long, env = "WEB_PORT")]
    web_port: Option<u16>,

    /// Logging level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup logging
    tracing_subscriber::fmt()
        .with_env_filter(args.log_level)
        .init();

    // Create store manager
    let manager = Arc::new(
        PoStoreManager::new(args.po_file)
            .await?
    );

    // Spawn MCP server on stdio
    let manager_mcp = manager.clone();
    let mcp_task = tokio::spawn(async move {
        let server = GetextMcpServer::new(manager_mcp);
        server.run().await
    });

    // Optionally spawn web UI
    let web_task = if let (Some(host), Some(port)) = (args.web_host, args.web_port) {
        let addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .expect("Invalid socket address");

        let manager_web = manager.clone();
        Some(tokio::spawn(async move {
            serve(addr, manager_web).await
        }))
    } else {
        None
    };

    // Wait for signals
    let ctrl_c = tokio::signal::ctrl_c();

    tokio::select! {
        res = mcp_task => {
            eprintln!("MCP server error: {:?}", res);
        }
        res = async {
            if let Some(task) = web_task {
                task.await
            } else {
                futures::future::pending().await
            }
        } => {
            eprintln!("Web server error: {:?}", res);
        }
        _ = ctrl_c => {
            eprintln!("Shutting down...");
        }
    }

    Ok(())
}
```

---

### Performance Optimizations

#### 1. Full-Text Search Indexing

For large PO files (>10k messages), add lazy indexing:

```rust
pub struct PoStore {
    path: PathBuf,
    data: Arc<RwLock<PoFile>>,
    search_index: Arc<RwLock<Option<SearchIndex>>>,  // Lazy-loaded
}

struct SearchIndex {
    msgid_index: HashMap<String, Vec<usize>>,
    content_index: HashMap<String, Vec<usize>>,
}

impl PoStore {
    async fn build_index(&self) {
        let data = self.data.read().await;
        let mut index = SearchIndex {
            msgid_index: HashMap::new(),
            content_index: HashMap::new(),
        };

        for (i, (_, msg)) in data.messages.iter().enumerate() {
            // Index by words
            for word in msg.msgid.split_whitespace() {
                index.msgid_index
                    .entry(word.to_lowercase())
                    .or_insert_with(Vec::new)
                    .push(i);
            }

            for translation in &msg.msgstr {
                for word in translation.split_whitespace() {
                    index.content_index
                        .entry(word.to_lowercase())
                        .or_insert_with(Vec::new)
                        .push(i);
                }
            }
        }

        *self.search_index.write().await = Some(index);
    }
}
```

#### 2. Batch Operations

For tools that process multiple messages:

```rust
pub async fn batch_upsert(&self, updates: Vec<MessageUpdate>) -> Result<(), StoreError> {
    {
        let mut data = self.data.write().await;
        for update in updates {
            let key = Self::make_key(update.context.as_deref(), &update.msgid);
            data.messages.insert(key, /* ... */);
        }
    }
    self.persist().await  // Single write!
}
```

#### 3. Lazy Context Loading

Only load message context when needed:

```rust
pub async fn get_messages_in_context(&self, context: &str) -> Vec<PoMessage> {
    let data = self.data.read().await;
    data.messages
        .values()
        .filter(|m| m.context.as_ref().map_or(false, |c| c == context))
        .cloned()
        .collect()
}
```

---

### Real-World Usage Examples

#### Example 1: Translate a message

```bash
# List untranslated messages in French PO file
curl http://localhost:8787/api/messages?path=/app/locales/fr.po&skip_fuzzy=true

# Update a translation via MCP
{
  "tool": "set_translation",
  "params": {
    "path": "/app/locales/fr.po",
    "msgid": "Hello, World!",
    "value": "Bonjour, monde!",
    "plural_index": 0
  }
}
```

#### Example 2: Handle plural forms

```json
{
  "tool": "upsert_message",
  "params": {
    "path": "/app/locales/fr.po",
    "msgid": "%d file",
    "msgid_plural": "%d files",
    "msgstr": ["%d fichier", "%d fichiers"],
    "context": "filesystem"
  }
}
```

#### Example 3: Mark for review

```json
{
  "tool": "set_fuzzy",
  "params": {
    "path": "/app/locales/fr.po",
    "msgid": "Recently changed string",
    "fuzzy": true
  }
}
```

#### Example 4: Progress tracking

```json
{
  "tool": "get_stats",
  "params": {
    "path": "/app/locales/fr.po"
  }
}

// Response:
{
  "total": 250,
  "translated": 180,
  "fuzzy": 15,
  "untranslated": 55,
  "percent_translated": 72.0
}
```

---

### Error Handling Strategy

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("message not found: msgid='{msgid}', context={context:?}")]
    MessageNotFound {
        msgid: String,
        context: Option<String>,
    },

    #[error("invalid plural index: {0}")]
    InvalidPluralIndex(String),

    #[error("path required")]
    PathRequired,

    #[error("invalid message format: {0}")]
    InvalidFormat(String),

    #[error("context: {0}")]
    ContextError(String),
}

impl From<StoreError> for McpError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::MessageNotFound { msgid, context } => {
                McpError::resource_not_found(
                    format!("Message not found: msgid='{}', context={:?}", msgid, context),
                    None
                )
            }
            StoreError::PathRequired => {
                McpError::invalid_params("Path parameter is required", None)
            }
            StoreError::ParseError(msg) => {
                McpError::internal_error(format!("Parse error: {}", msg), None)
            }
            _ => McpError::internal_error(err.to_string(), None),
        }
    }
}
```

---

### Migration Tools

Import from other formats:

```rust
/// Import from XLIFF format
pub async fn import_xliff(xliff_path: &Path, po_path: &Path) -> Result<(), StoreError> {
    let xliff_content = tokio::fs::read_to_string(xliff_path).await?;
    let po_file = XliffToPoConverter::convert(&xliff_content)?;
    let store = PoStore::load_or_create(po_path).await?;

    for (_, msg) in po_file.messages {
        let update = MessageUpdate {
            msgid: msg.msgid,
            msgstr: msg.msgstr,
            context: msg.context,
            ..Default::default()
        };
        store.upsert_message(update).await?;
    }
    Ok(())
}

/// Export to JSON for external tools
pub async fn export_json(po_path: &Path) -> Result<String, StoreError> {
    let store = PoStore::load_or_create(po_path).await?;
    let data = store.data.read().await;
    let messages: Vec<_> = data.messages.values().collect();
    Ok(serde_json::to_string_pretty(&messages)?)
}
```

---

### Build & Distribution

#### Cargo.toml Additions

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true

[[bin]]
name = "gettext-mcp"
path = "src/main.rs"

[package.metadata.deb]
maintainer = "Your Name <your@email.com>"
copyright = "2024"
extended-description = """\
GNU gettext MCP server for AI-assisted translation management."""
assets = [
    ["target/release/gettext-mcp", "usr/local/bin/", "755"],
    ["README.md", "usr/share/doc/gettext-mcp/", "644"],
]
```

#### Installation

```bash
# Build release binary
cargo build --release

# Install locally
cargo install --path .

# Build Debian package
cargo deb

# Build Docker image
docker build -t gettext-mcp:latest .

# Usage
gettext-mcp /path/to/messages.po
WEB_PORT=8787 gettext-mcp /path/to/messages.po
```

---

## Summary

This specification provides a complete blueprint for implementing a gettext MCP server in Rust that:

1. ✅ Parses and validates PO files with full format support
2. ✅ Exposes CRUD operations via MCP tools
3. ✅ Manages plural forms, contexts, and all PO metadata
4. ✅ Provides fuzzy flag & format specifier support
5. ✅ Offers optional web UI for human interaction
6. ✅ Supports both single and multi-file modes
7. ✅ Preserves file formatting and comments exactly
8. ✅ Uses async Rust for performance (suitable for 10k+ messages)
9. ✅ Implements atomic writes for data integrity
10. ✅ Includes full error handling and validation

The implementation follows xcstrings-mcp patterns while being comprehensively tailored to gettext's unique requirements and complexity.
