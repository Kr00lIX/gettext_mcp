//! Binary `.mo` file encoder — the in-house equivalent of GNU `msgfmt`.
//!
//! The `.mo` file format is laid out as a small fixed header followed by
//! two parallel `(length, offset)` index tables (one for original strings,
//! one for translations) and finally the strings themselves, each
//! NUL-terminated.
//!
//! Layout (all multi-byte fields little-endian, matching the magic we
//! emit):
//!
//! ```text
//! offset  size  meaning
//!   0     u32   magic (0x950412de)
//!   4     u32   format version (0)
//!   8     u32   N: number of strings
//!  12     u32   O: file offset of the originals' (length, offset) table
//!  16     u32   T: file offset of the translations' (length, offset) table
//!  20     u32   S: size of the hash table (we emit 0 — no hash table)
//!  24     u32   H: file offset of the hash table (0 when S == 0)
//!   …     2*N*8 bytes  the two index tables back-to-back
//!   …     packed strings, each NUL-terminated
//! ```
//!
//! References:
//! * <https://www.gnu.org/software/gettext/manual/html_node/MO-Files.html>
//!
//! Conventions implemented here:
//!
//! * Entries with the `fuzzy` flag and entries with an empty translation
//!   are skipped (gettext runtime does the same).
//! * The PO header (`msgid == ""`) is *always* included, even though its
//!   `msgstr` looks empty from the API surface — it carries `Language`,
//!   `Plural-Forms`, etc. and the runtime needs it.
//! * Plural entries: original side encodes `msgid + "\0" + msgid_plural`,
//!   translation side joins all `msgstr_plural[i]` with NUL separators.
//!   Plural entries with any empty form are skipped.
//! * Entries are sorted by their original-side bytes (gettext convention,
//!   permits the runtime to use binary search even though we don't emit a
//!   hash table).
//!
//! Deviations from the GNU spec we know about:
//!
//! * No hash table (S = 0, H = 0). The format permits this — runtimes
//!   fall back to binary search over the sorted strings table. GNU
//!   `msgfmt` does emit a hash table by default; ours is byte-smaller but
//!   functionally compatible.
//! * No system-dependent message segments (the optional v1 extension).
//!   We always emit version 0.
//! * Byte order is fixed to little-endian. Big-endian readers detect this
//!   via the magic number and byteswap accordingly, so this is the
//!   canonical layout.

use std::cmp::Ordering;

use crate::error::GettextError;
use crate::model::GettextFile;

/// MO magic number — readers detect endianness by comparing against this
/// constant and its byteswap.
pub const MO_MAGIC: u32 = 0x950412de;
/// Format revision number we emit. The spec defines 0 (current major
/// version) and we don't use any of the v1 extensions.
pub const MO_VERSION: u32 = 0;

/// One entry pre-encoded as the byte strings the MO format wants.
struct MoEntry {
    original: Vec<u8>,
    translation: Vec<u8>,
}

