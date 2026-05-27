//! `validate` subcommand — translation validation.
//!
//! TODO: this currently stubs `validate_translations` since the
//! concurrent `tools/validate.rs` handler has not landed yet. It still
//! emits a useful baseline check (empty translations on non-fuzzy
//! entries, missing plural forms) so CI users get a stable command.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{
    build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK, EXIT_VALIDATION_ISSUES,
};

#[derive(Debug, serde::Serialize)]
struct Issue {
    severity: &'static str,
    msgid: String,
    msgctxt: Option<String>,
    message: String,
}

pub fn run(path: Option<PathBuf>, json: bool) -> ExitCode {
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

    let issues: Vec<Issue> = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        let entries = store.list_all().await?;
        let mut issues = Vec::new();
        for (msgid, msgctxt, entry) in entries {
            if entry.is_fuzzy() {
                issues.push(Issue {
                    severity: "warning",
                    msgid: msgid.clone(),
                    msgctxt: msgctxt.clone(),
                    message: "entry marked fuzzy".to_string(),
                });
            }
            if entry.msgid_plural.is_some() {
                if entry.msgstr_plural.is_empty()
                    || entry.msgstr_plural.iter().any(|s| s.is_empty())
                {
                    issues.push(Issue {
                        severity: "error",
                        msgid: msgid.clone(),
                        msgctxt: msgctxt.clone(),
                        message: "plural entry missing one or more msgstr[n] forms".to_string(),
                    });
                }
            } else if entry.msgstr.is_empty() && !entry.is_fuzzy() {
                issues.push(Issue {
                    severity: "warning",
                    msgid: msgid.clone(),
                    msgctxt: msgctxt.clone(),
                    message: "empty translation".to_string(),
                });
            }
        }
        Ok::<_, gettext_mcp::GettextError>(issues)
    }) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    let errors = issues.iter().filter(|i| i.severity == "error").count();
    let warnings = issues.iter().filter(|i| i.severity == "warning").count();

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "issues": issues,
            "errors": errors,
            "warnings": warnings,
        });
        let code = print_json(&value);
        return if errors > 0 {
            ExitCode::from(EXIT_VALIDATION_ISSUES)
        } else {
            code
        };
    }

    if issues.is_empty() {
        println!("No validation issues found.");
        return ExitCode::from(EXIT_OK);
    }
    for issue in &issues {
        let ctx = issue
            .msgctxt
            .as_deref()
            .map(|c| format!(" [{c}]"))
            .unwrap_or_default();
        println!(
            "  {sev:<7} {msgid:?}{ctx}: {msg}",
            sev = issue.severity.to_uppercase(),
            msgid = issue.msgid,
            ctx = ctx,
            msg = issue.message
        );
    }
    println!();
    println!("Found {errors} error(s), {warnings} warning(s)");

    if errors > 0 {
        ExitCode::from(EXIT_VALIDATION_ISSUES)
    } else {
        ExitCode::from(EXIT_OK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_runs_on_clean_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"Language: fr\\n\"\n\nmsgid \"Hi\"\nmsgstr \"Salut\"\n",
        )
        .unwrap();
        let code = run(Some(path), true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
    }
}
