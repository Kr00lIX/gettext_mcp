//! In-memory representation of a parsed PO file.

use indexmap::IndexMap;

use super::entry::MessageEntry;

/// A parsed PO file: ordered translation entries plus header metadata
/// plus any preserved obsolete (`#~`) lines.
#[derive(Debug, Clone)]
pub struct GettextFile {
    /// All translation entries, including the header at key `("", None)`.
    pub entries: IndexMap<(String, Option<String>), MessageEntry>,
    /// Header metadata parsed out of the `msgid ""` entry's `msgstr`.
    pub metadata: IndexMap<String, String>,
    /// Raw obsolete entry lines (`#~ ...`) preserved verbatim for
    /// round-trip fidelity.
    pub obsolete_lines: Vec<String>,
}

impl GettextFile {
    /// Construct an empty PO file with no entries and no metadata.
    pub fn new() -> Self {
        Self {
            entries: IndexMap::new(),
            metadata: IndexMap::new(),
            obsolete_lines: Vec::new(),
        }
    }

    /// `Plural-Forms` header, if set.
    pub fn plural_forms(&self) -> Option<String> {
        self.metadata.get("Plural-Forms").cloned()
    }

    /// `Language` header, if set.
    pub fn language(&self) -> Option<String> {
        self.metadata.get("Language").cloned()
    }

    /// Rebuild the header entry's `msgstr` from the current metadata map.
    ///
    /// Called by store mutators that change a header value: the on-disk
    /// representation is the joined `Key: Value\n` block, so the entry
    /// has to be regenerated whenever a header changes.
    pub fn rebuild_header_entry(&mut self) {
        let mut header_str = String::new();
        for (k, v) in self.metadata.iter() {
            header_str.push_str(&format!("{}: {}\n", k, v));
        }
        let header_key = (String::new(), None);
        if let Some(entry) = self.entries.get_mut(&header_key) {
            entry.msgstr = header_str;
        } else {
            self.entries.insert(
                header_key,
                MessageEntry {
                    msgstr: header_str,
                    ..Default::default()
                },
            );
        }
    }
}

impl Default for GettextFile {
    fn default() -> Self {
        Self::new()
    }
}
