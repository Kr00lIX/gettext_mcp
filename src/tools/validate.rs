//! Translation validation tool.
//!
//! Tool: `validate_translations`. Walks every entry in a PO file and
//! emits a list of findings classified as `error`, `warning`, or `info`.
//!
//! Checks performed:
//!
//! * **format_specifier_mismatch** (error) — printf-style (`%s`, `%lld`,
//!   `%1$@`) or brace-style (`{0}`, `{name}`) specifiers must match
//!   between msgid and msgstr as a multiset. Plural msgstr forms are
//!   compared against `msgid_plural`'s specifier set.
//! * **plural_form_count_mismatch** (error) — when `msgid_plural` is set
//!   the number of `msgstr[n]` entries must equal `nplurals` declared in
//!   the `Plural-Forms` header.
//! * **empty_translation_fuzzy** (warning) — a fuzzy entry with no
//!   translation is half-done work that needs attention.
//! * **empty_translation** (info) — plain untranslated entry.
//! * **identical_translation** (info) — `msgstr == msgid`, often a
//!   placeholder left over from extraction.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::GettextError;
use crate::model::MessageEntry;
use crate::service::GettextStoreManager;

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct ValidateTranslationsParams {
    /// Path to the .po file (required in directory/dynamic mode).
    pub path: Option<String>,
    /// Restrict findings to a single severity: `"error"`, `"warning"`,
    /// or `"info"`. `None` returns all severities.
    pub severity_filter: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

struct Finding {
    msgid: String,
    msgctxt: Option<String>,
    kind: &'static str,
    detail: String,
    severity: Severity,
}

impl Finding {
    fn to_json(&self) -> Value {
        json!({
            "msgid": self.msgid,
            "msgctxt": self.msgctxt,
            "kind": self.kind,
            "detail": self.detail,
            "severity": self.severity.as_str(),
        })
    }
}

pub(crate) async fn handle_validate_translations(
    manager: &GettextStoreManager,
    params: ValidateTranslationsParams,
) -> Result<Value, GettextError> {
    let store = manager.store_for(params.path.as_deref()).await?;
    let metadata = store.metadata().await?;
    let nplurals = parse_nplurals(metadata.get("Plural-Forms").map(String::as_str));
    let entries = store.list_all().await?;

    let mut findings: Vec<Finding> = Vec::new();
    for (msgid, msgctxt, entry) in &entries {
        check_entry(msgid, msgctxt.as_deref(), entry, nplurals, &mut findings);
    }

    // Optional severity filter.
    if let Some(filter) = params.severity_filter.as_deref() {
        let want = match filter.to_ascii_lowercase().as_str() {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "info" => Some(Severity::Info),
            _ => {
                return Err(GettextError::InvalidInput(format!(
                    "Invalid severity_filter '{filter}': expected error/warning/info"
                )))
            }
        };
        if let Some(want) = want {
            findings.retain(|f| f.severity == want);
        }
    }

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut infos = Vec::new();
    for f in &findings {
        match f.severity {
            Severity::Error => errors.push(f.to_json()),
            Severity::Warning => warnings.push(f.to_json()),
            Severity::Info => infos.push(f.to_json()),
        }
    }

    let summary = json!({
        "errors": errors.len(),
        "warnings": warnings.len(),
        "infos": infos.len(),
    });

    Ok(json!({
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "summary": summary,
    }))
}

fn check_entry(
    msgid: &str,
    msgctxt: Option<&str>,
    entry: &MessageEntry,
    nplurals: Option<usize>,
    findings: &mut Vec<Finding>,
) {
    // 1) Format specifier mismatch.
    if entry.msgid_plural.is_some() {
        let id_plural = entry.msgid_plural.as_deref().unwrap_or("");
        // For plural entries: msgstr_plural[0] is compared against msgid
        // (singular), the rest against msgid_plural. This matches gettext
        // semantics (n=1 -> form[0], otherwise -> later forms).
        for (i, msgstr) in entry.msgstr_plural.iter().enumerate() {
            if msgstr.is_empty() {
                continue;
            }
            let source = if i == 0 { msgid } else { id_plural };
            if let Some(detail) = compare_specifiers(source, msgstr) {
                findings.push(Finding {
                    msgid: msgid.to_string(),
                    msgctxt: msgctxt.map(String::from),
                    kind: "format_specifier_mismatch",
                    detail: format!("plural form {i}: {detail}"),
                    severity: Severity::Error,
                });
            }
        }
    } else if !entry.msgstr.is_empty() {
        if let Some(detail) = compare_specifiers(msgid, &entry.msgstr) {
            findings.push(Finding {
                msgid: msgid.to_string(),
                msgctxt: msgctxt.map(String::from),
                kind: "format_specifier_mismatch",
                detail,
                severity: Severity::Error,
            });
        }
    }

    // 2) Plural form count vs nplurals header.
    if entry.msgid_plural.is_some() {
        if let Some(expected) = nplurals {
            let actual = entry.msgstr_plural.len();
            if actual != expected {
                findings.push(Finding {
                    msgid: msgid.to_string(),
                    msgctxt: msgctxt.map(String::from),
                    kind: "plural_form_count_mismatch",
                    detail: format!(
                        "header declares nplurals={expected} but entry has {actual} msgstr_plural entries"
                    ),
                    severity: Severity::Error,
                });
            }
        }
    }

    // 3/4) Empty translation states.
    let empty = if entry.msgid_plural.is_some() {
        entry.msgstr_plural.is_empty() || entry.msgstr_plural.iter().any(|s| s.is_empty())
    } else {
        entry.msgstr.is_empty()
    };
    if empty {
        if entry.is_fuzzy() {
            findings.push(Finding {
                msgid: msgid.to_string(),
                msgctxt: msgctxt.map(String::from),
                kind: "empty_translation_fuzzy",
                detail: "fuzzy entry has an empty translation".into(),
                severity: Severity::Warning,
            });
        } else {
            findings.push(Finding {
                msgid: msgid.to_string(),
                msgctxt: msgctxt.map(String::from),
                kind: "empty_translation",
                detail: "msgstr is empty".into(),
                severity: Severity::Info,
            });
        }
    }

    // 5) msgstr identical to msgid (likely placeholder).
    if !msgid.is_empty() && !entry.msgstr.is_empty() && entry.msgstr == msgid {
        findings.push(Finding {
            msgid: msgid.to_string(),
            msgctxt: msgctxt.map(String::from),
            kind: "identical_translation",
            detail: "msgstr is identical to msgid".into(),
            severity: Severity::Info,
        });
    }
}

/// Compare the format specifiers in `source` and `target` as multisets.
/// Returns `None` when the sets are equal, otherwise a human-readable
/// description of the mismatch.
fn compare_specifiers(source: &str, target: &str) -> Option<String> {
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
        "msgid specifiers {src:?} differ from msgstr specifiers {dst:?}"
    ))
}

