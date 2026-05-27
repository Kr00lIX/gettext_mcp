//! Serialize a [`GettextFile`] back into PO source text.
//!
//! Pure string-out logic; no I/O. The output must round-trip cleanly
//! through [`crate::service::parser::parse_po`] (and that property is
//! exercised in tests).

use crate::model::GettextFile;

/// Escape a string for use inside a PO `"..."` literal.
pub fn escape_po_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c => result.push(c),
        }
    }
    result
}

/// Emit `keyword "value"` for single-line strings, or the conventional
/// multi-line form (`keyword ""` followed by one quoted chunk per `\n`)
/// for values containing newlines.
fn write_quoted(output: &mut String, keyword: &str, value: &str) {
    if !value.contains('\n') {
        output.push_str(keyword);
        output.push_str(" \"");
        output.push_str(&escape_po_string(value));
        output.push_str("\"\n");
        return;
    }

    output.push_str(keyword);
    output.push_str(" \"\"\n");
    let mut start = 0;
    let bytes = value.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' {
            let chunk = &value[start..=i];
            output.push('"');
            output.push_str(&escape_po_string(chunk));
            output.push_str("\"\n");
            start = i + 1;
        }
    }
    if start < value.len() {
        let chunk = &value[start..];
        output.push('"');
        output.push_str(&escape_po_string(chunk));
        output.push_str("\"\n");
    }
}

