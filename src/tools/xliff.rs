//! XLIFF import/export tool handlers.
//!
//! Tools:
//!
//! * `export_xliff` — read a PO file via the manager, render it to XLIFF
//!   1.2, and write the result to a caller-supplied output path through
//!   the same [`FileStore`] used for PO writes. Plural and obsolete
//!   entries are skipped (XLIFF 1.2 has no clean plural model).
//! * `import_xliff` — read an XLIFF file, match each `<trans-unit>` to a
//!   PO entry by `msgid` (and `msgctxt` when the unit carries the
//!   `gettext-msgctxt` note), and overwrite `msgstr`. Units whose source
//!   has no matching PO entry are reported under `unmatched`; units whose
//!   format specifiers don't line up with the source are rejected.
//!
//! The handlers preserve other entry metadata (comments, source
//! locations, flags) by going through [`crate::service::store::GettextStore::update_entry`].

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::xliff as xliff_svc;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ExportXliffParams {
    /// Path to the source .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Path where the XLIFF document will be written. Must live within
    /// the configured base directory when one is set.
    pub output: String,
    /// Language being translated TO (e.g. `"fr"`). Written as the
    /// `target-language` attribute on the `<file>` element.
    pub target_language: String,
    /// Language of the msgids. Defaults to the PO header's `Language` (or
    /// `X-Source-Language`), and finally `"en"` if neither is set.
    pub source_language: Option<String>,
    /// When `true`, every non-plural, non-obsolete entry is emitted. When
    /// `false` (the default), only entries with an empty `msgstr` or the
    /// `fuzzy` flag are emitted.
    pub include_translated: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ImportXliffParams {
    /// Path to the target .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Path to the XLIFF document to read.
    pub xliff_path: String,
    /// When `true`, compute what *would* happen but do not write the PO
    /// file. Default `false`.
    pub dry_run: Option<bool>,
    /// When `true`, each imported entry is marked with the `fuzzy` flag
    /// so a reviewer can confirm the machine-translated text. Default
    /// `false`.
    pub mark_fuzzy: Option<bool>,
}

pub(crate) async fn handle_export_xliff(
    manager: &GettextStoreManager,
    params: ExportXliffParams,
) -> Result<Value, GettextError> {
    if params.target_language.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "target_language must not be empty".into(),
        ));
    }

    let store = manager.store_for(params.path.as_deref()).await?;

    let metadata = store.metadata().await?;
    // The PO header's `Language` is the *target* language, so we don't
    // fall back to it here. We use `X-Source-Language` if the PO declares
    // one, otherwise default to "en".
    let source_language = params
        .source_language
        .clone()
        .or_else(|| metadata.get("X-Source-Language").cloned())
        .unwrap_or_else(|| "en".to_string());

    let include_translated = params.include_translated.unwrap_or(false);

    // Snapshot of the parsed file. We need a `GettextFile` for the
    // serializer; build one from `list_all` + `metadata` so we don't have
    // to thread a private accessor through the store.
    let file = build_file_snapshot(&store).await?;
    let counts = xliff_svc::count_skipped(&file, include_translated);
    let xml = xliff_svc::export_to_xliff(
        &file,
        &params.target_language,
        &source_language,
        include_translated,
    )?;

    let output_path = PathBuf::from(&params.output);
    manager.validate_path(&output_path)?;

    // Reject obviously-wrong extensions so we don't accidentally write
    // XML into a .po file.
    match output_path.extension().and_then(|e| e.to_str()) {
        Some("xliff") | Some("xlf") | Some("xml") => {}
        _ => {
            return Err(GettextError::InvalidPath(
                "output file must use .xliff, .xlf, or .xml extension".into(),
            ));
        }
    }

    let file_store = manager.file_store().clone();
    let xml_clone = xml;
    let path_clone = output_path.clone();
    tokio::task::spawn_blocking(move || file_store.write(&path_clone, &xml_clone))
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    Ok(json!({
        "exported_path": output_path.to_string_lossy(),
        "unit_count": counts.unit_count,
        "skipped_plural": counts.skipped_plural,
        "skipped_obsolete": counts.skipped_obsolete,
        "source_language": source_language,
        "target_language": params.target_language,
    }))
}

