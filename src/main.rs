//! gettext-mcp binary entrypoint.
//!
//! Parses argv with [`clap`]. When no subcommand is provided the binary
//! starts the MCP server (and optionally the web UI) exactly as before.
//! When a subcommand is provided it runs that one-shot CLI operation
//! and exits — the MCP server is never started.

mod cli;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use rmcp::{transport::io::stdio, ServiceExt};

#[derive(Parser, Debug)]
#[command(
    name = "gettext-mcp",
    about = "MCP server & CLI for GNU gettext .po/.pot translation files.\n\n\
             Without a subcommand starts the MCP server (stdio transport).\n\
             Use subcommands for one-off operations from the terminal or CI.",
    version,
    after_help = "ENVIRONMENT:\n  \
                  WEB_PORT, WEB_HOST   when set, launches the web UI alongside the MCP server\n  \
                  RUST_LOG=debug       enable debug logging to stderr"
)]
pub struct Cli {
    /// Output JSON instead of human-readable text (CLI commands only).
    #[arg(long, global = true)]
    json: bool,

    /// Path to a .po/.pot file or directory (only used when no subcommand is given).
    po_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> ExitCode {
    let parsed = Cli::parse();

    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    match parsed.command {
        None => run_server(parsed.po_file),
        Some(cmd) => cli::run(cmd, parsed.json),
    }
}

fn run_server(path: Option<PathBuf>) -> ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(async move { server_main(path).await })
}

async fn server_main(path: Option<PathBuf>) -> ExitCode {
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
        let addr: SocketAddr = match format!("{}:{}", host, port).parse() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("error: invalid web address: {e}");
                return ExitCode::FAILURE;
            }
        };

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

    let service = match mcp_server.serve(stdio()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    match service.waiting().await {
        Ok(reason) => {
            eprintln!("MCP service stopped: {:?}", reason);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
