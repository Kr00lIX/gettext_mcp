use std::path::PathBuf;
use std::net::SocketAddr;
use std::sync::Arc;

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
    let manager = Arc::new(gettext_mcp::GettextStoreManager::new(path));

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

    // Initialize MCP server
    let _mcp_server = gettext_mcp::GettextMcpServer::new(Arc::clone(&manager));
    eprintln!("Gettext MCP Server initialized and ready for connections");

    // Keep the server running
    tokio::signal::ctrl_c().await?;
    eprintln!("Shutting down...");

    Ok(())
}
