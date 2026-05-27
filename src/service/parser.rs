//! Line-by-line PO file parser.
//!
//! Pure string-in/struct-out logic: no I/O, no async. Translates a UTF-8
//! `.po` source into a [`GettextFile`] (or returns [`GettextError`]).

use crate::error::GettextError;
use crate::model::{GettextFile, MessageEntry};

#[derive(Debug, Clone)]
enum LineType {
    TranslatorComment, // `# ...`
    ExtractedComment,  // `#. ...`
    SourceLocation,    // `#: ...`
    Flags,             // `#, fuzzy, c-format`
    PreviousMsgid,     // `#| msgid "..."`
    Obsolete,          // `#~ ...`
    Msgctxt,           // `msgctxt "..."`
    Msgid,             // `msgid "..."`
    MsgidPlural,       // `msgid_plural "..."`
    Msgstr,            // `msgstr "..."` or `msgstr[n] "..."`
    Continuation,      // `"..."` on its own line
    Blank,
}

#[derive(Debug, Clone, PartialEq)]
enum CurrentField {
    Msgctxt,
    Msgid,
    MsgidPlural,
    Msgstr,
    MsgstrPlural(usize),
}

fn classify_line(line: &str) -> LineType {
    if line.is_empty() {
        LineType::Blank
    } else if line.starts_with("#.") {
        LineType::ExtractedComment
    } else if line.starts_with("#:") {
        LineType::SourceLocation
    } else if line.starts_with("#,") {
        LineType::Flags
    } else if line.starts_with("#|") {
        LineType::PreviousMsgid
    } else if line.starts_with("#~") {
        LineType::Obsolete
    } else if line.starts_with('#') {
        LineType::TranslatorComment
    } else if line.starts_with("msgctxt") {
        LineType::Msgctxt
    } else if line.starts_with("msgid_plural") {
        LineType::MsgidPlural
    } else if line.starts_with("msgid") {
        LineType::Msgid
    } else if line.starts_with("msgstr") {
        LineType::Msgstr
    } else if line.starts_with('"') {
        LineType::Continuation
    } else {
        LineType::Blank
    }
}

/// Strip the keyword (msgid/msgstr/...) and surrounding quotes from
/// `keyword "value"`, then unescape PO escapes in the value.
fn parse_string_literal(line: &str) -> String {
    if let Some(start) = line.find('"') {
        if let Some(end) = line.rfind('"') {
            if start < end {
                let raw = &line[start + 1..end];
                return unescape_po_string(raw);
            }
        }
    }
    String::new()
}

