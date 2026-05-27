//! `coverage` subcommand — per-file translation coverage.
//!
//! TODO: when the concurrent `tools/coverage.rs` (`get_coverage`)
//! handler lands, switch to delegating to it. For now this computes
//! coverage directly from `list_all()` so the CLI still ships.

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

    let (language, total, translated, fuzzy, untranslated) = match rt.block_on(async {
        let store = manager.store_for(None).await?;
        let language = store.language().await?;
        let entries = store.list_all().await?;
        let total = entries.len();
        let translated = entries.iter().filter(|(_, _, e)| e.is_translated()).count();
        let fuzzy = entries.iter().filter(|(_, _, e)| e.is_fuzzy()).count();
        let untranslated = total - translated - fuzzy.min(total - translated);
        Ok::<_, gettext_mcp::GettextError>((language, total, translated, fuzzy, untranslated))
    }) {
        Ok(v) => v,
        Err(e) => return handle_error(e),
    };

    let percentage = if total == 0 {
        100.0
    } else {
        (translated as f64 / total as f64) * 100.0
    };

    if json {
        let value = json!({
            "path": resolved.display().to_string(),
            "language": language,
            "total": total,
            "translated": translated,
            "fuzzy": fuzzy,
            "untranslated": untranslated,
            "percentage": percentage,
        });
        return print_json(&value);
    }

    println!(
        "{:<40} {:>6} {:>6} {:>6} {:>8}",
        "File", "Total", "OK", "Fuzzy", "Coverage"
    );
    println!("{}", "\u{2500}".repeat(70));
    let file_label = resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| resolved.display().to_string());
    let lang_suffix = language
        .as_deref()
        .map(|l| format!(" [{l}]"))
        .unwrap_or_default();
    println!(
        "{:<40} {:>6} {:>6} {:>6} {:>7.1}%",
        format!("{file_label}{lang_suffix}"),
        total,
        translated,
        fuzzy,
        percentage
    );

    ExitCode::from(EXIT_OK)
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
         msgstr \"\"\n"
    }

    #[test]
    fn coverage_runs_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, sample_po()).unwrap();
        let code = run(Some(path), true);
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
    }

    #[test]
    fn coverage_runs_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(&path, sample_po()).unwrap();
        let code = run(Some(path), false);
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(EXIT_OK)));
    }
}
