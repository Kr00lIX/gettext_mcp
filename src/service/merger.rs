//! Three-way style merge of a PO file with a POT template — the in-house
//! equivalent of GNU `msgmerge`.
//!
//! Inputs:
//!
//! * `target` — an existing PO file (the translator's working copy).
//! * `source_pot` — a freshly extracted POT (or PO) holding the current set
//!   of msgids known to the source code.
//!
//! Output: a new [`GettextFile`] whose entries are the union, plus a
//! [`MergeReport`] enumerating what changed.
//!
//! Rules implemented (mirror what msgmerge does):
//!
//! * Every `(msgid, msgctxt)` in the POT becomes an entry in the result.
//!   When the same key already exists in `target`, the existing
//!   `msgstr`/`msgstr_plural`/flags/translator-comments are kept; source
//!   locations and developer/extracted comments come from the POT (since
//!   the source code is the authority on where strings live and what they
//!   mean).
//! * If `msgid_plural` differs between target and POT — or one side has
//!   `msgid_plural` set and the other doesn't — and
//!   `mark_changed_as_fuzzy` is true, the entry's `fuzzy` flag is set so
//!   the translator notices the drift. The previous msgid is recorded as
//!   `previous_msgid` for context.
//! * Entries present in `target` but missing from the POT are dropped
//!   from `entries` and moved to `obsolete_lines` (serialized as `#~`
//!   blocks). The translation is preserved verbatim so it can be revived.
//! * The PO header (`("", None)`) is preserved from `target` so we don't
//!   wipe `Language`, `Plural-Forms`, `Last-Translator`, etc. Only
//!   `POT-Creation-Date` is copied across (if present in the POT) — that's
//!   the one header field msgmerge synchronizes too.
//!
//! The function is pure: no I/O, no async, deterministic output for a
//! given (target, source_pot, options) triple.

use crate::model::{GettextFile, MessageEntry};
use crate::service::serializer::escape_po_string;

/// Knobs for [`merge`].
#[derive(Debug, Clone, Copy)]
pub struct MergeOptions {
    /// When a previously translated entry's source-side metadata
    /// (`msgid_plural`) changes, set the `fuzzy` flag so the translator
    /// re-checks it. Mirrors `msgmerge --previous`.
    pub mark_changed_as_fuzzy: bool,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            mark_changed_as_fuzzy: true,
        }
    }
}

/// Lightweight summary of what the merge did, suitable for serializing
/// straight into a tool response.
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    /// `msgid`s that were absent from the target and appeared verbatim
    /// from the POT.
    pub added: Vec<String>,
    /// `msgid`s that existed in the target but had source-side metadata
    /// changes (currently: `msgid_plural` drift).
    pub updated: Vec<String>,
    /// `msgid`s that existed in the target but no longer appear in the
    /// POT. Moved to obsolete lines.
    pub obsoleted: Vec<String>,
    /// Number of entries copied across without any change.
    pub unchanged: usize,
    /// Total number of fuzzy entries in the merged file (header excluded).
    pub fuzzy_count_after: usize,
}

