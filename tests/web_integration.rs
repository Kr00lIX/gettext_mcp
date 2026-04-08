use gettext_mcp::{GettextStoreManager, WebConfig};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::test]
async fn test_web_config_creation() {
    let manager = Arc::new(GettextStoreManager::new(None));
    let addr: SocketAddr = "127.0.0.1:8787".parse().unwrap();

    let config = WebConfig {
        addr,
        manager,
    };

    assert_eq!(config.addr.port(), 8787);
}

#[tokio::test]
async fn test_store_manager_discovered_paths() {
    let manager = GettextStoreManager::new(None);

    let paths = manager.discovered_paths().await;
    assert_eq!(paths.len(), 0, "New manager should have no paths");
}

#[tokio::test]
async fn test_store_list_languages() {
    use tempfile::NamedTempFile;

    // Create a temporary PO file
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path.clone())));
    let store = manager.store_for(None).await.unwrap();

    // Set a language
    store.add_language("en").await.unwrap();

    // Verify languages can be listed
    let languages = store.list_languages().await.unwrap();
    assert_eq!(languages.len(), 1);
    assert_eq!(languages[0], "en");
}

#[tokio::test]
async fn test_store_upsert_full() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Test upsert_full with plural forms
    store.upsert_full(
        "apples",
        None,
        "1 pomme",
        Some("apples"),
        Some(vec!["1 pomme".to_string(), "%d pommes".to_string()]),
        None,
    ).await.unwrap();

    // Verify it was stored
    let entry = store.get("apples", None).await.unwrap();
    assert_eq!(entry.msgstr, "1 pomme");
    assert_eq!(entry.msgid_plural, Some("apples".to_string()));
    assert_eq!(entry.msgstr_plural.len(), 2);
}

#[tokio::test]
async fn test_store_get_metadata() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Set some metadata
    store.set_header("Language", "fr").await.unwrap();
    store.set_header("Content-Type", "text/plain; charset=UTF-8").await.unwrap();

    // Retrieve metadata
    let metadata = store.metadata().await.unwrap();
    assert_eq!(metadata.get("Language"), Some(&"fr".to_string()));
    assert_eq!(metadata.get("Content-Type"), Some(&"text/plain; charset=UTF-8".to_string()));
}

#[tokio::test]
async fn test_store_remove_language() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Add then remove a language
    store.add_language("es").await.unwrap();
    let languages_before = store.list_languages().await.unwrap();
    assert_eq!(languages_before.len(), 1);

    store.remove_language("es").await.unwrap();
    let languages_after = store.list_languages().await.unwrap();
    assert_eq!(languages_after.len(), 0);
}

#[tokio::test]
async fn test_store_concurrent_access() {
    use tempfile::NamedTempFile;
    use tokio::task;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Spawn multiple tasks writing concurrently
    let mut handles = vec![];

    for i in 0..5 {
        let store_clone = Arc::clone(&store);
        let handle = task::spawn(async move {
            let msgid = format!("message_{}", i);
            let msgstr = format!("translation_{}", i);
            store_clone.upsert(&msgid, None, &msgstr, None)
                .await.unwrap();
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all messages are present with correct values
    let all_entries = store.list_all().await.unwrap();
    assert_eq!(all_entries.len(), 5);

    // Verify each entry has the correct translation
    for i in 0..5 {
        let expected_msgid = format!("message_{}", i);
        let expected_msgstr = format!("translation_{}", i);
        let entry = store.get(&expected_msgid, None).await.unwrap();
        assert_eq!(entry.msgstr, expected_msgstr, "Entry {} has wrong translation", i);
    }
}

#[tokio::test]
async fn test_store_round_trip() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path.clone())));
    let store1 = manager.store_for(None).await.unwrap();

    // Add multiple test entries
    store1.upsert("Hello", None, "Bonjour", None).await.unwrap();
    store1.upsert("World", None, "Monde", None).await.unwrap();
    store1.upsert("Goodbye", Some("context"), "Au revoir", None).await.unwrap();

    // Explicitly drop to force flush
    drop(store1);

    // Reload from disk using a new manager
    let manager2 = Arc::new(GettextStoreManager::new(Some(path)));
    let store2 = manager2.store_for(None).await.unwrap();

    // Verify all entries persisted correctly
    let hello = store2.get("Hello", None).await.unwrap();
    assert_eq!(hello.msgstr, "Bonjour");

    let world = store2.get("World", None).await.unwrap();
    assert_eq!(world.msgstr, "Monde");

    let goodbye = store2.get("Goodbye", Some("context")).await.unwrap();
    assert_eq!(goodbye.msgstr, "Au revoir");

    // Verify we can get all entries
    let all = store2.list_all().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_store_plural_forms() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Add entry with plurals
    store.upsert_full(
        "files",
        None,
        "1 fichier",
        Some("files"),
        Some(vec!["1 fichier".to_string(), "%d fichiers".to_string()]),
        None,
    ).await.unwrap();

    // Verify plural forms stored correctly
    let entry = store.get("files", None).await.unwrap();
    assert_eq!(entry.msgid_plural, Some("files".to_string()));
    assert_eq!(entry.msgstr_plural.len(), 2);
    assert_eq!(entry.msgstr_plural[1], "%d fichiers");
}

#[tokio::test]
async fn test_store_context_handling() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();

    let manager = Arc::new(GettextStoreManager::new(Some(path)));
    let store = manager.store_for(None).await.unwrap();

    // Add entries with same msgid but different contexts
    store.upsert("Save", Some("menu"), "Enregistrer", None).await.unwrap();
    store.upsert("Save", Some("button"), "Sauvegarder", None).await.unwrap();

    // Verify both are stored separately
    let entry1 = store.get("Save", Some("menu")).await.unwrap();
    let entry2 = store.get("Save", Some("button")).await.unwrap();

    assert_eq!(entry1.msgstr, "Enregistrer");
    assert_eq!(entry2.msgstr, "Sauvegarder");
}
