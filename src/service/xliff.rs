//! XLIFF 1.2 serialization for gettext PO files.
//!
//! Pure (no I/O) helpers used by [`crate::tools::xliff`]:
//!
//! * [`export_to_xliff`] converts a [`GettextFile`] into an XLIFF 1.2 XML
//!   string, one `<trans-unit>` per non-plural, non-obsolete entry.
//! * [`parse_xliff`] reads an XLIFF 1.2 string and returns a flat
//!   [`ParsedXliff`] view (msgid + optional msgctxt + msgstr per unit).
//!
//! Limitations (documented in the tool descriptions too):
//!
//! * XLIFF 1.2 has no first-class plural model, so PO entries with
//!   `msgid_plural` are skipped during export. The skipped count is
//!   reported back to the caller.
//! * Obsolete (`#~`) entries are not exported.

use std::io::Cursor;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use crate::error::GettextError;
use crate::model::GettextFile;

/// One translation unit parsed out of an XLIFF document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUnit {
    /// Source message id (from `<source>`).
    pub msgid: String,
    /// Optional context lifted from a `<note from="gettext-msgctxt">` note.
    pub msgctxt: Option<String>,
    /// Target string (from `<target>`).
    pub msgstr: String,
}

/// Result of parsing an XLIFF document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedXliff {
    pub source_language: String,
    pub target_language: String,
    pub units: Vec<ParsedUnit>,
}

const XLIFF_NS: &str = "urn:oasis:names:tc:xliff:document:1.2";
const MSGCTXT_NOTE_FROM: &str = "gettext-msgctxt";
const SOURCE_LOC_CONTEXT_TYPE: &str = "sourcefile";

/// Render a [`GettextFile`] as XLIFF 1.2 XML.
///
/// * `target_lang` is the language we're translating *to* (the file the
///   translator will work on).
/// * `source_lang` is the language the msgids are written in. Callers
///   typically derive this from the PO header (`Language` or
///   `X-Source-Language`) and fall back to `"en"`.
/// * When `include_translated` is `false` (the default in the tool), only
///   entries with an empty `msgstr` or a `fuzzy` flag are exported.
///
/// Returns the XML as a string. The header entry (`msgid == ""`), plural
/// entries, and obsolete entries are always skipped; the caller can count
/// what was skipped via [`count_skipped`].
pub fn export_to_xliff(
    file: &GettextFile,
    target_lang: &str,
    source_lang: &str,
    include_translated: bool,
) -> Result<String, GettextError> {
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    write_event(
        &mut writer,
        Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)),
    )?;

    let mut xliff = BytesStart::new("xliff");
    xliff.push_attribute(("version", "1.2"));
    xliff.push_attribute(("xmlns", XLIFF_NS));
    write_event(&mut writer, Event::Start(xliff))?;

    let mut file_elem = BytesStart::new("file");
    file_elem.push_attribute(("source-language", source_lang));
    file_elem.push_attribute(("target-language", target_lang));
    file_elem.push_attribute(("original", "messages.po"));
    file_elem.push_attribute(("datatype", "po"));
    write_event(&mut writer, Event::Start(file_elem))?;

    write_event(&mut writer, Event::Start(BytesStart::new("body")))?;

    for ((msgid, msgctxt), entry) in &file.entries {
        // Skip header entry.
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        // Skip plural entries — XLIFF 1.2 has no clean plural model.
        if entry.msgid_plural.is_some() {
            continue;
        }
        if !include_translated {
            let is_translated = !entry.msgstr.is_empty() && !entry.is_fuzzy();
            if is_translated {
                continue;
            }
        }

        let id = trans_unit_id(msgid, msgctxt.as_deref());
        let state = if entry.msgstr.is_empty() {
            "new"
        } else if entry.is_fuzzy() {
            "needs-review-translation"
        } else {
            "translated"
        };

        write_trans_unit(
            &mut writer,
            &id,
            msgid,
            &entry.msgstr,
            state,
            msgctxt.as_deref(),
            entry,
        )?;
    }

    write_event(&mut writer, Event::End(BytesEnd::new("body")))?;
    write_event(&mut writer, Event::End(BytesEnd::new("file")))?;
    write_event(&mut writer, Event::End(BytesEnd::new("xliff")))?;

    let bytes = writer.into_inner().into_inner();
    String::from_utf8(bytes).map_err(|e| GettextError::InvalidFormat(e.to_string()))
}

/// Counts of entries we'll skip during an export. Kept separate so the
/// tool handler can populate its result payload without re-walking the
/// file.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportCounts {
    pub unit_count: usize,
    pub skipped_plural: usize,
    pub skipped_obsolete: usize,
}