/// Perform the merge described in the module-level docs.
///
/// Returns the merged [`GettextFile`] and a [`MergeReport`].
pub fn merge(
    target: &GettextFile,
    source_pot: &GettextFile,
    opts: MergeOptions,
) -> (GettextFile, MergeReport) {
    let mut merged = GettextFile::new();
    let mut report = MergeReport::default();

    // 1. Carry the target header forward so translator-controlled headers
    //    (Language, Plural-Forms, Last-Translator, ...) survive.
    let header_key = (String::new(), None);
    if let Some(header_entry) = target.entries.get(&header_key) {
        merged
            .entries
            .insert(header_key.clone(), header_entry.clone());
    }
    // Copy every header value the target had, in order, so the result
    // serializes with the same `Key: Value\n` block we started with.
    for (k, v) in &target.metadata {
        merged.metadata.insert(k.clone(), v.clone());
    }
    // POT-Creation-Date is the single header msgmerge syncs from POT.
    if let Some(pot_date) = source_pot.metadata.get("POT-Creation-Date") {
        merged
            .metadata
            .insert("POT-Creation-Date".to_string(), pot_date.clone());
    }
    merged.rebuild_header_entry();

    // 2. For each POT entry, either reuse the target's translation or
    //    seed a fresh untranslated row.
    for ((msgid, msgctxt), pot_entry) in &source_pot.entries {
        // Header in POT is ignored — its msgstr is just template noise.
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }

        let key = (msgid.clone(), msgctxt.clone());
        match target.entries.get(&key) {
            Some(existing) => {
                // Detect source-side drift.
                let plural_changed = existing.msgid_plural != pot_entry.msgid_plural;

                let mut merged_entry = MessageEntry {
                    msgid: msgid.clone(),
                    msgctxt: msgctxt.clone(),
                    // Keep translations.
                    msgstr: existing.msgstr.clone(),
                    msgid_plural: pot_entry.msgid_plural.clone(),
                    msgstr_plural: existing.msgstr_plural.clone(),
                    // Source-side comments come from POT.
                    extracted_comment: pot_entry.extracted_comment.clone(),
                    // Source locations: POT (the source code is the authority).
                    source_locations: pot_entry.source_locations.clone(),
                    // Translator comments stay with the translator's file.
                    translator_comment: existing.translator_comment.clone(),
                    // Keep flags as-is.
                    flags: existing.flags.clone(),
                    previous_msgid: existing.previous_msgid.clone(),
                };

                // If the source-side plural form changed, the translator
                // hasn't seen the new shape. Mark fuzzy + record the
                // previous msgid so the diff is preserved.
                if plural_changed {
                    report.updated.push(msgid.clone());
                    if opts.mark_changed_as_fuzzy
                        && !merged_entry.flags.iter().any(|f| f == "fuzzy")
                    {
                        merged_entry.flags.push("fuzzy".to_string());
                    }
                    // Record previous msgid for the translator's reference.
                    if merged_entry.previous_msgid.is_none() {
                        merged_entry.previous_msgid = Some(msgid.clone());
                    }
                } else {
                    report.unchanged += 1;
                }

                merged.entries.insert(key, merged_entry);
            }
            None => {
                report.added.push(msgid.clone());
                // Brand-new entry from POT: empty msgstr.
                let plural_count = if pot_entry.msgid_plural.is_some() {
                    2
                } else {
                    0
                };
                let new_entry = MessageEntry {
                    msgid: msgid.clone(),
                    msgctxt: msgctxt.clone(),
                    msgstr: String::new(),
                    msgid_plural: pot_entry.msgid_plural.clone(),
                    msgstr_plural: vec![String::new(); plural_count],
                    extracted_comment: pot_entry.extracted_comment.clone(),
                    source_locations: pot_entry.source_locations.clone(),
                    translator_comment: Vec::new(),
                    flags: pot_entry
                        .flags
                        .iter()
                        .filter(|f| *f != "fuzzy")
                        .cloned()
                        .collect(),
                    previous_msgid: None,
                };
                merged.entries.insert(key, new_entry);
            }
        }
    }

    // 3. Anything in target but missing from POT → obsolete. Preserve any
    //    pre-existing obsolete lines from the target as-is.
    merged.obsolete_lines = target.obsolete_lines.clone();
    for ((msgid, msgctxt), entry) in &target.entries {
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        let key = (msgid.clone(), msgctxt.clone());
        if !source_pot.entries.contains_key(&key) {
            report.obsoleted.push(msgid.clone());
            append_obsolete(&mut merged.obsolete_lines, entry);
        }
    }

    // 4. Final fuzzy count (header excluded).
    report.fuzzy_count_after = merged
        .entries
        .iter()
        .filter(|((m, c), _)| !(m.is_empty() && c.is_none()))
        .filter(|(_, e)| e.is_fuzzy())
        .count();

    (merged, report)
}