/// Extract printf-style and brace-style format specifiers from a string.
///
/// Recognises:
///
/// * `%s`, `%d`, `%lld`, `%@`, `%.2f`, `%1$@`, etc. — C/printf and
///   Objective-C `%@`. Length modifiers (`l`, `ll`, `h`, `hh`, `z`,
///   `t`) and width/precision (`*` or digits) are accepted but only
///   the conversion character is significant for matching.
/// * `{}`, `{0}`, `{1}`, `{name}` — Python/Rust/.NET-style braces.
///
/// Returns the raw substrings (e.g. `"%lld"`, `"{name}"`); two strings
/// are compatible when their multisets of returned values match.
fn extract_specifiers(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            // `%%` literal — skip both.
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

/// Parse a `%...` printf-style specifier starting at `bytes[0]`. Returns
/// `(raw, consumed_bytes)`. Caller already verified `bytes[0] == b'%'`.
fn parse_percent(bytes: &[u8]) -> Option<(String, usize)> {
    // Pattern: % (digits$)? [-+ 0#']* (digits | *)? (.digits | .*)? (length-mod)? conv-char
    let mut idx = 1;

    // Optional positional argument: `1$`, `2$` …
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
    // Length modifier (greedy, up to two chars).
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
    // Conversion character.
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
    // The raw substring is bytes[0..idx], but we only need ASCII chars here,
    // so direct UTF-8 conversion is fine because all bytes consumed are ASCII.
    let raw = std::str::from_utf8(&bytes[..idx]).ok()?.to_string();
    Some((raw, idx))
}

/// Parse a `{...}` brace-style specifier. Returns `(raw, consumed)`.
fn parse_brace(bytes: &[u8]) -> Option<(String, usize)> {
    // Find matching `}`. Specifier name is allowed to contain alphanumerics,
    // underscores, dots, and digits. Bail out on whitespace or empty content.
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

/// Extract `nplurals=N` from a `Plural-Forms` header.
fn parse_nplurals(plural_forms: Option<&str>) -> Option<usize> {
    let pf = plural_forms?;
    let needle = "nplurals=";
    let start = pf.find(needle)? + needle.len();
    let tail = &pf[start..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    if end == 0 {
        return None;
    }
    tail[..end].parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn make_manager(path: &std::path::Path) -> Arc<GettextStoreManager> {
        Arc::new(GettextStoreManager::new(Some(path.to_path_buf())))
    }

    #[test]
    fn extract_percent_basic() {
        let s = extract_specifiers("Hello %s, you have %d items");
        assert_eq!(s, vec!["%s", "%d"]);
    }

    #[test]
    fn extract_percent_ll_and_at() {
        let s = extract_specifiers("%1$@ has %2$lld items");
        assert_eq!(s, vec!["%1$@", "%2$lld"]);
    }

    #[test]
    fn extract_brace_specifiers() {
        let s = extract_specifiers("Hello {0}, you have {name} {1} items");
        assert_eq!(s, vec!["{0}", "{name}", "{1}"]);
    }

    #[test]
    fn extract_ignores_percent_percent() {
        let s = extract_specifiers("100%% complete");
        assert!(s.is_empty());
    }

    #[test]
    fn extract_handles_precision() {
        let s = extract_specifiers("Pi = %.2f");
        assert_eq!(s, vec!["%.2f"]);
    }

    #[tokio::test]
    async fn validate_detects_format_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Hello %s", None, "Bonjour %d", None)
            .await
            .unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(result["summary"]["errors"], 1);
        assert_eq!(result["errors"][0]["kind"], "format_specifier_mismatch");
    }

    #[tokio::test]
    async fn validate_format_match_no_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Hello %s, %d items", None, "%d éléments pour %s", None)
            .await
            .unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: None,
            },
        )
        .await
        .unwrap();
        // multiset {%s, %d} == {%d, %s} so no format error.
        assert_eq!(result["summary"]["errors"], 0);
    }

    #[tokio::test]
    async fn validate_plural_form_count_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .set_header("Plural-Forms", "nplurals=3; plural=...;")
            .await
            .unwrap();
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

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: None,
            },
        )
        .await
        .unwrap();

        let kinds: Vec<&str> = result["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"plural_form_count_mismatch"));
    }

    #[tokio::test]
    async fn validate_empty_fuzzy_is_warning() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Hi", None, "", Some(vec!["fuzzy".into()]))
            .await
            .unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["summary"]["warnings"], 1);
        assert_eq!(result["warnings"][0]["kind"], "empty_translation_fuzzy");
    }

    #[tokio::test]
    async fn validate_identical_translation_is_info() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store.upsert("Cancel", None, "Cancel", None).await.unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: None,
            },
        )
        .await
        .unwrap();
        let kinds: Vec<&str> = result["infos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"identical_translation"));
    }

    #[tokio::test]
    async fn validate_severity_filter_restricts_output() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let store = manager.store_for(None).await.unwrap();
        store
            .upsert("Hello %s", None, "Bonjour %d", None)
            .await
            .unwrap();
        store.upsert("Cancel", None, "Cancel", None).await.unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: Some("error".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["summary"]["infos"], 0);
        assert_eq!(result["summary"]["warnings"], 0);
        assert!(result["summary"]["errors"].as_u64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn validate_unknown_severity_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");
        let manager = make_manager(&path).await;
        let _ = manager.store_for(None).await.unwrap();

        let result = handle_validate_translations(
            &manager,
            ValidateTranslationsParams {
                path: Some(path.to_str().unwrap().into()),
                severity_filter: Some("critical".into()),
            },
        )
        .await;
        assert!(result.is_err());
    }
}