/// Walk the file the same way [`export_to_xliff`] does and tally entries.
pub fn count_skipped(file: &GettextFile, include_translated: bool) -> ExportCounts {
    let mut counts = ExportCounts::default();

    for ((msgid, msgctxt), entry) in &file.entries {
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        if entry.msgid_plural.is_some() {
            counts.skipped_plural += 1;
            continue;
        }
        if !include_translated {
            let is_translated = !entry.msgstr.is_empty() && !entry.is_fuzzy();
            if is_translated {
                continue;
            }
        }
        counts.unit_count += 1;
    }

    counts.skipped_obsolete = file.obsolete_lines.iter().filter(|l| l.contains("msgid")).count();

    counts
}

/// Build a stable, document-unique id for a `<trans-unit>` element from
/// the gettext `(msgid, msgctxt)` pair.
fn trans_unit_id(msgid: &str, msgctxt: Option<&str>) -> String {
    match msgctxt {
        Some(ctx) if !ctx.is_empty() => format!("{ctx}\u{0004}{msgid}"),
        _ => msgid.to_string(),
    }
}

fn write_trans_unit(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    id: &str,
    source: &str,
    target: &str,
    state: &str,
    msgctxt: Option<&str>,
    entry: &crate::model::MessageEntry,
) -> Result<(), GettextError> {
    let mut tu = BytesStart::new("trans-unit");
    tu.push_attribute(("id", id));
    write_event(writer, Event::Start(tu))?;

    // <source>
    write_event(writer, Event::Start(BytesStart::new("source")))?;
    write_event(writer, Event::Text(BytesText::new(source)))?;
    write_event(writer, Event::End(BytesEnd::new("source")))?;

    // <target>
    let mut target_elem = BytesStart::new("target");
    target_elem.push_attribute(("state", state));
    if target.is_empty() {
        write_event(writer, Event::Empty(target_elem))?;
    } else {
        write_event(writer, Event::Start(target_elem))?;
        write_event(writer, Event::Text(BytesText::new(target)))?;
        write_event(writer, Event::End(BytesEnd::new("target")))?;
    }

    // msgctxt becomes a tagged <note>.
    if let Some(ctx) = msgctxt {
        if !ctx.is_empty() {
            let mut note = BytesStart::new("note");
            note.push_attribute(("from", MSGCTXT_NOTE_FROM));
            write_event(writer, Event::Start(note))?;
            write_event(writer, Event::Text(BytesText::new(ctx)))?;
            write_event(writer, Event::End(BytesEnd::new("note")))?;
        }
    }

    // Translator/extracted comments → unannotated <note>.
    let merged_comment = merged_comment(entry);
    if let Some(text) = merged_comment {
        write_event(writer, Event::Start(BytesStart::new("note")))?;
        write_event(writer, Event::Text(BytesText::new(&text)))?;
        write_event(writer, Event::End(BytesEnd::new("note")))?;
    }

    // Source locations → <context-group> with one <context> per location.
    if !entry.source_locations.is_empty() {
        let mut cg = BytesStart::new("context-group");
        cg.push_attribute(("purpose", "location"));
        write_event(writer, Event::Start(cg))?;
        for loc in &entry.source_locations {
            let mut ctx = BytesStart::new("context");
            ctx.push_attribute(("context-type", SOURCE_LOC_CONTEXT_TYPE));
            write_event(writer, Event::Start(ctx))?;
            write_event(writer, Event::Text(BytesText::new(loc)))?;
            write_event(writer, Event::End(BytesEnd::new("context")))?;
        }
        write_event(writer, Event::End(BytesEnd::new("context-group")))?;
    }

    write_event(writer, Event::End(BytesEnd::new("trans-unit")))?;
    Ok(())
}

