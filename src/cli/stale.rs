//! `stale` subcommand — list obsolete (`#~`) entries from the PO file.
//!
//! The store layer doesn't surface obsolete lines directly; we read them
//! from disk and emit one entry per `#~ msgid` block. When the
//! concurrent `get_stale` tool lands, swap to delegate to it.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{build_manager, handle_error, print_json, EXIT_OK};

pub fn run(path: Option<PathBuf>, json: bool) -> ExitCode {
    let (resolved, _manager) = match build_manager(path) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    let content = match std::fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) => return handle_error(gettext_mcp::GettextError::Io(e)),
    };

    let entries = parse_obsolete(&content);

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "obsolete": entries,
            "count": entries.len(),
        });
        return print_json(&value);
    }

    if entries.is_empty() {
        println!("No obsolete entries found.");
        return ExitCode::from(EXIT_OK);
    }
    println!("Found {} obsolete entry/entries:", entries.len());
    println!();
    for e in &entries {
        let msgid = e.get("msgid").and_then(|v| v.as_str()).unwrap_or("");
        let msgstr = e.get("msgstr").and_then(|v| v.as_str()).unwrap_or("");
        println!("  {msgid:?}");
        if !msgstr.is_empty() {
            println!("    -> {msgstr:?}");
        }
    }
    ExitCode::from(EXIT_OK)
}

/// Extract obsolete entries by reading consecutive `#~` lines. This is
/// a deliberately minimal parser since we just need `msgid`/`msgstr`
/// pairs for display.
fn parse_obsolete(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut current_msgid: Option<String> = None;
    let mut current_msgstr: Option<String> = None;

    let flush = |msgid: &mut Option<String>,
                 msgstr: &mut Option<String>,
                 out: &mut Vec<serde_json::Value>| {
        if let Some(id) = msgid.take() {
            let val = msgstr.take().unwrap_or_default();
            out.push(json!({ "msgid": id, "msgstr": val }));
        }
    };

    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#~ msgid ") {
            flush(&mut current_msgid, &mut current_msgstr, &mut out);
            current_msgid = Some(unquote(rest));
        } else if let Some(rest) = trimmed.strip_prefix("#~ msgstr ") {
            current_msgstr = Some(unquote(rest));
        } else if !trimmed.starts_with("#~") {
            flush(&mut current_msgid, &mut current_msgstr, &mut out);
        }
    }
    flush(&mut current_msgid, &mut current_msgstr, &mut out);
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn stale_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\n#~ msgid \"Old\"\n#~ msgstr \"Vieux\"\n",
        )
        .unwrap();
        let code = run(Some(path), true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
    }

    #[test]
    fn parse_obsolete_finds_entries() {
        let content = "msgid \"\"\nmsgstr \"\"\n\n#~ msgid \"Old\"\n#~ msgstr \"Vieux\"\n";
        let entries = parse_obsolete(content);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["msgid"], "Old");
        assert_eq!(entries[0]["msgstr"], "Vieux");
    }
}
