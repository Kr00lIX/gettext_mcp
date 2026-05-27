//! Crate-wide error type for the gettext-mcp server.
//!
//! Merges what used to live in `store.rs` as separate `ParseError` and
//! `StoreError` enums into a single [`GettextError`] so every layer
//! (parser, serializer, file I/O, store, MCP tools) can speak the same
//! error vocabulary.

use std::path::PathBuf;

/// Unified error type returned by the parser, serializer, file I/O,
/// store, and MCP tool layers.
#[derive(Debug, thiserror::Error)]
pub enum GettextError {
    /// PO format error surfaced by the parser.
    #[error("Invalid PO format: {0}")]
    InvalidFormat(String),

    /// Lookup failed: no entry with the given key exists.
    #[error("Translation not found for key: {key}, context: {context:?}")]
    TranslationNotFound {
        key: String,
        context: Option<String>,
    },

    /// A tool was called without `path` in dynamic mode, or without a
    /// configured default file in single-file mode.
    #[error("Path required for dynamic mode")]
    PathRequired,

    /// Path validation rejected the caller-supplied path (traversal,
    /// outside base directory, etc.).
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// Generic invalid-argument error from the store or a tool handler.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Another process (likely Poedit or a translator's editor) holds an
    /// exclusive lock on the file we tried to write.
    #[error("File is locked by another process: {path}")]
    FileLocked { path: PathBuf },

    /// Underlying `std::io::Error` from a filesystem call.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Backwards-compatible alias for code paths (and tests) that still spell
/// the parser error as `ParseError`. Both names point at the same enum.
pub type ParseError = GettextError;

/// Backwards-compatible alias for the pre-refactor `StoreError` name.
pub type StoreError = GettextError;

impl From<GettextError> for rmcp::model::ErrorData {
    fn from(e: GettextError) -> Self {
        rmcp::model::ErrorData::new(rmcp::model::ErrorCode::INTERNAL_ERROR, e.to_string(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_not_found_display() {
        let err = GettextError::TranslationNotFound {
            key: "Hello".into(),
            context: Some("menu".into()),
        };
        let s = err.to_string();
        assert!(s.contains("Hello"));
        assert!(s.contains("menu"));
    }

    #[test]
    fn file_locked_display_includes_path() {
        let err = GettextError::FileLocked {
            path: PathBuf::from("/tmp/messages.po"),
        };
        assert!(err.to_string().contains("/tmp/messages.po"));
    }

    #[test]
    fn aliases_are_the_same_type() {
        // Compile-time check: ParseError, StoreError, GettextError are the same.
        fn assert_same<T>(_: &T, _: &T) {}
        let e1: GettextError = GettextError::PathRequired;
        let e2: ParseError = GettextError::PathRequired;
        let e3: StoreError = GettextError::PathRequired;
        assert_same(&e1, &e2);
        assert_same(&e1, &e3);
    }
}
