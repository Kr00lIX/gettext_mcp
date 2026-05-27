//! `compile_mo` — in-house equivalent of `msgfmt`.
//!
//! Reads a PO file via the manager, encodes it to a binary `.mo` buffer
//! through [`crate::service::mo_writer::compile_mo`], and writes the
//! bytes through the [`crate::io::FileStore`] so the .mo gets the same
//! atomic write + advisory lock treatment as a `.po` file.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::model::GettextFile;
use crate::service::mo_writer;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct CompileMoParams {
    /// Path to the source `.po` file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Path where the compiled `.mo` binary will be written. Must live
    /// within the configured base directory when one is set; the file
    /// extension should be `.mo` (validated below).
    pub output: String,
}

pub(crate) async fn handle_compile_mo(
    manager: &GettextStoreManager,
    params: CompileMoParams,
) -> Result<Value, GettextError> {
    if params.output.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "output must not be empty".into(),
        ));
    }

    let output_path = PathBuf::from(&params.output);
    manager.validate_path(&output_path)?;
    match output_path.extension().and_then(|e| e.to_str()) {
        Some("mo") | Some("gmo") => {}
        _ => {
            return Err(GettextError::InvalidPath(
                "output file must use .mo extension".into(),
            ));
        }
    }

    let store = manager.store_for(params.path.as_deref()).await?;
    let file = build_file_snapshot(&store).await?;

    let counts = mo_writer::count_compile(&file);
    let bytes = mo_writer::compile_mo(&file)?;

    let file_store = manager.file_store().clone();
    let path_for_write = output_path.clone();
    tokio::task::spawn_blocking(move || file_store.write_bytes(&path_for_write, &bytes))
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    Ok(json!({
        "compiled_path": output_path.to_string_lossy(),
        "string_count": counts.string_count,
        "skipped_fuzzy": counts.skipped_fuzzy,
        "skipped_untranslated": counts.skipped_untranslated,
    }))
}

/// Same rebuild-from-public-API trick the xliff handler uses.
async fn build_file_snapshot(
    store: &crate::service::GettextStore,
) -> Result<GettextFile, GettextError> {
    let mut file = GettextFile::new();
    file.metadata = store.metadata().await?;
    file.rebuild_header_entry();
    for (msgid, msgctxt, entry) in store.list_all().await? {
        file.entries.insert((msgid, msgctxt), entry);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
    }

    #[tokio::test]
    async fn compile_mo_writes_valid_binary() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let mo = dir.path().join("messages.mo");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let result = handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: mo.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();

        // Header + 2 entries.
        assert_eq!(result["string_count"], 3);
        assert_eq!(result["skipped_fuzzy"], 0);
        assert_eq!(result["skipped_untranslated"], 0);

        // Parse the .mo header by hand: magic + N + offsets.
        let bytes = std::fs::read(&mo).unwrap();
        let magic = read_u32_le(&bytes, 0);
        assert_eq!(magic, mo_writer::MO_MAGIC);
        let n = read_u32_le(&bytes, 8);
        assert_eq!(n, 3);

        let originals_off = read_u32_le(&bytes, 12);
        // Look at the SECOND originals entry (header is at index 0 since
        // empty msgid sorts first; "Hello" < "World" by bytes).
        let entry_off = originals_off as usize + 8;
        let len = read_u32_le(&bytes, entry_off);
        let str_off = read_u32_le(&bytes, entry_off + 4);
        let original = &bytes[str_off as usize..(str_off + len) as usize];
        assert_eq!(original, b"Hello");
    }

    #[tokio::test]
    async fn compile_mo_skips_fuzzy_and_untranslated() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let mo = dir.path().join("messages.mo");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Translated", None, "Traduit", None)
            .await
            .unwrap();
        store.upsert("Untranslated", None, "", None).await.unwrap();
        store
            .upsert("Fuzzy", None, "Flou", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();

        let result = handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: mo.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();

        // Just "Translated" (no header set, so no synthetic header).
        assert_eq!(result["string_count"], 1);
        assert_eq!(result["skipped_fuzzy"], 1);
        assert_eq!(result["skipped_untranslated"], 1);
    }

    #[tokio::test]
    async fn compile_mo_emits_plural_strings_with_nul_separators() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let mo = dir.path().join("messages.mo");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
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

        handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: mo.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();

        let bytes = std::fs::read(&mo).unwrap();
        // Verify the encoded plural string appears in the strings region.
        assert!(
            bytes
                .windows(b"%d chat\0%d chats".len())
                .any(|w| w == b"%d chat\0%d chats"),
            "encoded plural msgstr missing"
        );
        assert!(
            bytes
                .windows(b"%d cat\0%d cats".len())
                .any(|w| w == b"%d cat\0%d cats"),
            "encoded plural msgid missing"
        );
    }

    #[tokio::test]
    async fn compile_mo_rejects_non_mo_extension() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let bad = dir.path().join("messages.txt");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let result = handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: bad.to_str().unwrap().into(),
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn compile_mo_rejects_empty_output() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let _ = manager.store_for(None).await.unwrap();

        let result = handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: "".into(),
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn compile_mo_header_included() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let mo = dir.path().join("messages.mo");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store
            .set_header("Plural-Forms", "nplurals=2; plural=(n > 1);")
            .await
            .unwrap();

        let result = handle_compile_mo(
            &manager,
            CompileMoParams {
                path: Some(po.to_str().unwrap().into()),
                output: mo.to_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["string_count"], 1);

        let bytes = std::fs::read(&mo).unwrap();
        // The synthesized header bytes must appear in the strings region.
        assert!(
            bytes
                .windows(b"Language: fr".len())
                .any(|w| w == b"Language: fr"),
            "header Language line should be present in .mo strings region"
        );
    }
}