pub(crate) async fn handle_import_xliff(
    manager: &GettextStoreManager,
    params: ImportXliffParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let dry_run = params.dry_run.unwrap_or(false);
    let mark_fuzzy = params.mark_fuzzy.unwrap_or(false);

    let xliff_path = PathBuf::from(&params.xliff_path);
    manager.validate_path(&xliff_path)?;

    let file_store = manager.file_store().clone();
    let xliff_path_clone = xliff_path.clone();
    let xml = tokio::task::spawn_blocking(move || file_store.read(&xliff_path_clone))
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    let parsed = xliff_svc::parse_xliff(&xml)?;

    let mut imported = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    let mut rejected: Vec<Value> = Vec::new();

    for unit in &parsed.units {
        // Locate the matching PO entry. We try `(msgid, msgctxt)` first;
        // if that fails and no msgctxt is set we just key off msgid.
        let lookup = store.get(&unit.msgid, unit.msgctxt.as_deref()).await.ok();

        let Some(mut entry) = lookup else {
            unmatched.push(unit.msgid.clone());
            continue;
        };

        // Skip plural entries — the XLIFF unit can't carry their forms.
        if entry.msgid_plural.is_some() {
            rejected.push(json!({
                "msgid": unit.msgid,
                "reason": "PO entry has plural forms which XLIFF 1.2 cannot represent",
            }));
            continue;
        }

        // Skip blank translations rather than wiping an existing msgstr.
        if unit.msgstr.is_empty() {
            continue;
        }

        if let Some(detail) = format_specifier_mismatch(&unit.msgid, &unit.msgstr) {
            rejected.push(json!({
                "msgid": unit.msgid,
                "reason": detail,
            }));
            continue;
        }

        entry.msgstr = unit.msgstr.clone();
        if mark_fuzzy {
            if !entry.flags.iter().any(|f| f == "fuzzy") {
                entry.flags.push("fuzzy".into());
            }
        } else {
            entry.flags.retain(|f| f != "fuzzy");
        }

        if !dry_run {
            store
                .update_entry(&unit.msgid, unit.msgctxt.as_deref(), entry)
                .await?;
        }
        imported += 1;
    }

    Ok(json!({
        "imported": imported,
        "unmatched": unmatched,
        "rejected": rejected,
        "dry_run": dry_run,
        "source_language": parsed.source_language,
        "target_language": parsed.target_language,
    }))
}

/// Rebuild a [`GettextFile`] snapshot from the store's public surface.
///
/// We avoid grabbing the inner `RwLock` directly so the store API stays
/// minimal; the snapshot is cheap (PO files are small relative to the
/// per-tool overhead) and lets the serializer treat the data as a plain
/// owned value.
async fn build_file_snapshot(
    store: &crate::service::GettextStore,
) -> Result<crate::model::GettextFile, GettextError> {
    let mut file = crate::model::GettextFile::new();
    file.metadata = store.metadata().await?;
    file.rebuild_header_entry();
    for (msgid, msgctxt, entry) in store.list_all().await? {
        file.entries.insert((msgid, msgctxt), entry);
    }
    Ok(file)
}

/// Return `Some(detail)` when source and target format-specifier multisets
/// don't match. Mirrors the validator's logic but lives here so the
/// `tools::xliff` module is self-contained.
fn format_specifier_mismatch(source: &str, target: &str) -> Option<String> {
    let mut src = extract_specifiers(source);
    let mut dst = extract_specifiers(target);
    if src == dst {
        return None;
    }
    src.sort();
    dst.sort();
    if src == dst {
        return None;
    }
    Some(format!(
        "format specifier mismatch: source {src:?} vs target {dst:?}"
    ))
}

/// Lightweight printf-/brace-style specifier extractor. Recognises `%s`,
/// `%d`, `%lld`, `%@`, `%1$@`, `%.2f`, plus `{}`, `{0}`, `{name}`.
fn extract_specifiers(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'%' {
                i += 2;
                continue;
            }
            if let Some((spec, consumed)) = parse_percent(&bytes[i..]) {
                out.push(spec);
                i += consumed;
                continue;
            }
            i += 1;
            continue;
        }
        if b == b'{' {
            if let Some((spec, consumed)) = parse_brace(&bytes[i..]) {
                out.push(spec);
                i += consumed;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn parse_percent(bytes: &[u8]) -> Option<(String, usize)> {
    let mut idx = 1;
    // Positional `1$`.
    let start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > start && idx < bytes.len() && bytes[idx] == b'$' {
        idx += 1;
    } else {
        idx = start;
    }
    // Flags.
    while idx < bytes.len() && matches!(bytes[idx], b'-' | b'+' | b' ' | b'0' | b'#' | b'\'') {
        idx += 1;
    }
    // Width.
    if idx < bytes.len() && bytes[idx] == b'*' {
        idx += 1;
    } else {
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
    }
    // Precision.
    if idx < bytes.len() && bytes[idx] == b'.' {
        idx += 1;
        if idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
        } else {
            while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                idx += 1;
            }
        }
    }
    // Length modifier.
    let mut len_chars = 0;
    while idx < bytes.len() && len_chars < 2 {
        if matches!(
            bytes[idx],
            b'h' | b'l' | b'q' | b'L' | b'z' | b't' | b'j' | b'Z'
        ) {
            idx += 1;
            len_chars += 1;
        } else {
            break;
        }
    }
    if idx >= bytes.len() {
        return None;
    }
    let conv = bytes[idx];
    if !matches!(
        conv,
        b's' | b'd'
            | b'i'
            | b'u'
            | b'o'
            | b'x'
            | b'X'
            | b'f'
            | b'F'
            | b'e'
            | b'E'
            | b'g'
            | b'G'
            | b'a'
            | b'A'
            | b'c'
            | b'p'
            | b'n'
            | b'@'
    ) {
        return None;
    }
    idx += 1;
    let raw = std::str::from_utf8(&bytes[..idx]).ok()?.to_string();
    Some((raw, idx))
}

