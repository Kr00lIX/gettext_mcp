//! `add-context` subcommand — copy an existing entry into a new context.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK};

pub fn run(
    msgid: String,
    msgctxt: String,
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

    let result = rt.block_on(async {
        let store = manager.store_for(None).await?;
        let source = store.get(&msgid, None).await?;

        // Refuse if (msgid, msgctxt) already exists.
        if store.get(&msgid, Some(&msgctxt)).await.is_ok() {
            return Err(gettext_mcp::GettextError::InvalidInput(format!(
                "entry already exists for msgid {msgid:?} with msgctxt {msgctxt:?}"
            )));
        }

        if dry_run {
            return Ok::<_, gettext_mcp::GettextError>(("dry-run".to_string(), source));
        }

        let mut new_entry = source.clone();
        new_entry.msgctxt = Some(msgctxt.clone());
        store
            .update_entry(&msgid, Some(&msgctxt), new_entry.clone())
            .await?;
        Ok(("created".to_string(), new_entry))
    });

    let (status, entry) = match result {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "status": status,
            "msgid": msgid,
            "msgctxt": msgctxt,
            "msgstr": entry.msgstr,
            "dry_run": dry_run,
        });
        return print_json(&value);
    }

    if dry_run {
        println!("[dry-run] would create entry msgid={msgid:?} msgctxt={msgctxt:?}");
    } else {
        println!("Created entry msgid={msgid:?} msgctxt={msgctxt:?}");
    }
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn add_context_creates_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Open\"\nmsgstr \"Ouvrir\"\n",
        )
        .unwrap();
        let code = run(
            "Open".into(),
            "menu".into(),
            Some(path.clone()),
            false,
            true,
        );
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("msgctxt \"menu\""));
    }

    #[test]
    fn add_context_dry_run_does_not_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        let original = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Open\"\nmsgstr \"Ouvrir\"\n";
        std::fs::write(&path, original).unwrap();
        let code = run("Open".into(), "menu".into(), Some(path.clone()), true, true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("msgctxt \"menu\""));
    }
}
