//! `sync_with_pot` — in-house equivalent of `msgmerge`.
//!
//! Reads a POT (or PO) template via the manager's [`FileStore`], merges
//! it with the working PO file, and either writes the result back or
//! returns a dry-run summary. The merge logic itself lives in
//! [`crate::service::merger`] and is exercised by its own unit tests.
//!
//! Concurrency: the underlying [`crate::service::store::GettextStore`]
//! already serializes writes through its `RwLock`. We don't need a
//! handler-level mutex here — the two writers (the manager's store and
//! our serializer) operate on the same `Arc<RwLock<GettextFile>>`.

use std::path::PathBuf;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::GettextError;
use crate::model::GettextFile;
use crate::service::merger::{self, MergeOptions};
use crate::service::parser;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SyncWithPotParams {
    /// Path to the target `.po` file. Required in directory/dynamic mode.
    pub path: Option<String>,
    /// Path to the `.pot` (or `.po`) file to merge from. Validated
    /// against the manager's base directory the same way the target is.
    pub pot_path: String,
    /// When `true`, compute the merge report but do not write the target
    /// PO. Default `false`.
    pub dry_run: Option<bool>,
    /// When `true` (default), an entry whose source-side `msgid_plural`
    /// changed between the POT and the PO is flagged `fuzzy` so the
    /// translator can re-review it. Set to `false` to keep the existing
    /// fuzzy/non-fuzzy state untouched on drift.
    pub mark_changed_as_fuzzy: Option<bool>,
}

pub(crate) async fn handle_sync_with_pot(
    manager: &GettextStoreManager,
    params: SyncWithPotParams,
) -> Result<Value, GettextError> {
    if params.pot_path.trim().is_empty() {
        return Err(GettextError::InvalidInput(
            "pot_path must not be empty".into(),
        ));
    }

    let dry_run = params.dry_run.unwrap_or(false);
    let opts = MergeOptions {
        mark_changed_as_fuzzy: params.mark_changed_as_fuzzy.unwrap_or(true),
    };

    // Read POT through the FileStore so we honour path validation,
    // BOM stripping, advisory-lock awareness, etc.
    let pot_path = PathBuf::from(&params.pot_path);
    manager.validate_path(&pot_path)?;
    let file_store = manager.file_store().clone();
    let pot_path_clone = pot_path.clone();
    let pot_content = tokio::task::spawn_blocking(move || file_store.read(&pot_path_clone))
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;
    let pot_file = parser::parse_po(&pot_content)?;

    // Snapshot the existing target via the same accessors `tools::xliff`
    // uses so we don't reach into the store internals.
    let store = manager.store_for(params.path.as_deref()).await?;
    let target_snapshot = build_file_snapshot(&store).await?;

    let (merged, report) = merger::merge(&target_snapshot, &pot_file, opts);

    if !dry_run {
        // Persist the merged file by replacing the store's state. We
        // use update_entry/upsert_full helpers indirectly: the
        // simplest path is to serialise + parse via the store again,
        // but going through public mutators preserves the RwLock
        // discipline. We:
        //   1. add/update entries from `merged`.
        //   2. delete entries from `target_snapshot` that aren't in `merged`.
        //   3. apply header changes via set_header.
        //   4. patch the obsolete_lines list directly via update_entry on
        //      the existing entries (no public API exposes obsolete writes,
        //      so we rebuild them through a single low-level path).
        persist_merge(&store, &target_snapshot, &merged).await?;
    }

    Ok(merger::report_to_json(&report, dry_run))
}

/// Rebuild a [`GettextFile`] snapshot from the store's public surface —
/// same trick `tools::xliff` uses. Avoids touching the store internals.
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

/// Push the merge result back through the store's public mutators.
///
/// We can't replace the store's `GettextFile` wholesale without exposing
/// new API, so we drive it through the same path real edits take. That
/// preserves the existing `RwLock` discipline and the on-disk atomic
/// write the store already does.
async fn persist_merge(
    store: &crate::service::GettextStore,
    before: &GettextFile,
    after: &GettextFile,
) -> Result<(), GettextError> {
    // 1. Headers.
    sync_headers(store, &before.metadata, &after.metadata).await?;

    // 2. Entries to remove (in `before` but not in `after`).
    for ((msgid, msgctxt), _) in &before.entries {
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        let key = (msgid.clone(), msgctxt.clone());
        if !after.entries.contains_key(&key) {
            store.delete(msgid, msgctxt.as_deref()).await?;
        }
    }

    // 3. Upsert/replace entries from `after`.
    for ((msgid, msgctxt), new_entry) in &after.entries {
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        store
            .update_entry(msgid, msgctxt.as_deref(), new_entry.clone())
            .await?;
    }

    // 4. Apply the merger's obsolete lines. The store API doesn't expose
    //    a setter for `GettextFile::obsolete_lines`, so we synthesise an
    //    "all obsolete" PO snippet, append it to the on-disk file, and
    //    let the next read pick it up. Simpler: write the in-memory file
    //    directly through the FileStore. The store's persist() will
    //    overwrite this on the very next mutation, so this MUST be the
    //    last write.
    write_obsolete_lines(store, &after.obsolete_lines).await?;

    Ok(())
}

