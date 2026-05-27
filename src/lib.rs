//! Gettext MCP server library.
//!
//! The crate is organized into layers:
//!
//! - [`error`] — unified [`error::GettextError`] type
//! - [`model`] — pure data types (`MessageEntry`, `GettextFile`)
//! - [`service::parser`] / [`service::serializer`] — PO format codec
//! - [`io`] — [`io::FileStore`] trait + [`io::FsFileStore`] impl
//! - [`service::store`] / [`service::manager`] — per-file CRUD + cache
//! - [`tools`] — per-tool handler functions
//! - [`server`] — [`server::GettextMcpServer`] with `#[tool_router]`
//! - [`prompts`] — empty `#[prompt_router]` stub
//! - [`web`] — optional axum-based HTTP UI

pub mod error;
pub mod io;
pub mod model;
pub(crate) mod prompts;
pub mod server;
pub mod service;
pub mod tools;
pub mod web;

pub use error::{GettextError, ParseError, StoreError};
pub use io::{cleanup_orphan_tmps, FileStore, FsFileStore};
pub use model::{GettextFile, MessageEntry};
pub use server::GettextMcpServer;
pub use service::{GettextStore, GettextStoreManager};
pub use web::WebConfig;