/// Append a target-only entry to `obsolete_lines` as `#~`-prefixed PO
/// source so it round-trips through the parser/serializer.
fn append_obsolete(obsolete: &mut Vec<String>, entry: &MessageEntry) {
    // Translator comments come first for context.
    for comment in &entry.translator_comment {
        for line in comment.lines() {
            obsolete.push(format!("#~ # {line}"));
        }
    }
    if !entry.flags.is_empty() {
        let flags = entry
            .flags
            .iter()
            .filter(|f| *f != "fuzzy")
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if !flags.is_empty() {
            obsolete.push(format!("#~ #, {flags}"));
        }
    }
    if let Some(ctx) = &entry.msgctxt {
        obsolete.push(format!("#~ msgctxt \"{}\"", escape_po_string(ctx)));
    }
    obsolete.push(format!("#~ msgid \"{}\"", escape_po_string(&entry.msgid)));
    if let Some(plural) = &entry.msgid_plural {
        obsolete.push(format!("#~ msgid_plural \"{}\"", escape_po_string(plural)));
        for (idx, t) in entry.msgstr_plural.iter().enumerate() {
            obsolete.push(format!("#~ msgstr[{idx}] \"{}\"", escape_po_string(t)));
        }
    } else {
        obsolete.push(format!("#~ msgstr \"{}\"", escape_po_string(&entry.msgstr)));
    }
}

/// Convenience: convert a [`MergeReport`] to the JSON shape the MCP tool
/// returns. Lives here so the merger module is the single source of truth
/// for the report wire format.
pub fn report_to_json(report: &MergeReport, dry_run: bool) -> serde_json::Value {
    serde_json::json!({
        "added": report.added,
        "updated": report.updated,
        "obsoleted": report.obsoleted,
        "unchanged": report.unchanged,
        "fuzzy_count_after": report.fuzzy_count_after,
        "dry_run": dry_run,
    })
}

