//! gettext-mcp binary entrypoint.
//!
//! Parses argv (optional positional path), builds the file store, store
//! manager, and MCP server, optionally launches the web UI when
//! `WEB_PORT` is set in the environment, and finally serves MCP over
//! stdio.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::{transport::io::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logs go to stderr — stdout is reserved for the MCP stdio transport.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    let args: Vec<String> = std::env::args().collect();
    let path: Option<PathBuf> = args.get(1).map(PathBuf::from);

    let manager = Arc::new(gettext_mcp::GettextStoreManager::new(path.clone()));

    // Best-effort orphan-temp-file cleanup before we start writing.
    let cleanup_dir: PathBuf = match path.as_ref() {
        Some(p) if p.is_dir() => p.clone(),
        Some(p) => p
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    gettext_mcp::cleanup_orphan_tmps(&cleanup_dir);

    if manager.is_directory_mode() {
        match manager.scan_directory().await {
            Ok(count) => eprintln!("Discovered {} .po/.pot files", count),
            Err(e) => eprintln!("Warning: failed to scan directory: {}", e),
        }
    }

    if std::env::var("WEB_PORT").is_ok() {
        let host = std::env::var("WEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("WEB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787);
        let addr: SocketAddr = format!("{}:{}", host, port).parse()?;

        let web_manager = Arc::clone(&manager);
        tokio::spawn(async move {
            let config = gettext_mcp::WebConfig {
                addr,
                manager: web_manager,
            };
            if let Err(e) = gettext_mcp::web::serve(config).await {
                eprintln!("Web server error: {}", e);
            }
        });

        eprintln!("Web UI enabled at http://{}:{}", host, port);
    }

    let mcp_server = gettext_mcp::GettextMcpServer::new(Arc::clone(&manager));
    eprintln!("Gettext MCP Server initialized and ready for connections");

    let service = mcp_server.serve(stdio()).await?;
    let quit_reason = service.waiting().await?;
    eprintln!("MCP service stopped: {:?}", quit_reason);

    Ok(())
}
