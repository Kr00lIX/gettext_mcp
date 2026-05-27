//! `info` subcommand — file summary (path, language, totals).

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;

use super::common::{build_manager, handle_error, print_json, runtime, EXIT_ERROR, EXIT_OK};

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

    let (language, total, translated, fuzzy, obsolete) = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        let language = store.language().await?;
        let entries = store.list_all().await?;
        let total = entries.len();
        let translated = entries.iter().filter(|(_, _, e)| e.is_translated()).count();
        let fuzzy = entries.iter().filter(|(_, _, e)| e.is_fuzzy()).count();
        // Obsolete lines aren't exposed directly; approximate by counting "#~ msgid" lines in serializer output is overkill — use metadata pass-through.
        // Instead, read raw obsolete_lines via a lightweight serializer round-trip is not available; we count via the on-disk file as a fallback.
        let obsolete = count_obsolete_in_file(&resolved);
        Ok::<_, gettext_mcp::GettextError>((language, total, translated, fuzzy, obsolete))
    }) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "language": language,
            "total_entries": total,
            "translated": translated,
            "fuzzy": fuzzy,
            "obsolete": obsolete,
        });
        return print_json(&value);
    }

    let file_name = resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| resolved.display().to_string());
    println!("File: {file_name}");
    println!("Path: {}", resolved.display());
    println!("Language: {}", language.as_deref().unwrap_or("(unset)"));
    println!("Total entries: {total}");
    println!("Translated:    {translated}");
    println!("Fuzzy:         {fuzzy}");
    println!("Obsolete:      {obsolete}");

    ExitCode::from(EXIT_OK)
}

/// Approximate obsolete-line count by reading the file and counting
/// `#~ msgid` markers. Acceptable for CLI display since the parser
/// preserves obsolete lines verbatim.
fn count_obsolete_in_file(path: &std::path::Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .filter(|l| l.trim_start().starts_with("#~ msgid"))
            .count(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_po() -> &'static str {
        "msgid \"\"\n\
         msgstr \"Language: fr\\n\"\n\
         \n\
         msgid \"Hello\"\n\
         msgstr \"Bonjour\"\n\
         \n\
         msgid \"World\"\n\
         msgstr \"\"\n\
         \n\
         #, fuzzy\n\
         msgid \"Goodbye\"\n\
         msgstr \"Adieu\"\n\
         \n\
         #~ msgid \"Old\"\n\
         #~ msgstr \"Vieux\"\n"
    }

    #[test]
    fn info_human_output_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, sample_po()).unwrap();
        let code = run(Some(path), false);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
    }

    #[test]
    fn info_json_output_runs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, sample_po()).unwrap();
        let code = run(Some(path), true);
        assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(EXIT_OK))
        );
    }
}