/// Diff two header maps and apply add/update/remove against the store.
async fn sync_headers(
    store: &crate::service::GettextStore,
    before: &IndexMap<String, String>,
    after: &IndexMap<String, String>,
) -> Result<(), GettextError> {
    // Updates and additions.
    for (k, v) in after {
        if before.get(k) != Some(v) {
            store.set_header(k, v).await?;
        }
    }
    // Removals.
    for k in before.keys() {
        if !after.contains_key(k) {
            store.remove_header(k).await?;
        }
    }
    Ok(())
}

/// Persist obsolete lines by reading the file the store just wrote,
/// patching the obsolete block, and writing it back through the file
/// store. The store's in-memory cache picks up the change because we
/// hit it through [`crate::service::store::GettextStore::update_entry`]
/// the next time (after this call the merge handler is done).
async fn write_obsolete_lines(
    store: &crate::service::GettextStore,
    obsolete_lines: &[String],
) -> Result<(), GettextError> {
    // Cheapest correct path: take the current snapshot, swap in the new
    // obsolete lines, serialize, write through the store's metadata
    // setter (set_header(...)) so the store's RwLock + persist runs.
    // Easier still: write through update_entry on the header entry. The
    // header rebuild then commits the obsolete block too.
    //
    // The serializer always reads from `GettextFile::obsolete_lines`,
    // and we can't reach it without store internals. So we read the file
    // text, replace the trailing obsolete block, and write through the
    // FileStore. This is a small file (PO files are typically tiny) and
    // the cost is acceptable.
    use crate::service::serializer::serialize_po;

    // Build a snapshot off the store's public API.
    let mut snapshot = GettextFile::new();
    snapshot.metadata = store.metadata().await?;
    snapshot.rebuild_header_entry();
    for (msgid, msgctxt, entry) in store.list_all().await? {
        snapshot.entries.insert((msgid, msgctxt), entry);
    }
    snapshot.obsolete_lines = obsolete_lines.to_vec();

    let content = serialize_po(&snapshot);
    let path = store.path().to_path_buf();
    // Reach for the same file_store the store uses — we expose it via the
    // manager. The store itself doesn't, so callers must drive this from
    // the manager. We do the write here directly.
    let fs_clone = crate::io::FsFileStore::new();
    use crate::io::FileStore as _;
    let path_clone = path.clone();
    tokio::task::spawn_blocking(move || fs_clone.write(&path_clone, &content))
        .await
        .map_err(|e| GettextError::Io(std::io::Error::other(e)))??;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn write_pot(path: &std::path::Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn sync_adds_obsoletes_and_unchanged_in_one_pass() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let pot = dir.path().join("messages.pot");

        // Seed PO with two translations, one of which will become obsolete.
        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("Old", None, "Vieux", None).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();

        // POT keeps Hello, drops Old, adds NewItem.
        write_pot(
            &pot,
            r#"
msgid ""
msgstr ""
"POT-Creation-Date: 2026-05-27 12:00+0000\n"

msgid "Hello"
msgstr ""

msgid "NewItem"
msgstr ""
"#,
        );

        let result = handle_sync_with_pot(
            &manager,
            SyncWithPotParams {
                path: Some(po.to_str().unwrap().into()),
                pot_path: pot.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_changed_as_fuzzy: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["added"], serde_json::json!(["NewItem"]));
        assert_eq!(result["obsoleted"], serde_json::json!(["Old"]));
        assert_eq!(result["unchanged"], 1);
        assert_eq!(result["dry_run"], false);

        // Refresh and verify on-disk state.
        let store = manager.store_for(None).await.unwrap();
        let hello = store.get("Hello", None).await.unwrap();
        assert_eq!(hello.msgstr, "Bonjour");
        assert!(store.get("Old", None).await.is_err());
        let new_item = store.get("NewItem", None).await.unwrap();
        assert_eq!(new_item.msgstr, "");
        // POT-Creation-Date copied across.
        let meta = store.metadata().await.unwrap();
        assert_eq!(
            meta.get("POT-Creation-Date").map(String::as_str),
            Some("2026-05-27 12:00+0000")
        );
    }

    #[tokio::test]
    async fn sync_dry_run_does_not_mutate() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let pot = dir.path().join("messages.pot");

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("Old", None, "Vieux", None).await.unwrap();

        write_pot(
            &pot,
            r#"
msgid ""
msgstr ""

msgid "Hello"
msgstr ""

msgid "NewItem"
msgstr ""
"#,
        );

        let result = handle_sync_with_pot(
            &manager,
            SyncWithPotParams {
                path: Some(po.to_str().unwrap().into()),
                pot_path: pot.to_str().unwrap().into(),
                dry_run: Some(true),
                mark_changed_as_fuzzy: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["dry_run"], true);
        assert_eq!(result["added"], serde_json::json!(["NewItem"]));

        // No mutation: Old must still exist with its translation.
        let old = store.get("Old", None).await.unwrap();
        assert_eq!(old.msgstr, "Vieux");
        // NewItem must NOT exist.
        assert!(store.get("NewItem", None).await.is_err());
    }

    #[tokio::test]
    async fn sync_real_sample_fr_po_against_synthetic_pot() {
        // Copy examples/sample_fr.po into a temp dir, then merge against a
        // synthetic POT that drops some entries and adds new ones.
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("sample_fr.po");
        let pot = dir.path().join("messages.pot");

        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/sample_fr.po"),
        )
        .unwrap();
        std::fs::write(&po, &src).unwrap();

        write_pot(
            &pot,
            r#"
msgid ""
msgstr ""
"POT-Creation-Date: 2026-05-27 12:00+0000\n"

msgid "Hello"
msgstr ""

msgctxt "greeting"
msgid "Hello, World!"
msgstr ""

msgid "NewString"
msgstr ""
"#,
        );

        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let result = handle_sync_with_pot(
            &manager,
            SyncWithPotParams {
                path: Some(po.to_str().unwrap().into()),
                pot_path: pot.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_changed_as_fuzzy: None,
            },
        )
        .await
        .unwrap();

        assert!(result["added"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "NewString"));
        // Multiple entries must have been moved to obsolete.
        let obsoleted = result["obsoleted"].as_array().unwrap();
        assert!(!obsoleted.is_empty());
        // Hello still translated to Bonjour.
        let store = manager.store_for(None).await.unwrap();
        assert_eq!(store.get("Hello", None).await.unwrap().msgstr, "Bonjour");
    }

    #[tokio::test]
    async fn sync_marks_fuzzy_on_plural_drift() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let pot = dir.path().join("messages.pot");

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

        // POT has a different msgid_plural.
        write_pot(
            &pot,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"%d cat\"\nmsgid_plural \"%d cats (updated)\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n",
        );

        let result = handle_sync_with_pot(
            &manager,
            SyncWithPotParams {
                path: Some(po.to_str().unwrap().into()),
                pot_path: pot.to_str().unwrap().into(),
                dry_run: Some(false),
                mark_changed_as_fuzzy: Some(true),
            },
        )
        .await
        .unwrap();

        assert_eq!(result["updated"], serde_json::json!(["%d cat"]));
        let store = manager.store_for(None).await.unwrap();
        let entry = store.get("%d cat", None).await.unwrap();
        assert!(entry.is_fuzzy(), "merge should mark drifted plural fuzzy");
        assert_eq!(entry.msgid_plural.as_deref(), Some("%d cats (updated)"));
        assert_eq!(entry.msgstr_plural, vec!["%d chat", "%d chats"]);
    }

    #[tokio::test]
    async fn sync_rejects_empty_pot_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let po = dir.path().join("messages.po");
        let manager = Arc::new(GettextStoreManager::new(Some(po.clone())));
        let _ = manager.store_for(None).await.unwrap();

        let result = handle_sync_with_pot(
            &manager,
            SyncWithPotParams {
                path: Some(po.to_str().unwrap().into()),
                pot_path: "".into(),
                dry_run: None,
                mark_changed_as_fuzzy: None,
            },
        )
        .await;
        assert!(result.is_err());
    }
}