fn merged_comment(entry: &crate::model::MessageEntry) -> Option<String> {
    let mut buf = String::new();
    for line in &entry.translator_comment {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    for line in &entry.extracted_comment {
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

fn write_event(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    event: Event<'_>,
) -> Result<(), GettextError> {
    writer
        .write_event(event)
        .map_err(|e| GettextError::InvalidFormat(format!("XLIFF write error: {e}")))
}

/// Parse XLIFF 1.2 XML into a flat list of units plus the declared
/// source/target language pair.
///
/// Recognises the `<note from="gettext-msgctxt">` convention emitted by
/// [`export_to_xliff`] to round-trip `msgctxt`. Other notes are ignored.
pub fn parse_xliff(xml: &str) -> Result<ParsedXliff, GettextError> {
    use quick_xml::escape::resolve_xml_entity;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);

    let mut source_language = String::new();
    let mut target_language = String::new();
    let mut units = Vec::new();

    let mut current_id = String::new();
    let mut current_source = String::new();
    let mut current_target = String::new();
    let mut current_msgctxt: Option<String> = None;
    let mut current_note_from: Option<String> = None;

    let mut in_source = false;
    let mut in_target = false;
    let mut in_note = false;
    let mut note_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"file" => {
                    for attr in e.attributes().flatten() {
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        match attr.key.as_ref() {
                            b"source-language" => source_language = value,
                            b"target-language" => target_language = value,
                            _ => {}
                        }
                    }
                }
                b"trans-unit" => {
                    current_id.clear();
                    current_source.clear();
                    current_target.clear();
                    current_msgctxt = None;
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"id" {
                            current_id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
                b"source" => in_source = true,
                b"target" => in_target = true,
                b"note" => {
                    in_note = true;
                    note_buf.clear();
                    current_note_from = None;
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"from" {
                            current_note_from =
                                Some(String::from_utf8_lossy(&attr.value).to_string());
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"file" => {
                    for attr in e.attributes().flatten() {
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        match attr.key.as_ref() {
                            b"source-language" => source_language = value,
                            b"target-language" => target_language = value,
                            _ => {}
                        }
                    }
                }
                b"target" => {
                    // Self-closing target — empty translation.
                }
                _ => {}
            },
            Ok(Event::Text(ref e)) => {
                let text = e
                    .decode()
                    .map_err(|err| GettextError::InvalidFormat(err.to_string()))?;
                if in_source {
                    current_source.push_str(&text);
                } else if in_target {
                    current_target.push_str(&text);
                } else if in_note {
                    note_buf.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(ref e)) => {
                let name = e
                    .decode()
                    .map_err(|err| GettextError::InvalidFormat(err.to_string()))?;
                let resolved = if let Some(s) = resolve_xml_entity(&name) {
                    s.to_owned()
                } else if let Ok(Some(ch)) = e.resolve_char_ref() {
                    ch.to_string()
                } else {
                    return Err(GettextError::InvalidFormat(format!(
                        "unknown XML entity: &{name};"
                    )));
                };
                if in_source {
                    current_source.push_str(&resolved);
                } else if in_target {
                    current_target.push_str(&resolved);
                } else if in_note {
                    note_buf.push_str(&resolved);
                }
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"source" => in_source = false,
                b"target" => in_target = false,
                b"note" => {
                    if current_note_from.as_deref() == Some(MSGCTXT_NOTE_FROM) {
                        current_msgctxt = Some(note_buf.clone());
                    }
                    in_note = false;
                    note_buf.clear();
                    current_note_from = None;
                }
                b"trans-unit" => {
                    // Source defaults to id when source element is absent.
                    let msgid = if current_source.is_empty() {
                        current_id.clone()
                    } else {
                        current_source.clone()
                    };
                    if !msgid.is_empty() {
                        units.push(ParsedUnit {
                            msgid,
                            msgctxt: current_msgctxt.clone(),
                            msgstr: current_target.clone(),
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(GettextError::InvalidFormat(e.to_string())),
            _ => {}
        }
    }

    if target_language.is_empty() {
        return Err(GettextError::InvalidFormat(
            "missing target-language attribute in <file> element".into(),
        ));
    }

    Ok(ParsedXliff {
        source_language,
        target_language,
        units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageEntry;
    use indexmap::IndexMap;

    fn make_file() -> GettextFile {
        let mut f = GettextFile::new();
        // Header.
        f.metadata.insert("Language".into(), "fr".into());
        f.rebuild_header_entry();
        // Three entries: one translated, one fuzzy, one untranslated.
        f.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );
        f.entries.insert(
            ("World".into(), Some("menu".into())),
            MessageEntry {
                msgid: "World".into(),
                msgctxt: Some("menu".into()),
                msgstr: "Monde".into(),
                flags: vec!["fuzzy".into()],
                translator_comment: vec!["needs review".into()],
                ..Default::default()
            },
        );
        f.entries.insert(
            ("Bye".into(), None),
            MessageEntry {
                msgid: "Bye".into(),
                msgstr: String::new(),
                source_locations: vec!["src/main.rs:12".into()],
                ..Default::default()
            },
        );
        f
    }

    #[test]
    fn export_default_skips_translated() {
        let file = make_file();
        let xml = export_to_xliff(&file, "fr", "en", false).unwrap();
        // "Hello"->"Bonjour" is fully translated, should be skipped.
        assert!(!xml.contains(">Hello<"));
        // Fuzzy and empty stay.
        assert!(xml.contains(">World<"));
        assert!(xml.contains(">Bye<"));
    }

    #[test]
    fn export_include_translated_emits_all_non_plural() {
        let file = make_file();
        let xml = export_to_xliff(&file, "fr", "en", true).unwrap();
        assert!(xml.contains(">Hello<"));
        assert!(xml.contains(">World<"));
        assert!(xml.contains(">Bye<"));
    }

    #[test]
    fn export_emits_xml_declaration_and_namespace() {
        let file = make_file();
        let xml = export_to_xliff(&file, "fr", "en", true).unwrap();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("xmlns=\"urn:oasis:names:tc:xliff:document:1.2\""));
        assert!(xml.contains("source-language=\"en\""));
        assert!(xml.contains("target-language=\"fr\""));
    }

    #[test]
    fn export_msgctxt_as_note_round_trips() {
        let file = make_file();
        let xml = export_to_xliff(&file, "fr", "en", true).unwrap();
        assert!(xml.contains("from=\"gettext-msgctxt\""));
        let parsed = parse_xliff(&xml).unwrap();
        let world = parsed
            .units
            .iter()
            .find(|u| u.msgid == "World")
            .expect("World unit");
        assert_eq!(world.msgctxt.as_deref(), Some("menu"));
    }

    #[test]
    fn export_source_locations_emitted_as_context_group() {
        let file = make_file();
        let xml = export_to_xliff(&file, "fr", "en", true).unwrap();
        assert!(xml.contains("<context-group purpose=\"location\">"));
        assert!(xml.contains("src/main.rs:12"));
    }

    #[test]
    fn export_skips_plural_entries() {
        let mut f = GettextFile::new();
        f.entries.insert(
            ("%d cat".into(), None),
            MessageEntry {
                msgid: "%d cat".into(),
                msgid_plural: Some("%d cats".into()),
                msgstr_plural: vec!["%d chat".into(), "%d chats".into()],
                ..Default::default()
            },
        );
        f.entries.insert(
            ("Simple".into(), None),
            MessageEntry {
                msgid: "Simple".into(),
                msgstr: String::new(),
                ..Default::default()
            },
        );

        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        assert!(!xml.contains("%d cat"));
        assert!(xml.contains("Simple"));

        let counts = count_skipped(&f, true);
        assert_eq!(counts.skipped_plural, 1);
        assert_eq!(counts.unit_count, 1);
    }

    #[test]
    fn export_skips_header_entry() {
        let mut f = GettextFile::new();
        f.metadata.insert("Language".into(), "fr".into());
        f.rebuild_header_entry();

        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        // Body should be empty (no trans-units).
        assert!(!xml.contains("<trans-unit"));
    }

    #[test]
    fn export_then_parse_round_trip() {
        let mut f = GettextFile::new();
        f.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );
        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        let parsed = parse_xliff(&xml).unwrap();
        assert_eq!(parsed.source_language, "en");
        assert_eq!(parsed.target_language, "fr");
        assert_eq!(parsed.units.len(), 1);
        assert_eq!(parsed.units[0].msgid, "Hello");
        assert_eq!(parsed.units[0].msgstr, "Bonjour");
        assert_eq!(parsed.units[0].msgctxt, None);
    }

    #[test]
    fn parse_xliff_extracts_source_target_and_units() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="de" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hello">
        <source>Hello</source>
        <target state="translated">Hallo</target>
      </trans-unit>
      <trans-unit id="World">
        <source>World</source>
        <target state="new"></target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        let parsed = parse_xliff(xml).unwrap();
        assert_eq!(parsed.source_language, "en");
        assert_eq!(parsed.target_language, "de");
        assert_eq!(parsed.units.len(), 2);
        assert_eq!(parsed.units[0].msgstr, "Hallo");
        assert_eq!(parsed.units[1].msgstr, "");
    }

    #[test]
    fn parse_xliff_recovers_msgctxt_from_note() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="menu\u{0004}Open">
        <source>Open</source>
        <target state="translated">Ouvrir</target>
        <note from="gettext-msgctxt">menu</note>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        let parsed = parse_xliff(xml).unwrap();
        assert_eq!(parsed.units.len(), 1);
        assert_eq!(parsed.units[0].msgctxt.as_deref(), Some("menu"));
    }

    #[test]
    fn parse_xliff_rejects_missing_target_language() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" original="messages.po" datatype="po">
    <body></body>
  </file>
</xliff>"#;
        let err = parse_xliff(xml).unwrap_err();
        assert!(err.to_string().contains("target-language"));
    }

    #[test]
    fn parse_xliff_escapes_xml_entities() {
        let mut f = GettextFile::new();
        f.entries.insert(
            ("A & B < C".into(), None),
            MessageEntry {
                msgid: "A & B < C".into(),
                msgstr: "X & Y > Z".into(),
                ..Default::default()
            },
        );
        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        assert!(xml.contains("A &amp; B &lt; C"));
        assert!(xml.contains("X &amp; Y &gt; Z"));
        // Round-trip preserves the original content.
        let parsed = parse_xliff(&xml).unwrap();
        assert_eq!(parsed.units[0].msgid, "A & B < C");
        assert_eq!(parsed.units[0].msgstr, "X & Y > Z");
    }

    #[test]
    fn export_preserves_entry_order() {
        let mut f = GettextFile::new();
        let keys = ["alpha", "beta", "gamma", "delta"];
        for k in &keys {
            f.entries.insert(
                ((*k).into(), None),
                MessageEntry {
                    msgid: (*k).into(),
                    msgstr: String::new(),
                    ..Default::default()
                },
            );
        }
        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        let positions: Vec<usize> = keys
            .iter()
            .map(|k| xml.find(&format!("id=\"{k}\"")).unwrap_or(usize::MAX))
            .collect();
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(positions, sorted);
    }

    #[test]
    fn parse_xliff_ignores_other_notes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Hi">
        <source>Hi</source>
        <target state="translated">Salut</target>
        <note>Translator comment</note>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        let parsed = parse_xliff(xml).unwrap();
        assert_eq!(parsed.units.len(), 1);
        assert!(parsed.units[0].msgctxt.is_none());
    }

    #[test]
    fn count_skipped_includes_obsolete_lines_with_msgid() {
        let mut f = GettextFile::new();
        f.obsolete_lines = vec![
            "#~ msgid \"Old\"".into(),
            "#~ msgstr \"Ancien\"".into(),
        ];
        f.entries.insert(
            ("Live".into(), None),
            MessageEntry {
                msgid: "Live".into(),
                msgstr: String::new(),
                ..Default::default()
            },
        );
        let counts = count_skipped(&f, true);
        assert_eq!(counts.unit_count, 1);
        assert_eq!(counts.skipped_obsolete, 1);
    }

    #[test]
    fn build_trans_unit_id_includes_context() {
        assert_eq!(trans_unit_id("Open", None), "Open");
        assert_eq!(trans_unit_id("Open", Some("")), "Open");
        let id = trans_unit_id("Open", Some("menu"));
        assert!(id.starts_with("menu"));
        assert!(id.ends_with("Open"));
    }

    #[test]
    fn parse_handles_entries_without_explicit_source() {
        // Some XLIFF authoring tools omit the inner <source> element and
        // rely on `id` to carry the msgid. Make sure we cope.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff version="1.2" xmlns="urn:oasis:names:tc:xliff:document:1.2">
  <file source-language="en" target-language="fr" original="messages.po" datatype="po">
    <body>
      <trans-unit id="Cancel">
        <target state="translated">Annuler</target>
      </trans-unit>
    </body>
  </file>
</xliff>"#;
        let parsed = parse_xliff(xml).unwrap();
        assert_eq!(parsed.units.len(), 1);
        assert_eq!(parsed.units[0].msgid, "Cancel");
        assert_eq!(parsed.units[0].msgstr, "Annuler");
    }

    #[test]
    fn export_writes_state_attribute_for_each_target() {
        let mut f = GettextFile::new();
        f.entries.insert(
            ("a".into(), None),
            MessageEntry {
                msgid: "a".into(),
                msgstr: "A".into(),
                ..Default::default()
            },
        );
        f.entries.insert(
            ("b".into(), None),
            MessageEntry {
                msgid: "b".into(),
                msgstr: "B".into(),
                flags: vec!["fuzzy".into()],
                ..Default::default()
            },
        );
        f.entries.insert(
            ("c".into(), None),
            MessageEntry {
                msgid: "c".into(),
                msgstr: String::new(),
                ..Default::default()
            },
        );
        let xml = export_to_xliff(&f, "fr", "en", true).unwrap();
        assert!(xml.contains("state=\"translated\""));
        assert!(xml.contains("state=\"needs-review-translation\""));
        assert!(xml.contains("state=\"new\""));
    }

    // Silence the `IndexMap` import-only-via-features warning when the
    // module compiles in isolation during cargo check.
    #[allow(dead_code)]
    fn _ensure_indexmap_in_scope() -> IndexMap<String, String> {
        IndexMap::new()
    }
}