fn parse_brace(bytes: &[u8]) -> Option<(String, usize)> {
    if bytes.is_empty() || bytes[0] != b'{' {
        return None;
    }
    let mut idx = 1;
    while idx < bytes.len() {
        let b = bytes[idx];
        if b == b'}' {
            let raw = std::str::from_utf8(&bytes[..=idx]).ok()?.to_string();
            return Some((raw, idx + 1));
        }
        if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':') {
            return None;
        }
        idx += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn make_manager(path: &std::path::Path) -> Arc<GettextStoreManager> {
        Arc::new(GettextStoreManager::new(Some(path.to_path_buf())))
    }

    #[tokio::test]
    async fn export_writes_xliff_to_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("out.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let result = handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: xliff.to_str().unwrap().into(),
                target_language: "fr".into(),
                source_language: Some("en".into()),
                include_translated: Some(false),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["unit_count"], 1);
        let xml = std::fs::read_to_string(&xliff).unwrap();
        assert!(xml.contains("<xliff"));
        assert!(xml.contains("target-language=\"fr\""));
        assert!(xml.contains(">Hello<"));
        // "World"->"Monde" is translated and include_translated=false.
        assert!(!xml.contains(">World<"));
    }

    #[tokio::test]
    async fn export_include_translated_emits_all_non_plural() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("out.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "", None).await.unwrap();

        let result = handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: xliff.to_str().unwrap().into(),
                target_language: "fr".into(),
                source_language: Some("en".into()),
                include_translated: Some(true),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["unit_count"], 2);
    }

    #[tokio::test]
    async fn export_skips_plurals_and_reports_count() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("out.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert_full(
                "%d cat",
                None,
                "",
                Some("%d cats"),
                Some(vec!["%d chat".into(), "%d chats".into()]),
                None,
            )
            .await
            .unwrap();
        store.upsert("Simple", None, "", None).await.unwrap();

        let result = handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: xliff.to_str().unwrap().into(),
                target_language: "fr".into(),
                source_language: None,
                include_translated: Some(true),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["unit_count"], 1);
        assert_eq!(result["skipped_plural"], 1);
    }

    #[tokio::test]
    async fn export_rejects_non_xliff_extension() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let bad = dir.path().join("out.po");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();

        let result = handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: bad.to_str().unwrap().into(),
                target_language: "fr".into(),
                source_language: None,
                include_translated: None,
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn export_rejects_empty_target_language() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("out.xliff");
        let manager = make_manager(&po).await;
        let _ = manager.store_for(None).await.unwrap();

        let result = handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: xliff.to_str().unwrap().into(),
                target_language: "  ".into(),
                source_language: None,
                include_translated: None,
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn import_updates_existing_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        // Seed PO with an empty translation we'll fill in.
        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();
        store.upsert("World", None, "", None).await.unwrap();

        // Build XLIFF via export so we know the formatting is well-formed,
        // then patch in target text.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Bonjour</target>
      </trans-unit>
      <trans-unit id="World">
        <source>World</source>
        <target state="translated">Monde</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: Some(false),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["imported"], 2);
        assert!(result["unmatched"].as_array().unwrap().is_empty());
        assert!(result["rejected"].as_array().unwrap().is_empty());

        let hello = store.get("Hello", None).await.unwrap();
        assert_eq!(hello.msgstr, "Bonjour");
        let world = store.get("World", None).await.unwrap();
        assert_eq!(world.msgstr, "Monde");
    }

    #[tokio::test]
    async fn import_dry_run_does_not_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Bonjour</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(true),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["imported"], 1);
        assert_eq!(result["dry_run"], true);

        let hello = store.get("Hello", None).await.unwrap();
        assert_eq!(hello.msgstr, "", "dry_run must not persist the translation");
    }

    #[tokio::test]
    async fn import_reports_unmatched() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Existing", None, "", None).await.unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Existing">
        <source>Existing</source>
        <target state="translated">Existant</target>
      </trans-unit>
      <trans-unit id="Ghost">
        <source>Ghost</source>
        <target state="translated">Fantôme</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["imported"], 1);
        let unmatched = result["unmatched"].as_array().unwrap();
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0], "Ghost");
    }

    #[tokio::test]
    async fn import_rejects_format_specifier_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello %s", None, "", None).await.unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello %s">
        <source>Hello %s</source>
        <target state="translated">Bonjour %d</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["imported"], 0);
        let rejected = result["rejected"].as_array().unwrap();
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0]["reason"]
            .as_str()
            .unwrap()
            .contains("format specifier"));

        // Entry untouched on disk.
        assert_eq!(store.get("Hello %s", None).await.unwrap().msgstr, "");
    }

    #[tokio::test]
    async fn import_mark_fuzzy_adds_flag() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Bonjour</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: Some(true),
            },
        )
        .await
        .unwrap();

        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Bonjour");
        assert!(entry.flags.iter().any(|f| f == "fuzzy"));
    }

    #[tokio::test]
    async fn import_clears_fuzzy_when_not_marking() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Hello", None, "", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Bonjour</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: Some(false),
            },
        )
        .await
        .unwrap();

        let entry = store.get("Hello", None).await.unwrap();
        assert!(!entry.flags.iter().any(|f| f == "fuzzy"));
    }

    #[tokio::test]
    async fn import_preserves_comments_and_source_locations() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "", None).await.unwrap();

        // Inject a translator comment + source location through update_entry.
        let mut entry = store.get("Hello", None).await.unwrap();
        entry.translator_comment = vec!["A greeting".into()];
        entry.source_locations = vec!["src/lib.rs:42".into()];
        store.update_entry("Hello", None, entry).await.unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Bonjour</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();

        let updated = store.get("Hello", None).await.unwrap();
        assert_eq!(updated.msgstr, "Bonjour");
        assert_eq!(updated.translator_comment, vec!["A greeting".to_string()]);
        assert_eq!(updated.source_locations, vec!["src/lib.rs:42".to_string()]);
    }

    #[tokio::test]
    async fn import_matches_by_msgctxt_note() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("in.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Open", Some("menu"), "", None).await.unwrap();
        store
            .upsert("Open", Some("button"), "", None)
            .await
            .unwrap();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="menu_Open">
        <source>Open</source>
        <target state="translated">Ouvrir le menu</target>
        <note from="gettext-msgctxt">menu</note>
      </trans-unit>
      <trans-unit id="button_Open">
        <source>Open</source>
        <target state="translated">Ouvrir le bouton</target>
        <note from="gettext-msgctxt">button</note>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        std::fs::write(&xliff, xml).unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["imported"], 2);

        assert_eq!(
            store.get("Open", Some("menu")).await.unwrap().msgstr,
            "Ouvrir le menu"
        );
        assert_eq!(
            store.get("Open", Some("button")).await.unwrap().msgstr,
            "Ouvrir le bouton"
        );
    }

    #[tokio::test]
    async fn round_trip_export_then_import_preserves_translations() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let xliff = dir.path().join("rt.xliff");

        let manager = make_manager(&po).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "", None).await.unwrap();

        // Export everything.
        handle_export_xliff(
            &manager,
            ExportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                output: xliff.to_str().unwrap().into(),
                target_language: "fr".into(),
                source_language: Some("en".into()),
                include_translated: Some(true),
            },
        )
        .await
        .unwrap();

        // Clear translations on disk to prove import re-applies them.
        store.upsert("Hello", None, "", None).await.unwrap();

        let result = handle_import_xliff(
            &manager,
            ImportXliffParams {
                path: Some(po.to_str().unwrap().into()),
                xliff_path: xliff.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_fuzzy: None,
            },
        )
        .await
        .unwrap();
        // "World"'s target is empty in the XLIFF, so only "Hello" is imported.
        assert_eq!(result["imported"], 1);
        assert_eq!(store.get("Hello", None).await.unwrap().msgstr, "Bonjour");
    }

    #[test]
    fn extract_specifiers_basic() {
        assert_eq!(
            extract_specifiers("Hello %s, you have %d items"),
            vec!["%s", "%d"]
        );
        assert_eq!(extract_specifiers("100%% done"), Vec::<String>::new());
        assert_eq!(extract_specifiers("{name} {0}"), vec!["{name}", "{0}"]);
    }

    #[test]
    fn format_specifier_mismatch_detects() {
        assert!(format_specifier_mismatch("Hello %s", "Bonjour %d").is_some());
        // Reordering is fine — same multiset.
        assert!(format_specifier_mismatch("%s %d", "%d %s").is_none());
        // Equal sets.
        assert!(format_specifier_mismatch("Plain", "Texte").is_none());
    }
}
