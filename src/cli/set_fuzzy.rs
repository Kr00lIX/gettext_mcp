//! `set-fuzzy` subcommand — toggle the fuzzy flag on an entry.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK};

pub fn run(
    msgid: String,
    msgctxt: Option<String>,
    clear: bool,
    path: Option<PathBuf>,
    dry_run: bool,
    json: bool,
) -> ExitCode {
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

    let new_value = !clear;

    if dry_run {
        // Just verify the entry exists.
        let exists = rt.block_on(async {
            let store = manager.store_for(None).await?;
            store.get(&msgid, msgctxt.as_deref()).await.map(|_| ())
        });
        if let Err(e) = exists {
            return handle_error(e);
        }
        if json {
            let value = json!({
                "path": resolved.display().to_string(),
                "msgid": msgid,
                "msgctxt": msgctxt,
                "fuzzy": new_value,
                "dry_run": true,
            });
            return print_json(&value);
        }
        println!("[dry-run] would set fuzzy={new_value} on msgid={msgid:?} msgctxt={msgctxt:?}");
        return ExitCode::from(EXIT_OK);
    }

    let result = rt.block_on(async {
        let store = manager.store_for(None).await?;
        let mut entry = store.get(&msgid, msgctxt.as_deref()).await?;
        if new_value {
            if !entry.flags.iter().any(|f| f == "fuzzy") {
                entry.flags.push("fuzzy".to_string());
            }
        } else {
            entry.flags.retain(|f| f != "fuzzy");
        }
        store
            .update_entry(&msgid, msgctxt.as_deref(), entry)
            .await?;
        Ok::<_, gettext_mcp::GettextError>(())
    });
    if let Err(e) = result {
        return handle_error(e);
    }

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "msgid": msgid,
            "msgctxt": msgctxt,
            "fuzzy": new_value,
            "dry_run": false,
        });
        return print_json(&value);
    }
    println!("Set fuzzy={new_value} on msgid={msgid:?} msgctxt={msgctxt:?}");
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn set_fuzzy_marks_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();
        let code = run("Hello".into(), None, false, Some(path.clone()), false, true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("#, fuzzy"));
    }

    #[test]
    fn set_fuzzy_clear_removes_flag() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\n#, fuzzy\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();
        let code = run("Hello".into(), None, true, Some(path.clone()), false, true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("#, fuzzy"));
    }
}
