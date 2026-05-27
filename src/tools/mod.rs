//! Per-tool handler functions and parameter types.
//!
//! Each `tools/*.rs` file groups related tool handlers. The `#[tool]`
//! methods on [`crate::server::GettextMcpServer`] are thin shims that
//! call these handlers and JSON-encode the result.

pub(crate) mod crud;
pub(crate) mod discover;
pub(crate) mod header;
pub(crate) mod metadata;
