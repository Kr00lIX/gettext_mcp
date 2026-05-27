//! `header` subcommand group — list/get/set PO header fields.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use serde_json::json;

use super::common::{build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK};

#[derive(Subcommand, Debug)]
pub enum HeaderCommand {
    /// List all header fields.
    List {
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Get a single header field.
    Get {
        /// Header key (e.g. Language, Plural-Forms).
        key: String,
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Set a header field.
    Set {
        /// Header key.
        key: String,
        /// Header value.
        value: String,
        /// Path to .po/.pot file (auto-discovered if omitted).
        #[arg(long)]
        path: Option<PathBuf>,
        /// Preview without writing.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(cmd: HeaderCommand, json: bool) -> ExitCode {
    match cmd {
        HeaderCommand::List { path } => run_list(path, json),
        HeaderCommand::Get { key, path } => run_get(key, path, json),
        HeaderCommand::Set {
            key,
            value,
            path,
            dry_run,
        } => run_set(key, value, path, dry_run, json),
    }
}

fn run_list(path: Option<PathBuf>, json: bool) -> ExitCode {
    let (resolved, manager) = match build_manager(path) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start runtime: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let metadata = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        store.metadata().await
    }) {
        Ok(m) => m,
        Err(e) => return handle_error(e),
    };
    let language = metadata.get("Language").cloned();
    let meta_json: serde_json::Map<String, serde_json::Value> = metadata
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    if json {
        let value = json!({
            "metadata": serde_json::Value::Object(meta_json),
            "language": language,
        });
        return print_json(&value);
    }
    println!("Header for {}:", resolved.display());
    if metadata.is_empty() {
        println!("  (empty)");
    } else {
        for (k, v) in &metadata {
            println!("  {k}: {v}");
        }
    }
    ExitCode::from(EXIT_OK)
}

fn run_get(key: String, path: Option<PathBuf>, json: bool) -> ExitCode {
    let (_resolved, manager) = match build_manager(path) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start runtime: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let meta = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        store.metadata().await
    }) {
        Ok(m) => m,
        Err(e) => return handle_error(e),
    };
    let value = meta.get(&key).cloned();
    if json {
        let out = json!({
            "key": key,
            "value": value,
        });
        return print_json(&out);
    }
    match value {
        Some(v) => println!("{v}"),
        None => {
            eprintln!("header key {key:?} not set");
            return ExitCode::from(super::common::EXIT_ERROR);
        }
    }
    ExitCode::from(EXIT_OK)
}

fn run_set(
    key: String,
    value: String,
    path: Option<PathBuf>,
    dry_run: bool,
    json: bool,
) -> ExitCode {
    let (_resolved, manager) = match build_manager(path) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start runtime: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };

    if dry_run {
        if json {
            let out = json!({
                "key": key,
                "value": value,
                "dry_run": true,
            });
            return print_json(&out);
        }
        println!("[dry-run] would set header {key}={value:?}");
        return ExitCode::from(EXIT_OK);
    }

    let result = rt.block_on(async {
        let store = manager.store_for(None).await?;
        store.set_header(&key, &value).await
    });
    if let Err(e) = result {
        return handle_error(e);
    }
    if json {
        let out = json!({
            "key": key,
            "value": value,
            "dry_run": false,
        });
        return print_json(&out);
    }
    println!("Set header {key}={value:?}");
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn po_with_header() -> &'static str {
        "msgid \"\"\nmsgstr \"Language: fr\\nPlural-Forms: nplurals=2; plural=(n > 1);\\n\"\n"
    }

    #[test]
    fn header_list_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, po_with_header()).unwrap();
        let code = run(
            HeaderCommand::List { path: Some(path) },
            true,
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
    }

    #[test]
    fn header_get_returns_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, po_with_header()).unwrap();
        let code = run(
            HeaderCommand::Get {
                key: "Language".into(),
                path: Some(path),
            },
            true,
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
    }

    #[test]
    fn header_set_writes_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, po_with_header()).unwrap();
        let code = run(
            HeaderCommand::Set {
                key: "Last-Translator".into(),
                value: "test".into(),
                path: Some(path.clone()),
                dry_run: false,
            },
            true,
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Last-Translator: test"));
    }

    #[test]
    fn header_set_dry_run_does_not_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, po_with_header()).unwrap();
        let code = run(
            HeaderCommand::Set {
                key: "Last-Translator".into(),
                value: "ghost".into(),
                path: Some(path.clone()),
                dry_run: true,
            },
            true,
        );
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("Last-Translator: ghost"));
    }
}
