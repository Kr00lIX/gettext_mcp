//! Shared helpers for the CLI subcommands.
//!
//! Centralizes auto-discovery of a `.po`/`.pot` file in the current
//! directory, store-manager construction, and error formatting so each
//! subcommand stays a thin shell over the tool handlers.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use gettext_mcp::{GettextError, GettextStoreManager};

pub const EXIT_OK: u8 = 0;
pub const EXIT_ERROR: u8 = 1;
pub const EXIT_VALIDATION_ISSUES: u8 = 2;

/// Resolve the target `.po`/`.pot` file path.
///
/// If `path` is `Some`, it's returned as-is (relative or absolute).
/// Otherwise the current working directory is scanned (non-recursively)
/// for a single `.po`/`.pot` file. Zero or multiple matches return a
/// helpful error.
pub fn resolve_file(path: Option<PathBuf>) -> Result<PathBuf, GettextError> {
    if let Some(p) = path {
        return Ok(p);
    }

    let cwd = std::env::current_dir().map_err(|e| {
        GettextError::InvalidInput(format!("cannot determine current directory: {e}"))
    })?;

    let mut matches: Vec<PathBuf> = Vec::new();
    let read = std::fs::read_dir(&cwd).map_err(|e| {
        GettextError::InvalidInput(format!("cannot read directory {}: {e}", cwd.display()))
    })?;
    for entry in read.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        match p.extension().and_then(|e| e.to_str()) {
            Some("po") | Some("pot") => matches.push(p),
            _ => {}
        }
    }
    matches.sort();

    match matches.len() {
        0 => Err(GettextError::InvalidInput(
            "no .po/.pot files found in current directory (pass --path or run from the directory containing the file)"
                .into(),
        )),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let listing = matches
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Err(GettextError::InvalidInput(format!(
                "found {n} .po/.pot files in current directory, specify one with --path:\n{listing}"
            )))
        }
    }
}

/// Build a `GettextStoreManager` rooted at the resolved single-file path
/// and return it together with the canonical absolute file path. The
/// manager is constructed in single-file mode so `store_for(None)` works.
pub fn build_manager(
    path: Option<PathBuf>,
) -> Result<(PathBuf, Arc<GettextStoreManager>), GettextError> {
    let resolved = resolve_file(path)?;
    // Canonicalize when the file exists so the manager sees an absolute
    // path (matches xcstrings-mcp behavior and keeps log messages stable).
    let absolute = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    let manager = Arc::new(GettextStoreManager::new(Some(absolute.clone())));
    Ok((absolute, manager))
}

/// Construct a single-thread tokio runtime for sync CLI entry points.
pub fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// Print error to stderr and convert to error exit code.
pub fn handle_error(err: GettextError) -> ExitCode {
    eprintln!("error: {err}");
    ExitCode::from(EXIT_ERROR)
}

/// Pretty-print a JSON value to stdout, or return an error exit code.
pub fn print_json(value: &serde_json::Value) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(s) => {
            println!("{s}");
            ExitCode::from(EXIT_OK)
        }
        Err(e) => {
            eprintln!("error: failed to serialize JSON: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}
