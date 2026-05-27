use std::path::PathBuf;
use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging to stderr to avoid interfering with stdio MCP protocol
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    // Parse arguments and environment
    let args: Vec<String> = std::env::args().collect();
    let path: Option<PathBuf> = args.get(1).map(PathBuf::from);

    // Create store manager
    let manager = Arc::new(gettext_mcp::GettextStoreManager::new(path.clone()));

    // Best-effort cleanup of orphan `.gettext-mcp-*.tmp` files left over
    // from previous crashed writes. Scans either the configured directory
    // (directory mode) or the parent of the configured file (single-file
    // mode). Falls back to CWD when no path was supplied.
    let cleanup_dir: PathBuf = match path.as_ref() {
        Some(p) if p.is_dir() => p.clone(),
        Some(p) => p
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    gettext_mcp::store::cleanup_orphan_tmps(&cleanup_dir);

    // If the path is a directory, scan for .po/.pot files
    if manager.is_directory_mode() {
        match manager.scan_directory().await {
            Ok(count) => eprintln!("Discovered {} .po/.pot files", count),
            Err(e) => eprintln!("Warning: failed to scan directory: {}", e),
        }
    }

    // Check if web UI is enabled
    let web_enabled = std::env::var("WEB_PORT").is_ok();

    if web_enabled {
        // Parse web configuration
        let host = std::env::var("WEB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port: u16 = std::env::var("WEB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787);

        let addr: SocketAddr = format!("{}:{}", host, port).parse()?;

        // Spawn web server in background
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

    // Initialize MCP server and serve over stdio
    let mcp_server = gettext_mcp::GettextMcpServer::new(Arc::clone(&manager));
    eprintln!("Gettext MCP Server initialized and ready for connections");

    let service = mcp_server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await?;
    let quit_reason = service.waiting().await?;
    eprintln!("MCP service stopped: {:?}", quit_reason);

    Ok(())
}
