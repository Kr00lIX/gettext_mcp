//! MCP prompt router for the gettext server.
//!
//! Empty by design: a follow-up agent will populate this with prompts.
//! The router stub is required so `server.rs` can wire up
//! `#[prompt_handler]` even when no prompts are defined.

use rmcp::prompt_router;

use crate::server::GettextMcpServer;

#[prompt_router(vis = "pub(crate)")]
impl GettextMcpServer {}
