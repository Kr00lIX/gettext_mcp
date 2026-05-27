//! Recursive .po/.pot file discovery tool.
//!
//! Tool: `discover_files`. Independent of the manager's directory-mode
//! scan — walks any directory the caller names, ignoring a small set of
//! commonly-noisy directories (.git, node_modules, target, etc.).

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::service::GettextStoreManager;

const DEFAULT_MAX_DEPTH: usize = 10;
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cargo",
    ".idea",
    ".vscode",
    ".DS_Store",
    "node_modules",
    "target",
    "build",
    "dist",
    "__pycache__",
    ".tox",
    ".venv",
    "venv",
];

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DiscoverFilesParams {
    /// Directory to scan (absolute or relative to the manager's base dir
    /// if configured). Required.
    pub directory: String,
    /// Maximum recursion depth from `directory` (default 10).
    pub max_depth: Option<usize>,
    /// Whether to include `.pot` template files in the result (default
    /// `true`).
    pub include_pot: Option<bool>,
}

pub(crate) async fn handle_discover_files(
    manager: &GettextStoreManager,
    params: DiscoverFilesParams,
) -> Result<Value, GettextError> {
    let dir = PathBuf::from(&params.directory);
    // Reuse the manager's path validator (rejects traversal + enforces
    // base-dir scoping when one is configured).
    manager.validate_path(&dir)?;
    if !dir.exists() {
        return Err(GettextError::InvalidPath(format!(
            "Directory does not exist: {}",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(GettextError::InvalidPath(format!(
            "Not a directory: {}",
            dir.display()
        )));
    }

    let max_depth = params.max_depth.unwrap_or(DEFAULT_MAX_DEPTH);
    let include_pot = params.include_pot.unwrap_or(true);

    let mut po_files: Vec<PathBuf> = Vec::new();
    let mut pot_files: Vec<PathBuf> = Vec::new();
    walk(&dir, 0, max_depth, &mut po_files, &mut pot_files);

    po_files.sort();
    pot_files.sort();

    let po_str: Vec<String> = po_files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let pot_str: Vec<String> = if include_pot {
        pot_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    } else {
        Vec::new()
    };

    let total = po_str.len() + pot_str.len();
    Ok(json!({
        "po_files": po_str,
        "pot_files": pot_str,
        "total": total,
    }))
}

fn walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    po_files: &mut Vec<PathBuf>,
    pot_files: &mut Vec<PathBuf>,
) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden dirs and the explicit denylist.
        if path.is_dir() {
            if name_str.starts_with('.') || SKIPPED_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            if depth < max_depth {
                walk(&path, depth + 1, max_depth, po_files, pot_files);
            }
            continue;
        }

        match path.extension().and_then(|e| e.to_str()) {
            Some("po") => po_files.push(path),
            Some("pot") => pot_files.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn discover_finds_po_and_pot() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();
        std::fs::write(dir.path().join("b.pot"), "msgid \"\"\nmsgstr \"\"\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "hi").unwrap();
        let nested = dir.path().join("locales").join("fr");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("messages.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let manager = Arc::new(GettextStoreManager::new(Some(dir.path().to_path_buf())));
        let result = handle_discover_files(
            &manager,
            DiscoverFilesParams {
                directory: dir.path().to_str().unwrap().into(),
                max_depth: None,
                include_pot: None,
            },
        )
        .await
        .unwrap();

        let po = result["po_files"].as_array().unwrap();
        let pot = result["pot_files"].as_array().unwrap();
        assert_eq!(po.len(), 2);
        assert_eq!(pot.len(), 1);
        assert_eq!(result["total"], 3);
    }

    #[tokio::test]
    async fn discover_skips_hidden_and_denylisted_dirs() {
        let dir = tempfile::TempDir::new().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("inside.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let node = dir.path().join("node_modules");
        std::fs::create_dir_all(&node).unwrap();
        std::fs::write(node.join("inside.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        std::fs::write(dir.path().join("real.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let manager = Arc::new(GettextStoreManager::new(Some(dir.path().to_path_buf())));
        let result = handle_discover_files(
            &manager,
            DiscoverFilesParams {
                directory: dir.path().to_str().unwrap().into(),
                max_depth: None,
                include_pot: None,
            },
        )
        .await
        .unwrap();

        let po = result["po_files"].as_array().unwrap();
        assert_eq!(po.len(), 1);
        assert!(po[0].as_str().unwrap().ends_with("real.po"));
    }

    #[tokio::test]
    async fn discover_include_pot_false_drops_templates() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();
        std::fs::write(dir.path().join("b.pot"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let manager = Arc::new(GettextStoreManager::new(Some(dir.path().to_path_buf())));
        let result = handle_discover_files(
            &manager,
            DiscoverFilesParams {
                directory: dir.path().to_str().unwrap().into(),
                max_depth: None,
                include_pot: Some(false),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["pot_files"].as_array().unwrap().len(), 0);
        assert_eq!(result["po_files"].as_array().unwrap().len(), 1);
        assert_eq!(result["total"], 1);
    }

    #[tokio::test]
    async fn discover_max_depth_bounds_recursion() {
        let dir = tempfile::TempDir::new().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();
        std::fs::write(dir.path().join("top.po"), "msgid \"\"\nmsgstr \"\"\n").unwrap();

        let manager = Arc::new(GettextStoreManager::new(Some(dir.path().to_path_buf())));
        let result = handle_discover_files(
            &manager,
            DiscoverFilesParams {
                directory: dir.path().to_str().unwrap().into(),
                max_depth: Some(1),
                include_pot: None,
            },
        )
        .await
        .unwrap();

        let po = result["po_files"].as_array().unwrap();
        // Depth 1 should not reach a/b/c/deep.po.
        assert_eq!(po.len(), 1);
        assert!(po[0].as_str().unwrap().ends_with("top.po"));
    }

    #[tokio::test]
    async fn discover_errors_when_dir_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = Arc::new(GettextStoreManager::new(Some(dir.path().to_path_buf())));
        let result = handle_discover_files(
            &manager,
            DiscoverFilesParams {
                directory: dir.path().join("nope").to_str().unwrap().into(),
                max_depth: None,
                include_pot: None,
            },
        )
        .await;
        assert!(result.is_err());
    }
}