/// Build a `(msgid, msgctxt)` lookup over the merged file, used by tests.
#[cfg(test)]
fn keys(file: &GettextFile) -> std::collections::HashSet<(String, Option<String>)> {
    file.entries.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(msgid: &str, msgstr: &str) -> MessageEntry {
        MessageEntry {
            msgid: msgid.to_string(),
            msgstr: msgstr.to_string(),
            ..Default::default()
        }
    }

    fn insert(file: &mut GettextFile, e: MessageEntry) {
        file.entries.insert((e.msgid.clone(), e.msgctxt.clone()), e);
    }

    #[test]
    fn new_entry_added_with_empty_msgstr() {
        let target = GettextFile::new();
        let mut pot = GettextFile::new();
        insert(&mut pot, entry("Hello", ""));

        let (merged, report) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(report.added, vec!["Hello"]);
        assert_eq!(report.obsoleted.len(), 0);
        assert_eq!(report.unchanged, 0);
        let new = merged.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(new.msgstr, "");
        // Empty msgctxt key + Hello key = 2 entries when header is empty.
        assert!(!merged.entries.is_empty());
    }

    #[test]
    fn removed_entry_becomes_obsolete() {
        let mut target = GettextFile::new();
        insert(&mut target, entry("Old", "Vieux"));
        insert(&mut target, entry("Keep", "Garder"));

        let mut pot = GettextFile::new();
        insert(&mut pot, entry("Keep", ""));

        let (merged, report) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(report.obsoleted, vec!["Old"]);
        assert!(merged.entries.contains_key(&("Keep".to_string(), None)));
        assert!(!merged.entries.contains_key(&("Old".to_string(), None)));
        assert!(merged.obsolete_lines.iter().any(|l| l.contains("Old")));
        assert!(merged.obsolete_lines.iter().any(|l| l.contains("Vieux")));
    }

    #[test]
    fn unchanged_entry_keeps_translation() {
        let mut target = GettextFile::new();
        insert(&mut target, entry("Hello", "Bonjour"));
        let mut pot = GettextFile::new();
        insert(&mut pot, entry("Hello", ""));

        let (merged, report) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(report.unchanged, 1);
        let kept = merged.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(kept.msgstr, "Bonjour");
    }

    #[test]
    fn msgid_plural_change_marks_fuzzy() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "%d file".into(),
                msgstr: "".into(),
                msgid_plural: Some("%d files".into()),
                msgstr_plural: vec!["%d fichier".into(), "%d fichiers".into()],
                ..Default::default()
            },
        );
        let mut pot = GettextFile::new();
        insert(
            &mut pot,
            MessageEntry {
                msgid: "%d file".into(),
                // Source code changed the plural form.
                msgid_plural: Some("%d files (changed)".into()),
                ..Default::default()
            },
        );

        let (merged, report) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(report.updated, vec!["%d file"]);
        let entry = merged.entries.get(&("%d file".to_string(), None)).unwrap();
        assert!(entry.is_fuzzy(), "plural change must mark fuzzy");
        assert_eq!(
            entry.msgid_plural.as_deref(),
            Some("%d files (changed)"),
            "POT side wins for source-side metadata"
        );
        // Translations preserved.
        assert_eq!(entry.msgstr_plural, vec!["%d fichier", "%d fichiers"]);
        assert!(entry.previous_msgid.is_some());
    }

    #[test]
    fn msgid_plural_change_without_fuzzy_flag_when_disabled() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "%d file".into(),
                msgstr: "".into(),
                msgid_plural: Some("%d files".into()),
                msgstr_plural: vec!["%d fichier".into(), "%d fichiers".into()],
                ..Default::default()
            },
        );
        let mut pot = GettextFile::new();
        insert(
            &mut pot,
            MessageEntry {
                msgid: "%d file".into(),
                msgid_plural: Some("%d files (changed)".into()),
                ..Default::default()
            },
        );

        let (merged, report) = merge(
            &target,
            &pot,
            MergeOptions {
                mark_changed_as_fuzzy: false,
            },
        );
        let entry = merged.entries.get(&("%d file".to_string(), None)).unwrap();
        assert!(!entry.is_fuzzy());
        assert_eq!(report.updated, vec!["%d file"]);
    }

    #[test]
    fn msgctxt_disambiguated_entries_handled_independently() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "Open".into(),
                msgctxt: Some("menu".into()),
                msgstr: "Ouvrir".into(),
                ..Default::default()
            },
        );
        insert(
            &mut target,
            MessageEntry {
                msgid: "Open".into(),
                msgctxt: Some("button".into()),
                msgstr: "Lancer".into(),
                ..Default::default()
            },
        );

        let mut pot = GettextFile::new();
        // POT still has the menu version, but the button one was removed.
        insert(
            &mut pot,
            MessageEntry {
                msgid: "Open".into(),
                msgctxt: Some("menu".into()),
                ..Default::default()
            },
        );

        let (merged, report) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.obsoleted, vec!["Open"]);
        let kept = merged
            .entries
            .get(&("Open".to_string(), Some("menu".to_string())))
            .unwrap();
        assert_eq!(kept.msgstr, "Ouvrir");
        assert!(merged.obsolete_lines.iter().any(|l| l.contains("button")));
        assert!(merged.obsolete_lines.iter().any(|l| l.contains("Lancer")));
    }

    #[test]
    fn header_preserved_from_target_pot_date_synced() {
        let mut target = GettextFile::new();
        target.metadata.insert("Language".into(), "fr".into());
        target
            .metadata
            .insert("Plural-Forms".into(), "nplurals=2; plural=(n > 1);".into());
        target
            .metadata
            .insert("POT-Creation-Date".into(), "2025-01-01 00:00+0000".into());
        target.rebuild_header_entry();

        let mut pot = GettextFile::new();
        pot.metadata
            .insert("POT-Creation-Date".into(), "2026-05-27 12:00+0000".into());
        pot.rebuild_header_entry();

        let (merged, _) = merge(&target, &pot, MergeOptions::default());
        assert_eq!(merged.metadata.get("Language").unwrap(), "fr");
        assert_eq!(
            merged.metadata.get("Plural-Forms").unwrap(),
            "nplurals=2; plural=(n > 1);"
        );
        assert_eq!(
            merged.metadata.get("POT-Creation-Date").unwrap(),
            "2026-05-27 12:00+0000"
        );
    }

    #[test]
    fn source_locations_come_from_pot() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                source_locations: vec!["old/file.c:1".into()],
                ..Default::default()
            },
        );
        let mut pot = GettextFile::new();
        insert(
            &mut pot,
            MessageEntry {
                msgid: "Hello".into(),
                source_locations: vec!["src/new.rs:42".into()],
                extracted_comment: vec!["A greeting".into()],
                ..Default::default()
            },
        );

        let (merged, _) = merge(&target, &pot, MergeOptions::default());
        let kept = merged.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(kept.source_locations, vec!["src/new.rs:42"]);
        assert_eq!(kept.extracted_comment, vec!["A greeting"]);
        assert_eq!(kept.msgstr, "Bonjour");
    }

    #[test]
    fn translator_comments_stay_with_target() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                translator_comment: vec!["informal greeting".into()],
                ..Default::default()
            },
        );
        let mut pot = GettextFile::new();
        insert(
            &mut pot,
            MessageEntry {
                msgid: "Hello".into(),
                ..Default::default()
            },
        );

        let (merged, _) = merge(&target, &pot, MergeOptions::default());
        let kept = merged.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(kept.translator_comment, vec!["informal greeting"]);
    }

    #[test]
    fn fuzzy_count_after_reflects_merge() {
        let mut target = GettextFile::new();
        insert(
            &mut target,
            MessageEntry {
                msgid: "A".into(),
                msgstr: "a".into(),
                flags: vec!["fuzzy".into()],
                ..Default::default()
            },
        );
        insert(&mut target, entry("B", "b"));
        let mut pot = GettextFile::new();
        insert(
            &mut pot,
            MessageEntry {
                msgid: "A".into(),
                ..Default::default()
            },
        );
        insert(
            &mut pot,
            MessageEntry {
                msgid: "B".into(),
                ..Default::default()
            },
        );
        insert(
            &mut pot,
            MessageEntry {
                msgid: "C".into(),
                ..Default::default()
            },
        );

        let (_, report) = merge(&target, &pot, MergeOptions::default());
        // A was fuzzy in target and stays fuzzy.
        assert_eq!(report.fuzzy_count_after, 1);
        assert_eq!(report.added, vec!["C"]);
    }

    #[test]
    fn obsolete_entry_roundtrips_through_parser() {
        let mut target = GettextFile::new();
        insert(&mut target, entry("Goes", "Va"));
        let pot = GettextFile::new();

        let (merged, _) = merge(&target, &pot, MergeOptions::default());
        let serialized = crate::service::serializer::serialize_po(&merged);
        let reparsed = crate::service::parser::parse_po(&serialized).unwrap();
        // The active entries are now empty; obsolete lines preserved.
        assert!(
            !reparsed.entries.contains_key(&("Goes".to_string(), None)),
            "obsoleted entry should not be active in merged file"
        );
        assert!(reparsed.obsolete_lines.iter().any(|l| l.contains("Goes")));
        assert!(reparsed.obsolete_lines.iter().any(|l| l.contains("Va")));
    }

    #[test]
    fn keys_helper_returns_expected_set() {
        let mut f = GettextFile::new();
        insert(&mut f, entry("a", "A"));
        insert(&mut f, entry("b", "B"));
        let set = keys(&f);
        assert_eq!(set.len(), 2);
    }
}