/// Serialize a PO file back to its on-disk text form.
pub fn serialize_po(file: &GettextFile) -> String {
    let mut output = String::new();

    for ((msgid, msgctxt), entry) in &file.entries {
        for comment in &entry.translator_comment {
            for line in comment.lines() {
                output.push_str("# ");
                output.push_str(line);
                output.push('\n');
            }
        }
        for comment in &entry.extracted_comment {
            for line in comment.lines() {
                output.push_str("#. ");
                output.push_str(line);
                output.push('\n');
            }
        }
        for location in &entry.source_locations {
            for line in location.lines() {
                output.push_str("#: ");
                output.push_str(line);
                output.push('\n');
            }
        }
        if !entry.flags.is_empty() {
            output.push_str("#, ");
            output.push_str(&entry.flags.join(", "));
            output.push('\n');
        }
        if let Some(prev) = &entry.previous_msgid {
            output.push_str("#| msgid \"");
            output.push_str(&escape_po_string(prev));
            output.push_str("\"\n");
        }

        if let Some(ctx) = msgctxt {
            write_quoted(&mut output, "msgctxt", ctx);
        }
        write_quoted(&mut output, "msgid", msgid);

        if let Some(plural) = &entry.msgid_plural {
            write_quoted(&mut output, "msgid_plural", plural);

            for (idx, trans) in entry.msgstr_plural.iter().enumerate() {
                write_quoted(&mut output, &format!("msgstr[{}]", idx), trans);
            }
        } else {
            write_quoted(&mut output, "msgstr", &entry.msgstr);
        }

        output.push('\n');
    }

    if !file.obsolete_lines.is_empty() {
        for line in &file.obsolete_lines {
            output.push_str(line);
            output.push('\n');
        }
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageEntry;
    use crate::service::parser::parse_po;

    #[test]
    fn serialize_basic_entry() {
        let mut file = GettextFile::new();
        let entry = MessageEntry {
            msgid: "Hello".into(),
            msgstr: "Bonjour".into(),
            extracted_comment: vec!["A greeting".into()],
            source_locations: vec!["main.rs:42".into()],
            ..Default::default()
        };
        file.entries.insert(("Hello".into(), None), entry);

        let serialized = serialize_po(&file);
        assert!(serialized.contains("msgid \"Hello\""));
        assert!(serialized.contains("msgstr \"Bonjour\""));
        assert!(serialized.contains("#. A greeting"));
        assert!(serialized.contains("#: main.rs:42"));
    }

    #[test]
    fn serialize_roundtrip_with_escapes() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Line one\nLine two".into(), None),
            MessageEntry {
                msgid: "Line one\nLine two".into(),
                msgstr: "Ligne un\nLigne deux".into(),
                ..Default::default()
            },
        );

        let serialized = serialize_po(&file);
        assert!(serialized.contains("msgid \"\"\n\"Line one\\n\"\n\"Line two\""));
        assert!(serialized.contains("msgstr \"\"\n\"Ligne un\\n\"\n\"Ligne deux\""));

        let reparsed = parse_po(&serialized).expect("reparse");
        let entry = reparsed
            .entries
            .get(&("Line one\nLine two".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgstr, "Ligne un\nLigne deux");
    }

    #[test]
    fn serialize_roundtrip_with_plurals() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("%d item".into(), None),
            MessageEntry {
                msgid: "%d item".into(),
                msgid_plural: Some("%d items".into()),
                msgstr_plural: vec!["%d élément".into(), "%d éléments".into()],
                ..Default::default()
            },
        );

        let serialized = serialize_po(&file);
        let reparsed = parse_po(&serialized).expect("reparse");
        let entry = reparsed
            .entries
            .get(&("%d item".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgid_plural, Some("%d items".into()));
        assert_eq!(entry.msgstr_plural.len(), 2);
    }

    #[test]
    fn serialize_roundtrip_with_context() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("OK".into(), Some("button".into())),
            MessageEntry {
                msgid: "OK".into(),
                msgctxt: Some("button".into()),
                msgstr: "D'accord".into(),
                ..Default::default()
            },
        );

        let serialized = serialize_po(&file);
        assert!(serialized.contains("msgctxt \"button\""));

        let reparsed = parse_po(&serialized).expect("reparse");
        let entry = reparsed
            .entries
            .get(&("OK".to_string(), Some("button".to_string())))
            .unwrap();
        assert_eq!(entry.msgstr, "D'accord");
    }

    #[test]
    fn serialize_roundtrip_with_all_comment_types() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                extracted_comment: vec!["Extracted note".into()],
                translator_comment: vec!["Translator note".into()],
                source_locations: vec!["app.rs:10".into()],
                flags: vec!["fuzzy".into(), "c-format".into()],
                previous_msgid: Some("Old Hello".into()),
                ..Default::default()
            },
        );

        let serialized = serialize_po(&file);
        assert!(serialized.contains("# Translator note"));
        assert!(serialized.contains("#. Extracted note"));
        assert!(serialized.contains("#: app.rs:10"));
        assert!(serialized.contains("#, fuzzy, c-format"));
        assert!(serialized.contains("#| msgid \"Old Hello\""));

        let reparsed = parse_po(&serialized).expect("reparse");
        let entry = reparsed.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.extracted_comment, vec!["Extracted note"]);
        assert_eq!(entry.translator_comment, vec!["Translator note"]);
        assert_eq!(entry.source_locations, vec!["app.rs:10"]);
        assert!(entry.flags.contains(&"fuzzy".to_string()));
        assert!(entry.flags.contains(&"c-format".to_string()));
        assert_eq!(entry.previous_msgid, Some("Old Hello".into()));
    }

    #[test]
    fn serialize_obsolete_lines_preserved() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Active".into(), None),
            MessageEntry {
                msgid: "Active".into(),
                msgstr: "Actif".into(),
                ..Default::default()
            },
        );
        file.obsolete_lines = vec![
            "#~ msgid \"Old\"".into(),
            "#~ msgstr \"Ancien\"".into(),
        ];

        let serialized = serialize_po(&file);
        assert!(serialized.contains("#~ msgid \"Old\""));
        assert!(serialized.contains("#~ msgstr \"Ancien\""));
    }

    #[test]
    fn full_header_metadata_roundtrip() {
        // Crowdin-managed Swedish header: every key parsed, original
        // insertion order kept, multi-line form preserved, byte-stable
        // across a second serialize/parse cycle.
        let input = "msgid \"\"\n\
            msgstr \"\"\n\
            \"Language: sv\\n\"\n\
            \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n\
            \"X-Crowdin-Project: eyr-phoenix\\n\"\n\
            \"X-Crowdin-Project-ID: 695599\\n\"\n\
            \"X-Crowdin-Language: sv-SE\\n\"\n\
            \"X-Crowdin-File: errors.po\\n\"\n\
            \"X-Crowdin-File-ID: 22\\n\"\n\
            \"Project-Id-Version: eyr-phoenix\\n\"\n\
            \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
            \"Language-Team: Swedish\\n\"\n\
            \"PO-Revision-Date: 2026-04-20 10:51\\n\"\n\
            \n\
            msgid \"Hello\"\n\
            msgstr \"Hej\"\n";

        let parsed = parse_po(input).expect("parse");
        let expected_order = [
            ("Language", "sv"),
            ("Plural-Forms", "nplurals=2; plural=(n != 1);"),
            ("X-Crowdin-Project", "eyr-phoenix"),
            ("X-Crowdin-Project-ID", "695599"),
            ("X-Crowdin-Language", "sv-SE"),
            ("X-Crowdin-File", "errors.po"),
            ("X-Crowdin-File-ID", "22"),
            ("Project-Id-Version", "eyr-phoenix"),
            ("Content-Type", "text/plain; charset=UTF-8"),
            ("Language-Team", "Swedish"),
            ("PO-Revision-Date", "2026-04-20 10:51"),
        ];
        let actual: Vec<(&str, &str)> = parsed
            .metadata
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(actual, expected_order);

        let serialized = serialize_po(&parsed);
        assert!(serialized.contains("msgstr \"\"\n\"Language: sv\\n\""));
        assert!(serialized.contains("\"PO-Revision-Date: 2026-04-20 10:51\\n\""));
        assert!(!serialized.contains("Language: sv\\nPlural-Forms"));

        let reparsed = parse_po(&serialized).expect("reparse");
        let reparsed_order: Vec<(&str, &str)> = reparsed
            .metadata
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(reparsed_order, expected_order);

        // Fixed point.
        assert_eq!(serialized, serialize_po(&reparsed));
    }

    #[test]
    fn arbitrary_headers_roundtrip() {
        let input = "msgid \"\"\n\
            msgstr \"\"\n\
            \"Project-Id-Version: my-app 1.2.3\\n\"\n\
            \"Report-Msgid-Bugs-To: bugs@example.com\\n\"\n\
            \"POT-Creation-Date: 2026-01-15 09:00+0000\\n\"\n\
            \"PO-Revision-Date: 2026-04-20 10:51+0200\\n\"\n\
            \"Last-Translator: Anna Andersson <anna@example.com>\\n\"\n\
            \"Language-Team: Swedish <sv@li.org>\\n\"\n\
            \"Language: sv_SE\\n\"\n\
            \"MIME-Version: 1.0\\n\"\n\
            \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
            \"Content-Transfer-Encoding: 8bit\\n\"\n\
            \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n\
            \"X-Generator: Poedit 3.4.2\\n\"\n\
            \"X-Poedit-SourceCharset: UTF-8\\n\"\n\
            \"X-Poedit-Basepath: ../..\\n\"\n\
            \"X-Crowdin-Project: my-app\\n\"\n\
            \"X-Custom-Vendor-Header: anything goes here\\n\"\n\
            \"X-Empty-Value: \\n\"\n\
            \"Some-Unknown-Key: value with: embedded colons and ; semicolons\\n\"\n\
            \n";

        let parsed = parse_po(input).expect("parse");
        let keys: Vec<&str> = parsed.metadata.keys().map(String::as_str).collect();
        let expected_keys = [
            "Project-Id-Version",
            "Report-Msgid-Bugs-To",
            "POT-Creation-Date",
            "PO-Revision-Date",
            "Last-Translator",
            "Language-Team",
            "Language",
            "MIME-Version",
            "Content-Type",
            "Content-Transfer-Encoding",
            "Plural-Forms",
            "X-Generator",
            "X-Poedit-SourceCharset",
            "X-Poedit-Basepath",
            "X-Crowdin-Project",
            "X-Custom-Vendor-Header",
            "X-Empty-Value",
            "Some-Unknown-Key",
        ];
        assert_eq!(keys, expected_keys);

        assert_eq!(
            parsed.metadata.get("X-Empty-Value").map(String::as_str),
            Some("")
        );
        assert_eq!(
            parsed.metadata.get("Some-Unknown-Key").map(String::as_str),
            Some("value with: embedded colons and ; semicolons"),
        );
        assert_eq!(
            parsed.metadata.get("Last-Translator").map(String::as_str),
            Some("Anna Andersson <anna@example.com>"),
        );
        assert_eq!(
            parsed.metadata.get("Content-Type").map(String::as_str),
            Some("text/plain; charset=UTF-8"),
        );

        let serialized = serialize_po(&parsed);
        let reparsed = parse_po(&serialized).expect("reparse");
        let reparsed_keys: Vec<&str> = reparsed.metadata.keys().map(String::as_str).collect();
        assert_eq!(reparsed_keys, expected_keys);
        for key in expected_keys {
            assert_eq!(
                parsed.metadata.get(key),
                reparsed.metadata.get(key),
                "key {key}"
            );
        }
    }
}
