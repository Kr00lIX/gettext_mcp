//! Per-locale-pair glossary store backed by a JSON file.
//!
//! A glossary holds preferred translations for individual terms, keyed by
//! a `"source\u{2192}target"` locale pair (e.g. `"en→fr"`). Tools use it
//! to enforce consistent terminology across `.po`/`.pot` files: an LLM
//! translator should look up `Settings` in the glossary before inventing
//! a new translation.
//!
//! This module is pure (no I/O); the [`crate::tools::glossary`] module
//! handles reading/writing the JSON file through the
//! [`crate::io::FileStore`] abstraction.

use std::collections::BTreeMap;

use crate::error::GettextError;

/// Glossary data shape: outer key is a `"source\u{2192}target"` locale
/// pair (see [`locale_pair_key`]); inner map is term → translation.
pub type Glossary = BTreeMap<String, BTreeMap<String, String>>;

/// Build the outer-map key for a locale pair using the U+2192 RIGHTWARDS
/// ARROW (e.g. `"en→fr"`). The arrow keeps the key unambiguous even when
/// language tags contain hyphens (`"en-US→pt-BR"`).
pub(crate) fn locale_pair_key(source: &str, target: &str) -> String {
    format!("{source}\u{2192}{target}")
}

/// Deserialize a glossary from a raw JSON string. `None` (i.e. file
/// missing on disk) yields an empty glossary rather than an error.
pub fn parse_glossary(raw: Option<&str>) -> Result<Glossary, GettextError> {
    match raw {
        Some(json) => {
            let trimmed = json.trim();
            if trimmed.is_empty() {
                return Ok(Glossary::new());
            }
            serde_json::from_str(json)
                .map_err(|e| GettextError::InvalidFormat(format!("glossary JSON: {e}")))
        }
        None => Ok(Glossary::new()),
    }
}

/// Serialize a glossary to pretty-printed JSON.
pub fn serialize_glossary(glossary: &Glossary) -> Result<String, GettextError> {
    serde_json::to_string_pretty(glossary)
        .map_err(|e| GettextError::InvalidFormat(format!("glossary JSON: {e}")))
}

