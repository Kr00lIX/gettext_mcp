//! A single translation entry within a PO file.

/// Represents a single translation entry in a PO file.
///
/// The header entry is also a [`MessageEntry`] — it's the one whose
/// `msgid` is empty and whose `msgstr` is the `Key: Value\n` header block.
#[derive(Debug, Clone, Default)]
pub struct MessageEntry {
    /// Message ID (singular form, typically English).
    pub msgid: String,
    /// Optional context for disambiguation (`msgctxt`).
    pub msgctxt: Option<String>,
    /// Translated string (singular form).
    pub msgstr: String,
    /// Plural form of the source string (`msgid_plural`).
    pub msgid_plural: Option<String>,
    /// Plural translations: `[0]` = singular, `[1]` = plural, etc.
    pub msgstr_plural: Vec<String>,
    /// Developer/extracted comments (lines starting with `#.`).
    pub extracted_comment: Vec<String>,
    /// Translator comments (lines starting with `# `).
    pub translator_comment: Vec<String>,
    /// Source code locations (lines starting with `#:`).
    pub source_locations: Vec<String>,
    /// Flags like `fuzzy`, `c-format`, `python-format` (lines starting `#,`).
    pub flags: Vec<String>,
    /// Previous untranslated string (line starting with `#|`).
    pub previous_msgid: Option<String>,
}

impl MessageEntry {
    /// True when the entry carries the `fuzzy` flag.
    pub fn is_fuzzy(&self) -> bool {
        self.flags.iter().any(|f| f == "fuzzy")
    }

    /// True when the entry is fully translated per GNU gettext semantics.
    ///
    /// Fuzzy entries are *not* considered translated: the gettext runtime
    /// silently ignores them, so for our purposes they're untranslated.
    pub fn is_translated(&self) -> bool {
        if self.is_fuzzy() {
            return false;
        }
        if self.msgid_plural.is_some() {
            !self.msgstr_plural.is_empty() && self.msgstr_plural.iter().all(|s| !s.is_empty())
        } else {
            !self.msgstr.is_empty()
        }
    }
}