/// Translate PO-format escape sequences (`\\`, `\"`, `\n`, `\r`, `\t`)
/// in `s` back into their literal characters.
pub fn unescape_po_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some(c) => {
                    result.push('\\');
                    result.push(c);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Parse PO file content into a [`GettextFile`].
pub fn parse_po(content: &str) -> Result<GettextFile, GettextError> {
    let mut file = GettextFile::new();
    let mut current_entry = MessageEntry::default();

    let mut in_entry = false;
    let mut current_field: Option<CurrentField> = None;

    for line in content.lines() {
        match classify_line(line) {
            LineType::TranslatorComment => {
                current_entry
                    .translator_comment
                    .push(line[1..].trim().to_string());
            }
            LineType::ExtractedComment => {
                current_entry
                    .extracted_comment
                    .push(line[2..].trim().to_string());
            }
            LineType::SourceLocation => {
                current_entry
                    .source_locations
                    .push(line[2..].trim().to_string());
            }
            LineType::Flags => {
                let flags_str = &line[2..].trim();
                if !flags_str.is_empty() {
                    current_entry.flags.extend(
                        flags_str
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    );
                }
            }
            LineType::PreviousMsgid => {
                current_entry.previous_msgid = Some(parse_string_literal(line));
            }
            LineType::Obsolete => {
                file.obsolete_lines.push(line.to_string());
            }
            LineType::Msgctxt => {
                if in_entry && (!current_entry.msgid.is_empty() || !current_entry.msgstr.is_empty())
                {
                    let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());
                    file.entries.insert(key, current_entry.clone());
                    current_entry = MessageEntry::default();
                }
                in_entry = true;
                current_field = Some(CurrentField::Msgctxt);
                current_entry.msgctxt = Some(parse_string_literal(line));
            }
            LineType::Msgid => {
                if in_entry && (!current_entry.msgid.is_empty() || !current_entry.msgstr.is_empty())
                {
                    let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());
                    file.entries.insert(key, current_entry.clone());
                    current_entry = MessageEntry::default();
                }
                in_entry = true;
                current_field = Some(CurrentField::Msgid);
                current_entry.msgid = parse_string_literal(line);
            }
            LineType::MsgidPlural => {
                current_field = Some(CurrentField::MsgidPlural);
                current_entry.msgid_plural = Some(parse_string_literal(line));
            }
            LineType::Msgstr => {
                let value = parse_string_literal(line);

                if line.contains("msgstr[") {
                    if let Some(start) = line.find('[') {
                        if let Some(end) = line.find(']') {
                            if let Ok(index) = line[start + 1..end].parse::<usize>() {
                                // Plural indices must arrive in order.
                                if index != current_entry.msgstr_plural.len() {
                                    return Err(GettextError::InvalidFormat(format!(
                                        "Plural form indices must be sequential. Expected {}, got {}",
                                        current_entry.msgstr_plural.len(),
                                        index
                                    )));
                                }
                                current_entry.msgstr_plural.push(value);
                                current_field = Some(CurrentField::MsgstrPlural(index));
                            }
                        }
                    }
                } else {
                    current_entry.msgstr = value;
                    current_field = Some(CurrentField::Msgstr);
                }
            }
            LineType::Continuation => {
                let continuation = parse_string_literal(line);
                match &current_field {
                    Some(CurrentField::Msgstr) => {
                        current_entry.msgstr.push_str(&continuation);
                    }
                    Some(CurrentField::MsgstrPlural(index))
                        if *index < current_entry.msgstr_plural.len() =>
                    {
                        current_entry.msgstr_plural[*index].push_str(&continuation);
                    }
                    Some(CurrentField::Msgid) => {
                        current_entry.msgid.push_str(&continuation);
                    }
                    Some(CurrentField::MsgidPlural) => {
                        if let Some(plural) = &mut current_entry.msgid_plural {
                            plural.push_str(&continuation);
                        }
                    }
                    Some(CurrentField::Msgctxt) => {
                        if let Some(ctx) = &mut current_entry.msgctxt {
                            ctx.push_str(&continuation);
                        }
                    }
                    Some(CurrentField::MsgstrPlural(_)) | None => {}
                }
            }
            LineType::Blank => {
                if in_entry {
                    let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());

                    // Header entry is identified by empty msgid AND no context.
                    if current_entry.msgid.is_empty() && current_entry.msgctxt.is_none() {
                        for header_line in current_entry.msgstr.lines() {
                            if let Some((k, value)) = header_line.split_once(':') {
                                file.metadata
                                    .insert(k.trim().to_string(), value.trim().to_string());
                            }
                        }
                    }

                    file.entries.insert(key, current_entry.clone());
                    current_entry = MessageEntry::default();
                    in_entry = false;
                    current_field = None;
                }
            }
        }
    }

    // Trailing entry without a blank-line terminator.
    if in_entry {
        if current_entry.msgid.is_empty() && current_entry.msgctxt.is_none() {
            for header_line in current_entry.msgstr.lines() {
                if let Some((k, value)) = header_line.split_once(':') {
                    file.metadata
                        .insert(k.trim().to_string(), value.trim().to_string());
                }
            }
        }
        let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());
        file.entries.insert(key, current_entry);
    }

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_po() {
        let content = r#"
msgid ""
msgstr ""
"Language: en\n"

msgid "Hello"
msgstr "Bonjour"

msgid "World"
msgstr "Monde"
"#;

        let file = parse_po(content).expect("parse");
        assert_eq!(file.entries.len(), 3);

        let hello = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(hello.msgstr, "Bonjour");
        let world = file.entries.get(&("World".to_string(), None)).unwrap();
        assert_eq!(world.msgstr, "Monde");
        assert_eq!(file.metadata.get("Language"), Some(&"en".to_string()));
    }

    #[test]
    fn parse_with_context() {
        let content = r#"
msgctxt "greeting"
msgid "Hello"
msgstr "Bonjour"

msgctxt "farewell"
msgid "Hello"
msgstr "Adieu"
"#;
        let file = parse_po(content).expect("parse");
        assert_eq!(file.entries.len(), 2);
        let greeting = file
            .entries
            .get(&("Hello".to_string(), Some("greeting".to_string())))
            .unwrap();
        assert_eq!(greeting.msgstr, "Bonjour");
        let farewell = file
            .entries
            .get(&("Hello".to_string(), Some("farewell".to_string())))
            .unwrap();
        assert_eq!(farewell.msgstr, "Adieu");
    }

    #[test]
    fn parse_with_flags() {
        let content = r#"
#, fuzzy, c-format
msgid "Hello %s"
msgstr "Bonjour %s"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.values().next().unwrap();
        assert!(entry.is_fuzzy());
        assert!(entry.flags.contains(&"c-format".to_string()));
    }

    #[test]
    fn parse_multiline_strings() {
        let content = r#"
msgid ""
"This is a long "
"multiline string"
msgstr ""
"Ceci est une longue "
"chaine multiligne"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file
            .entries
            .get(&("This is a long multiline string".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgstr, "Ceci est une longue chaine multiligne");
    }

    #[test]
    fn parse_obsolete_entries_not_misclassified() {
        let content = r#"
msgid "Active"
msgstr "Actif"

#~ msgid "Old entry"
#~ msgstr "Ancienne entree"

msgid "Another"
msgstr "Autre"
"#;
        let file = parse_po(content).expect("parse");
        let active = file.entries.get(&("Active".to_string(), None)).unwrap();
        assert!(active.translator_comment.is_empty());
        let another = file.entries.get(&("Another".to_string(), None)).unwrap();
        assert!(another.translator_comment.is_empty());
    }

    #[test]
    fn parse_escape_sequences() {
        let content = r#"
msgid "Line one\nLine two"
msgstr "Ligne un\nLigne deux"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file
            .entries
            .get(&("Line one\nLine two".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgstr, "Ligne un\nLigne deux");
    }

    #[test]
    fn parse_escaped_quotes() {
        let content = r#"
msgid "She said \"hello\""
msgstr "Elle a dit \"bonjour\""
"#;
        let file = parse_po(content).expect("parse");
        let entry = file
            .entries
            .get(&("She said \"hello\"".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgstr, "Elle a dit \"bonjour\"");
    }

    #[test]
    fn parse_escaped_backslash() {
        let content = r#"
msgid "path\\to\\file"
msgstr "chemin\\vers\\fichier"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file
            .entries
            .get(&("path\\to\\file".to_string(), None))
            .unwrap();
        assert_eq!(entry.msgstr, "chemin\\vers\\fichier");
    }

    #[test]
    fn parse_tabs() {
        let content = "msgid \"col1\\tcol2\"\nmsgstr \"col1\\tcol2\"\n";
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("col1\tcol2".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "col1\tcol2");
    }

    #[test]
    fn parse_plural_forms() {
        let content = r#"
msgid "%d item"
msgid_plural "%d items"
msgstr[0] "%d élément"
msgstr[1] "%d éléments"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("%d item".to_string(), None)).unwrap();
        assert_eq!(entry.msgid_plural, Some("%d items".to_string()));
        assert_eq!(entry.msgstr_plural.len(), 2);
        assert_eq!(entry.msgstr_plural[0], "%d élément");
        assert_eq!(entry.msgstr_plural[1], "%d éléments");
    }

    #[test]
    fn parse_three_plural_forms() {
        let content = r#"
msgid "%d file"
msgid_plural "%d files"
msgstr[0] "%d plik"
msgstr[1] "%d pliki"
msgstr[2] "%d plików"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("%d file".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr_plural.len(), 3);
        assert_eq!(entry.msgstr_plural[2], "%d plików");
    }

    #[test]
    fn parse_non_sequential_plural_index_fails() {
        let content = r#"
msgid "%d item"
msgid_plural "%d items"
msgstr[0] "zero"
msgstr[2] "skipped one"
"#;
        assert!(parse_po(content).is_err());
    }

    #[test]
    fn parse_extracted_comments() {
        let content = r#"
#. This is an extracted comment
#. Second line of extracted comment
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.extracted_comment.len(), 2);
        assert_eq!(entry.extracted_comment[0], "This is an extracted comment");
        assert_eq!(
            entry.extracted_comment[1],
            "Second line of extracted comment"
        );
    }

    #[test]
    fn parse_source_locations() {
        let content = r#"
#: src/main.rs:42
#: src/lib.rs:100
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.source_locations.len(), 2);
    }

    #[test]
    fn parse_previous_msgid() {
        let content = r#"
#| msgid "Old hello"
#, fuzzy
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.previous_msgid, Some("Old hello".to_string()));
        assert!(entry.is_fuzzy());
    }

    #[test]
    fn parse_empty_file() {
        let file = parse_po("").expect("parse");
        assert!(file.entries.is_empty());
        assert!(file.metadata.is_empty());
    }

    #[test]
    fn parse_header_only() {
        let content = r#"
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Language: fr\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"
"#;
        let file = parse_po(content).expect("parse");
        assert_eq!(file.metadata.get("Language"), Some(&"fr".to_string()));
        assert_eq!(
            file.metadata.get("Plural-Forms"),
            Some(&"nplurals=2; plural=(n != 1);".to_string())
        );
    }

    #[test]
    fn parse_multiple_flags_on_one_line() {
        let content = r#"
#, fuzzy, c-format, python-format
msgid "Hello %s"
msgstr "Bonjour %s"
"#;
        let file = parse_po(content).expect("parse");
        let entry = file.entries.values().next().unwrap();
        assert!(entry.flags.contains(&"fuzzy".to_string()));
        assert!(entry.flags.contains(&"c-format".to_string()));
        assert!(entry.flags.contains(&"python-format".to_string()));
    }

    #[test]
    fn parse_obsolete_lines_preserved() {
        let content = r#"
msgid "Active"
msgstr "Actif"

#~ msgid "Removed"
#~ msgstr "Supprimé"
"#;
        let file = parse_po(content).expect("parse");
        assert_eq!(file.obsolete_lines.len(), 2);
        assert!(file.obsolete_lines[0].contains("Removed"));
    }

    #[test]
    fn unescape_handles_unknown_escape() {
        assert_eq!(unescape_po_string("a\\xb"), "a\\xb");
        assert_eq!(unescape_po_string("trailing\\"), "trailing\\");
    }
}
