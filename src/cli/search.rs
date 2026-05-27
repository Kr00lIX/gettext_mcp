//! `search` subcommand — substring search across msgid + msgstr.
//!
//! Delegates to the existing `handle_list_translations` handler with the
//! `query` parameter set. When the concurrent `search_keys` tool lands,
//! swap to it without changing the CLI surface.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK};

pub fn run(pattern: String, path: Option<PathBuf>, limit: usize, json: bool) -> ExitCode {
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

    let results: Vec<serde_json::Value> = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        let entries = store.list_all().await?;
        let q = pattern.to_lowercase();
        let mut hits = Vec::new();
        for (msgid, msgctxt, entry) in entries {
            let matches = msgid.to_lowercase().contains(&q)
                || entry.msgstr.to_lowercase().contains(&q)
                || entry
                    .msgid_plural
                    .as_deref()
                    .is_some_and(|p| p.to_lowercase().contains(&q))
                || entry
                    .msgstr_plural
                    .iter()
                    .any(|p| p.to_lowercase().contains(&q));
            if matches {
                hits.push(json!({
                    "msgid": msgid,
                    "msgctxt": msgctxt,
                    "msgstr": entry.msgstr,
                    "msgid_plural": entry.msgid_plural,
                    "msgstr_plural": entry.msgstr_plural,
                    "is_translated": entry.is_translated(),
                    "is_fuzzy": entry.is_fuzzy(),
                }));
                if hits.len() >= limit {
                    break;
                }
            }
        }
        Ok::<_, gettext_mcp::GettextError>(hits)
    }) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    if json {
        return print_json(&serde_json::Value::Array(results));
    }

    if results.is_empty() {
        println!("No entries matching \"{pattern}\".");
        return ExitCode::from(EXIT_OK);
    }

    println!(
        "Found {} entry/entries matching \"{pattern}\":",
        results.len()
    );
    println!();
    for item in &results {
        let msgid = item.get("msgid").and_then(|v| v.as_str()).unwrap_or("");
        let msgstr = item.get("msgstr").and_then(|v| v.as_str()).unwrap_or("");
        let msgctxt = item.get("msgctxt").and_then(|v| v.as_str());
        let ctx_part = msgctxt.map(|c| format!(" [{c}]")).unwrap_or_default();
        println!("  {msgid:?}{ctx_part}");
        println!("    -> {msgstr:?}");
    }
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn search_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();
        let code = run("Hello".into(), Some(path), 50, true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
    }
}