/// Return all entries for a locale pair, optionally filtered by a
/// case-insensitive substring that matches either the source term or the
/// translation.
pub fn get_entries(
    glossary: &Glossary,
    source_locale: &str,
    target_locale: &str,
    filter: Option<&str>,
) -> BTreeMap<String, String> {
    let key = locale_pair_key(source_locale, target_locale);
    let Some(entries) = glossary.get(&key) else {
        return BTreeMap::new();
    };
    match filter {
        Some(f) => {
            let f_lower = f.to_lowercase();
            entries
                .iter()
                .filter(|(k, v)| {
                    k.to_lowercase().contains(&f_lower) || v.to_lowercase().contains(&f_lower)
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }
        None => entries.clone(),
    }
}

/// Upsert `entries` into the glossary for `(source_locale, target_locale)`.
/// Returns the number of terms that were inserted or overwritten (equal
/// to `entries.len()`).
pub fn update_entries(
    glossary: &mut Glossary,
    source_locale: &str,
    target_locale: &str,
    entries: BTreeMap<String, String>,
) -> usize {
    let key = locale_pair_key(source_locale, target_locale);
    let pair = glossary.entry(key).or_default();
    let count = entries.len();
    for (term, translation) in entries {
        pair.insert(term, translation);
    }
    count
}

/// Remove the listed `terms` from the glossary entry for
/// `(source_locale, target_locale)`. Returns the number of terms that
/// were actually present (and therefore removed). When the locale pair
/// ends up empty after removal, the outer entry is cleaned up too.
pub fn delete_entries(
    glossary: &mut Glossary,
    source_locale: &str,
    target_locale: &str,
    terms: &[String],
) -> usize {
    let key = locale_pair_key(source_locale, target_locale);
    let Some(pair) = glossary.get_mut(&key) else {
        return 0;
    };
    let mut removed = 0usize;
    for term in terms {
        if pair.remove(term).is_some() {
            removed += 1;
        }
    }
    if pair.is_empty() {
        glossary.remove(&key);
    }
    removed
}

/// Count of entries for the given locale pair (zero when the pair is
/// absent). Convenience wrapper used by tool result payloads.
pub fn pair_total(glossary: &Glossary, source_locale: &str, target_locale: &str) -> usize {
    let key = locale_pair_key(source_locale, target_locale);
    glossary.get(&key).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_pair_key_uses_arrow() {
        assert_eq!(locale_pair_key("en", "fr"), "en\u{2192}fr");
        assert_eq!(locale_pair_key("en-US", "pt-BR"), "en-US\u{2192}pt-BR");
    }

    #[test]
    fn parse_none_yields_empty_glossary() {
        let g = parse_glossary(None).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn parse_empty_string_yields_empty_glossary() {
        let g = parse_glossary(Some("")).unwrap();
        assert!(g.is_empty());
        let g = parse_glossary(Some("   \n")).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn serialize_then_parse_roundtrip() {
        let mut g = Glossary::new();
        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Paramètres".into());
        entries.insert("Cancel".into(), "Annuler".into());
        update_entries(&mut g, "en", "fr", entries);

        let json = serialize_glossary(&g).unwrap();
        let reloaded = parse_glossary(Some(&json)).unwrap();
        let out = get_entries(&reloaded, "en", "fr", None);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("Settings").unwrap(), "Paramètres");
        assert_eq!(out.get("Cancel").unwrap(), "Annuler");
    }

    #[test]
    fn filter_matches_term_or_translation() {
        let mut g = Glossary::new();
        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Einstellungen".into());
        entries.insert("Cancel".into(), "Abbrechen".into());
        entries.insert("Save".into(), "Speichern".into());
        update_entries(&mut g, "en", "de", entries);

        // Match by term.
        let out = get_entries(&g, "en", "de", Some("set"));
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("Settings"));

        // Match by translation (case-insensitive).
        let out = get_entries(&g, "en", "de", Some("ABBRECH"));
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("Cancel"));

        // No match.
        let out = get_entries(&g, "en", "de", Some("zzz"));
        assert!(out.is_empty());
    }

    #[test]
    fn update_overwrites_existing_entry() {
        let mut g = Glossary::new();
        let mut e1 = BTreeMap::new();
        e1.insert("Settings".into(), "Old".into());
        update_entries(&mut g, "en", "fr", e1);

        let mut e2 = BTreeMap::new();
        e2.insert("Settings".into(), "New".into());
        update_entries(&mut g, "en", "fr", e2);

        let out = get_entries(&g, "en", "fr", None);
        assert_eq!(out.get("Settings").unwrap(), "New");
    }

    #[test]
    fn delete_removes_matching_terms_only() {
        let mut g = Glossary::new();
        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Paramètres".into());
        entries.insert("Cancel".into(), "Annuler".into());
        update_entries(&mut g, "en", "fr", entries);

        let n = delete_entries(
            &mut g,
            "en",
            "fr",
            &["Settings".into(), "Nonexistent".into()],
        );
        assert_eq!(n, 1);
        let out = get_entries(&g, "en", "fr", None);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("Cancel"));
    }

    #[test]
    fn delete_clears_empty_pair() {
        let mut g = Glossary::new();
        let mut entries = BTreeMap::new();
        entries.insert("Only".into(), "Seul".into());
        update_entries(&mut g, "en", "fr", entries);

        delete_entries(&mut g, "en", "fr", &["Only".into()]);
        assert!(g.is_empty(), "outer map should drop empty pair");
    }

    #[test]
    fn multiple_locale_pairs_stay_isolated() {
        let mut g = Glossary::new();
        let mut en_fr = BTreeMap::new();
        en_fr.insert("Settings".into(), "Paramètres".into());
        update_entries(&mut g, "en", "fr", en_fr);

        let mut en_de = BTreeMap::new();
        en_de.insert("Settings".into(), "Einstellungen".into());
        update_entries(&mut g, "en", "de", en_de);

        let fr = get_entries(&g, "en", "fr", None);
        let de = get_entries(&g, "en", "de", None);
        assert_eq!(fr.get("Settings").unwrap(), "Paramètres");
        assert_eq!(de.get("Settings").unwrap(), "Einstellungen");

        // Deleting from one pair must not touch the other.
        delete_entries(&mut g, "en", "fr", &["Settings".into()]);
        let de_again = get_entries(&g, "en", "de", None);
        assert_eq!(de_again.get("Settings").unwrap(), "Einstellungen");
    }

    #[test]
    fn corrupt_json_surfaces_error() {
        let res = parse_glossary(Some("{not json"));
        assert!(res.is_err(), "expected parse error for corrupt JSON");
        let err = res.unwrap_err();
        match err {
            GettextError::InvalidFormat(msg) => assert!(msg.contains("glossary")),
            other => panic!("expected InvalidFormat, got: {other:?}"),
        }
    }

    #[test]
    fn pair_total_counts_entries() {
        let mut g = Glossary::new();
        assert_eq!(pair_total(&g, "en", "fr"), 0);
        let mut entries = BTreeMap::new();
        entries.insert("Settings".into(), "Paramètres".into());
        entries.insert("Cancel".into(), "Annuler".into());
        update_entries(&mut g, "en", "fr", entries);
        assert_eq!(pair_total(&g, "en", "fr"), 2);
        assert_eq!(pair_total(&g, "en", "de"), 0);
    }
}