/// Compile a [`GettextFile`] into a binary `.mo` byte buffer.
///
/// Skips fuzzy entries and entries whose msgstr (or any msgstr_plural
/// form) is empty. The PO header is always emitted.
pub fn compile_mo(file: &GettextFile) -> Result<Vec<u8>, GettextError> {
    let mut entries: Vec<MoEntry> = Vec::new();

    // Always emit the header when it carries any content. Its key is
    // ("", None) and its msgstr is the joined `Key: Value\n` block.
    // We skip a blank header (no metadata, no msgstr) because gettext
    // runtimes treat an empty msgstr at msgid="" as "no translation
    // present" and an empty .mo without that pseudo-entry is the
    // expected output of `msgfmt` on a brand-new file.
    if let Some(header) = file.entries.get(&(String::new(), None)) {
        if !header.msgstr.is_empty() {
            entries.push(MoEntry {
                original: Vec::new(),
                translation: header.msgstr.as_bytes().to_vec(),
            });
        } else if !file.metadata.is_empty() {
            // Header entry exists but is empty while metadata is set:
            // synthesise header bytes from the metadata map.
            let mut h = String::new();
            for (k, v) in &file.metadata {
                h.push_str(k);
                h.push_str(": ");
                h.push_str(v);
                h.push('\n');
            }
            entries.push(MoEntry {
                original: Vec::new(),
                translation: h.into_bytes(),
            });
        }
    } else if !file.metadata.is_empty() {
        // No header entry at all but metadata is set — synthesise one.
        let mut header = String::new();
        for (k, v) in &file.metadata {
            header.push_str(k);
            header.push_str(": ");
            header.push_str(v);
            header.push('\n');
        }
        entries.push(MoEntry {
            original: Vec::new(),
            translation: header.into_bytes(),
        });
    }

    for ((msgid, msgctxt), entry) in &file.entries {
        // Skip header (already handled).
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        if entry.is_fuzzy() {
            continue;
        }

        // Build the original side. With msgctxt the convention is
        // "msgctxt\u{0004}msgid" (EOT separator), per GNU spec.
        let mut original: Vec<u8> = Vec::new();
        if let Some(ctx) = msgctxt {
            original.extend_from_slice(ctx.as_bytes());
            original.push(0x04);
        }
        original.extend_from_slice(msgid.as_bytes());

        // Build the translation side.
        let translation = if let Some(_plural) = &entry.msgid_plural {
            // Plural entry: need all forms filled.
            if entry.msgstr_plural.is_empty() || entry.msgstr_plural.iter().any(|s| s.is_empty()) {
                continue;
            }
            // Original side: msgid NUL msgid_plural.
            let mut o = original.clone();
            o.push(0);
            o.extend_from_slice(entry.msgid_plural.as_ref().unwrap().as_bytes());
            original = o;

            // Translation: join with NULs.
            let mut t: Vec<u8> = Vec::new();
            for (i, form) in entry.msgstr_plural.iter().enumerate() {
                if i > 0 {
                    t.push(0);
                }
                t.extend_from_slice(form.as_bytes());
            }
            t
        } else {
            if entry.msgstr.is_empty() {
                continue;
            }
            entry.msgstr.as_bytes().to_vec()
        };

        entries.push(MoEntry {
            original,
            translation,
        });
    }

    // Sort by original bytes (gettext convention — enables binary search
    // even without a hash table).
    entries.sort_by(|a, b| match a.original.cmp(&b.original) {
        Ordering::Equal => Ordering::Equal,
        ord => ord,
    });
    // Drop duplicates on the original side (last write wins). The order
    // is stable above so we keep the last entry.
    entries.dedup_by(|a, b| a.original == b.original);

    let n = entries.len() as u32;
    let header_size: u32 = 28;
    let table_size: u32 = n * 8;
    let originals_table_offset = header_size;
    let translations_table_offset = header_size + table_size;
    let mut string_offset = header_size + 2 * table_size;

    // Compute per-entry offsets for the strings region.
    let mut originals_table: Vec<(u32, u32)> = Vec::with_capacity(entries.len());
    let mut translations_table: Vec<(u32, u32)> = Vec::with_capacity(entries.len());
    let mut strings_region: Vec<u8> = Vec::new();

    for e in &entries {
        let olen = e.original.len() as u32;
        originals_table.push((olen, string_offset));
        strings_region.extend_from_slice(&e.original);
        strings_region.push(0);
        string_offset += olen + 1;
    }
    for e in &entries {
        let tlen = e.translation.len() as u32;
        translations_table.push((tlen, string_offset));
        strings_region.extend_from_slice(&e.translation);
        strings_region.push(0);
        string_offset += tlen + 1;
    }

    let mut out =
        Vec::with_capacity(header_size as usize + 2 * table_size as usize + strings_region.len());
    out.extend_from_slice(&MO_MAGIC.to_le_bytes());
    out.extend_from_slice(&MO_VERSION.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&originals_table_offset.to_le_bytes());
    out.extend_from_slice(&translations_table_offset.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // S: hash table size
    out.extend_from_slice(&0u32.to_le_bytes()); // H: hash table offset

    for (len, off) in originals_table {
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
    }
    for (len, off) in translations_table {
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
    }
    out.extend_from_slice(&strings_region);

    Ok(out)
}

/// Counts of what was emitted vs. skipped during [`compile_mo`].
#[derive(Debug, Clone, Default)]
pub struct CompileMoCounts {
    pub string_count: usize,
    pub skipped_fuzzy: usize,
    pub skipped_untranslated: usize,
}

/// Walk the file and report what `compile_mo` *would* skip vs include,
/// without doing the encoding. Useful for the tool's response payload.
pub fn count_compile(file: &GettextFile) -> CompileMoCounts {
    let mut counts = CompileMoCounts::default();
    // Header counts as one string only when it has content (metadata or
    // a non-empty header msgstr). An empty pseudo-header is dropped to
    // match what `compile_mo` actually emits.
    let header_present = file
        .entries
        .get(&(String::new(), None))
        .map(|h| !h.msgstr.is_empty())
        .unwrap_or(false)
        || !file.metadata.is_empty();
    if header_present {
        counts.string_count += 1;
    }
    for ((msgid, msgctxt), entry) in &file.entries {
        if msgid.is_empty() && msgctxt.is_none() {
            continue;
        }
        if entry.is_fuzzy() {
            counts.skipped_fuzzy += 1;
            continue;
        }
        if let Some(_p) = &entry.msgid_plural {
            if entry.msgstr_plural.is_empty() || entry.msgstr_plural.iter().any(|s| s.is_empty()) {
                counts.skipped_untranslated += 1;
                continue;
            }
        } else if entry.msgstr.is_empty() {
            counts.skipped_untranslated += 1;
            continue;
        }
        counts.string_count += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageEntry;

    fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
    }

    /// Decode the .mo header into (magic, version, n, originals_off,
    /// translations_off, hash_size, hash_off).
    fn read_header(buf: &[u8]) -> (u32, u32, u32, u32, u32, u32, u32) {
        (
            read_u32_le(buf, 0),
            read_u32_le(buf, 4),
            read_u32_le(buf, 8),
            read_u32_le(buf, 12),
            read_u32_le(buf, 16),
            read_u32_le(buf, 20),
            read_u32_le(buf, 24),
        )
    }

    /// Pull a NUL-terminated string out of the .mo at `(len, offset)`.
    fn read_string(buf: &[u8], len: u32, offset: u32) -> &[u8] {
        let start = offset as usize;
        let end = start + len as usize;
        &buf[start..end]
    }

    #[test]
    fn empty_file_emits_valid_header() {
        let file = GettextFile::new();
        let out = compile_mo(&file).unwrap();
        let (magic, version, n, o, t, s, h) = read_header(&out);
        assert_eq!(magic, MO_MAGIC);
        assert_eq!(version, MO_VERSION);
        assert_eq!(n, 0);
        assert_eq!(o, 28);
        assert_eq!(t, 28);
        assert_eq!(s, 0);
        assert_eq!(h, 0);
    }

    #[test]
    fn single_entry_layout_is_correct() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (magic, _, n, o_off, t_off, _, _) = read_header(&out);
        assert_eq!(magic, MO_MAGIC);
        assert_eq!(n, 1);

        let o_len = read_u32_le(&out, o_off as usize);
        let o_str_off = read_u32_le(&out, o_off as usize + 4);
        let t_len = read_u32_le(&out, t_off as usize);
        let t_str_off = read_u32_le(&out, t_off as usize + 4);

        assert_eq!(read_string(&out, o_len, o_str_off), b"Hello");
        assert_eq!(read_string(&out, t_len, t_str_off), b"Bonjour");
        // NUL terminator must follow each string.
        assert_eq!(out[(o_str_off + o_len) as usize], 0);
        assert_eq!(out[(t_str_off + t_len) as usize], 0);
    }

    #[test]
    fn header_entry_emitted() {
        let mut file = GettextFile::new();
        file.metadata.insert("Language".into(), "fr".into());
        file.metadata
            .insert("Plural-Forms".into(), "nplurals=2; plural=(n > 1);".into());
        file.rebuild_header_entry();
        file.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, o_off, _, _, _) = read_header(&out);
        assert_eq!(n, 2, "header + Hello = 2 strings");
        // After sorting on original-side bytes, the empty header msgid
        // (length 0) comes first.
        let first_len = read_u32_le(&out, o_off as usize);
        assert_eq!(first_len, 0, "header msgid is empty and sorts first");
    }

    #[test]
    fn fuzzy_entries_are_skipped() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );
        file.entries.insert(
            ("Fuzzy".into(), None),
            MessageEntry {
                msgid: "Fuzzy".into(),
                msgstr: "Flou".into(),
                flags: vec!["fuzzy".into()],
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, _, _, _, _) = read_header(&out);
        assert_eq!(n, 1);

        let counts = count_compile(&file);
        assert_eq!(counts.skipped_fuzzy, 1);
        assert_eq!(counts.string_count, 1);
    }

    #[test]
    fn untranslated_entries_are_skipped() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Hello".into(), None),
            MessageEntry {
                msgid: "Hello".into(),
                msgstr: "Bonjour".into(),
                ..Default::default()
            },
        );
        file.entries.insert(
            ("Missing".into(), None),
            MessageEntry {
                msgid: "Missing".into(),
                msgstr: "".into(),
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, _, _, _, _) = read_header(&out);
        assert_eq!(n, 1);

        let counts = count_compile(&file);
        assert_eq!(counts.skipped_untranslated, 1);
    }

    #[test]
    fn plural_entry_encoded_with_nul_separators() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("%d file".into(), None),
            MessageEntry {
                msgid: "%d file".into(),
                msgid_plural: Some("%d files".into()),
                msgstr_plural: vec!["%d fichier".into(), "%d fichiers".into()],
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, o_off, t_off, _, _) = read_header(&out);
        assert_eq!(n, 1);

        let o_len = read_u32_le(&out, o_off as usize);
        let o_str_off = read_u32_le(&out, o_off as usize + 4);
        let t_len = read_u32_le(&out, t_off as usize);
        let t_str_off = read_u32_le(&out, t_off as usize + 4);

        let original = read_string(&out, o_len, o_str_off);
        let translation = read_string(&out, t_len, t_str_off);
        assert_eq!(original, b"%d file\0%d files");
        assert_eq!(translation, b"%d fichier\0%d fichiers");
    }

    #[test]
    fn msgctxt_uses_eot_separator() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Open".into(), Some("menu".into())),
            MessageEntry {
                msgid: "Open".into(),
                msgctxt: Some("menu".into()),
                msgstr: "Ouvrir".into(),
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, o_off, _, _, _) = read_header(&out);
        assert_eq!(n, 1);
        let o_len = read_u32_le(&out, o_off as usize);
        let o_str_off = read_u32_le(&out, o_off as usize + 4);
        let original = read_string(&out, o_len, o_str_off);
        assert_eq!(original, b"menu\x04Open");
    }

    #[test]
    fn entries_are_sorted_by_original() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("Banana".into(), None),
            MessageEntry {
                msgid: "Banana".into(),
                msgstr: "Banane".into(),
                ..Default::default()
            },
        );
        file.entries.insert(
            ("Apple".into(), None),
            MessageEntry {
                msgid: "Apple".into(),
                msgstr: "Pomme".into(),
                ..Default::default()
            },
        );
        file.entries.insert(
            ("Cherry".into(), None),
            MessageEntry {
                msgid: "Cherry".into(),
                msgstr: "Cerise".into(),
                ..Default::default()
            },
        );

        let out = compile_mo(&file).unwrap();
        let (_, _, n, o_off, _, _, _) = read_header(&out);
        assert_eq!(n, 3);

        let mut originals = Vec::new();
        for i in 0..n {
            let entry_off = o_off as usize + (i as usize) * 8;
            let len = read_u32_le(&out, entry_off);
            let off = read_u32_le(&out, entry_off + 4);
            originals.push(read_string(&out, len, off).to_vec());
        }
        assert_eq!(originals[0], b"Apple");
        assert_eq!(originals[1], b"Banana");
        assert_eq!(originals[2], b"Cherry");
    }

    #[test]
    fn plural_skipped_when_any_form_empty() {
        let mut file = GettextFile::new();
        file.entries.insert(
            ("%d cat".into(), None),
            MessageEntry {
                msgid: "%d cat".into(),
                msgid_plural: Some("%d cats".into()),
                msgstr_plural: vec!["%d chat".into(), "".into()],
                ..Default::default()
            },
        );

        let counts = count_compile(&file);
        assert_eq!(counts.string_count, 0);
        assert_eq!(counts.skipped_untranslated, 1);
        let out = compile_mo(&file).unwrap();
        let n = read_u32_le(&out, 8);
        assert_eq!(n, 0);
    }
}
