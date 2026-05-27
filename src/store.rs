use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::RwLock;

/// Represents a single translation entry in a PO file
#[derive(Debug, Clone, Default)]
pub struct MessageEntry {
    /// Message ID (singular form, typically English)
    pub msgid: String,
    /// Optional context for disambiguation
    pub msgctxt: Option<String>,
    /// Translated string (singular)
    pub msgstr: String,
    /// Plural form (if message has plurals)
    pub msgid_plural: Option<String>,
    /// Plural translations [0]=singular, [1]=plural, etc
    pub msgstr_plural: Vec<String>,
    /// Developer comments (lines starting with #.)
    pub extracted_comment: Vec<String>,
    /// Translator comments (lines starting with #)
    pub translator_comment: Vec<String>,
    /// Source locations (lines starting with #:)
    pub source_locations: Vec<String>,
    /// Flags like fuzzy, c-format, python-format (lines starting with #,)
    pub flags: Vec<String>,
    /// Previous untranslated string (lines starting with #|)
    pub previous_msgid: Option<String>,
}

impl MessageEntry {
    /// Check if this entry has the fuzzy flag
    pub fn is_fuzzy(&self) -> bool {
        self.flags.contains(&"fuzzy".to_string())
    }

    /// Check if this message is fully translated (has msgstr and is not fuzzy)
    ///
    /// Per GNU gettext semantics, fuzzy entries are not considered translated
    /// because the gettext runtime ignores them.
    pub fn is_translated(&self) -> bool {
        if self.is_fuzzy() {
            return false;
        }
        if self.msgid_plural.is_some() {
            // Plural entries: check that all plural forms are filled
            !self.msgstr_plural.is_empty()
                && self.msgstr_plural.iter().all(|s| !s.is_empty())
        } else {
            // Singular entries: check msgstr
            !self.msgstr.is_empty()
        }
    }

}

/// Represents an entire PO file
#[derive(Debug, Clone)]
pub struct GettextFile {
    /// All translation entries (including header at key "")
    pub entries: IndexMap<(String, Option<String>), MessageEntry>,
    /// File metadata from header entry (msgid "")
    pub metadata: IndexMap<String, String>,
    /// Raw obsolete entry lines (#~ ...) preserved for round-trip fidelity
    pub obsolete_lines: Vec<String>,
}

impl GettextFile {
    /// Create a new empty PO file
    pub fn new() -> Self {
        Self {
            entries: IndexMap::new(),
            metadata: IndexMap::new(),
            obsolete_lines: Vec::new(),
        }
    }

    /// Get plural rules from metadata, if present
    pub fn plural_forms(&self) -> Option<String> {
        self.metadata.get("Plural-Forms").cloned()
    }

    /// Get language from metadata, if present
    pub fn language(&self) -> Option<String> {
        self.metadata.get("Language").cloned()
    }

    /// Rebuild the header entry msgstr from the current metadata map
    fn rebuild_header_entry(&mut self) {
        let mut header_str = String::new();
        for (k, v) in self.metadata.iter() {
            header_str.push_str(&format!("{}: {}\n", k, v));
        }
        let header_key = (String::new(), None);
        if let Some(entry) = self.entries.get_mut(&header_key) {
            entry.msgstr = header_str;
        } else {
            self.entries.insert(header_key, MessageEntry {
                msgstr: header_str,
                ..Default::default()
            });
        }
    }
}

impl Default for GettextFile {
    fn default() -> Self {
        Self::new()
    }
}

/// Parser for PO format files
pub mod parser {
    use super::*;

    #[derive(Debug, Clone)]
    enum LineType {
        TranslatorComment,   // #
        ExtractedComment,    // #.
        SourceLocation,      // #:
        Flags,               // #,
        PreviousMsgid,       // #|
        Obsolete,            // #~
        Msgctxt,             // msgctxt
        Msgid,               // msgid
        MsgidPlural,         // msgid_plural
        Msgstr,              // msgstr or msgstr[n]
        Continuation,        // "..." (multi-line string continuation)
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

    fn parse_string_literal(line: &str) -> String {
        // Remove msgid/msgstr prefix and surrounding quotes
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

    fn unescape_po_string(s: &str) -> String {
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

    /// Parse a PO file from string content
    pub fn parse_po(content: &str) -> Result<GettextFile, ParseError> {
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
                                .filter(|s| !s.is_empty())
                        );
                    }
                }
                LineType::PreviousMsgid => {
                    current_entry.previous_msgid = Some(parse_string_literal(line));
                }
                LineType::Obsolete => {
                    // Preserve obsolete lines for round-trip fidelity
                    file.obsolete_lines.push(line.to_string());
                }
                LineType::Msgctxt => {
                    if in_entry {
                        // Save previous entry (including header with empty msgid)
                        if !current_entry.msgid.is_empty() || !current_entry.msgstr.is_empty() {
                            let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());
                            file.entries.insert(key, current_entry.clone());
                        }
                        current_entry = MessageEntry::default();
                    }
                    in_entry = true;
                    current_field = Some(CurrentField::Msgctxt);
                    current_entry.msgctxt = Some(parse_string_literal(line));
                }
                LineType::Msgid => {
                    if in_entry && (!current_entry.msgid.is_empty() || !current_entry.msgstr.is_empty()) {
                        // Save previous entry (including header with empty msgid)
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

                    // Check if this is msgstr[n] format
                    if line.contains("msgstr[") {
                        // Plural form
                        if let Some(start) = line.find('[') {
                            if let Some(end) = line.find(']') {
                                if let Ok(index) = line[start + 1..end].parse::<usize>() {
                                    // Validate index is sequential (0, 1, 2, ...)
                                    if index != current_entry.msgstr_plural.len() {
                                        return Err(ParseError::InvalidFormat(
                                            format!("Plural form indices must be sequential. Expected {}, got {}",
                                                current_entry.msgstr_plural.len(), index)
                                        ));
                                    }
                                    current_entry.msgstr_plural.push(value);
                                    current_field = Some(CurrentField::MsgstrPlural(index));
                                }
                            }
                        }
                    } else {
                        // Singular form
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
                        Some(CurrentField::MsgstrPlural(index)) => {
                            if *index < current_entry.msgstr_plural.len() {
                                current_entry.msgstr_plural[*index].push_str(&continuation);
                            }
                        }
                        Some(CurrentField::Msgid) => {
                            current_entry.msgid.push_str(&continuation);
                        }
                        Some(CurrentField::MsgidPlural) => {
                            if let Some(ref mut plural) = &mut current_entry.msgid_plural {
                                plural.push_str(&continuation);
                            }
                        }
                        Some(CurrentField::Msgctxt) => {
                            if let Some(ref mut ctx) = &mut current_entry.msgctxt {
                                ctx.push_str(&continuation);
                            }
                        }
                        None => {}
                    }
                }
                LineType::Blank => {
                    if in_entry {
                        let key = (current_entry.msgid.clone(), current_entry.msgctxt.clone());

                        // Store in metadata if this is the header
                        // Header is identified by empty msgid AND no context
                        if current_entry.msgid.is_empty() && current_entry.msgctxt.is_none() {
                            for line in current_entry.msgstr.lines() {
                                if let Some((k, value)) = line.split_once(':') {
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

        // Save last entry
        if in_entry {
            // Extract metadata if this is the header entry
            if current_entry.msgid.is_empty() && current_entry.msgctxt.is_none() {
                for line in current_entry.msgstr.lines() {
                    if let Some((k, value)) = line.split_once(':') {
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

    /// Serialize a PO file back to string format
    fn escape_po_string(s: &str) -> String {
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

    /// Emit `keyword "value"` (or the multi-line form for values containing
    /// newlines, as conventional gettext tools produce). The multi-line form
    /// is required for round-trip readability of headers like
    /// `msgstr ""` followed by one continuation line per `Key: Value\n` entry.
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

    pub fn serialize_po(file: &GettextFile) -> String {
        let mut output = String::new();

        // Write entries in order
        for ((msgid, msgctxt), entry) in &file.entries {
            // Write comments (sanitize newlines to prevent PO format corruption)
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

            // Write message
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

        // Append preserved obsolete entries
        if !file.obsolete_lines.is_empty() {
            for line in &file.obsolete_lines {
                output.push_str(line);
                output.push('\n');
            }
            output.push('\n');
        }

        output
    }
}

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Invalid PO format: {0}")]
    InvalidFormat(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Translation not found for key: {key}, context: {context:?}")]
    TranslationNotFound {
        key: String,
        context: Option<String>,
    },
    #[error("Path required for dynamic mode")]
    PathRequired,
    #[error("Parse error: {0}")]
    ParseError(#[from] ParseError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("File is locked by another process: {path}")]
    FileLocked { path: PathBuf },
}

/// Prefix used for temporary files created during atomic writes.
const TMP_FILE_PREFIX: &str = ".gettext-mcp-";
/// Suffix used for temporary files created during atomic writes.
const TMP_FILE_SUFFIX: &str = ".tmp";

/// Strip a leading UTF-8 BOM (`\u{feff}`) if present.
fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

/// Best-effort cleanup of orphan `.gettext-mcp-*.tmp` files left over from
/// crashed writes. Scans the given directory non-recursively.
pub fn cleanup_orphan_tmps(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(TMP_FILE_PREFIX) && name_str.ends_with(TMP_FILE_SUFFIX) {
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!("cleaned up orphan temp file: {}", path.display()),
                Err(e) => tracing::warn!(
                    "failed to remove orphan temp file {}: {}",
                    path.display(),
                    e
                ),
            }
        }
    }
}

/// Atomically write `content` to `target` using a temp-file + fsync + rename
/// sequence. On Unix, acquires a best-effort `flock(LOCK_EX | LOCK_NB)` on
/// the existing target for the duration of the write. If the lock is held by
/// another process, returns [`StoreError::FileLocked`].
fn atomic_write(target: &std::path::Path, content: &str) -> Result<(), StoreError> {
    use std::io::Write;

    let dir = target.parent().ok_or_else(|| {
        StoreError::InvalidPath(format!(
            "no parent directory for target path: {}",
            target.display()
        ))
    })?;

    // Acquire advisory lock on existing target file (best-effort).
    let _lock_guard = acquire_flock(target)?;

    // Process-wide monotonic counter so concurrent writers in the same
    // process can't collide on the millisecond resolution of SystemTime.
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_name = format!(
        "{}{}-{}-{}{}",
        TMP_FILE_PREFIX,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        seq,
        TMP_FILE_SUFFIX,
    );
    let tmp_path = dir.join(&tmp_name);

    let result = (|| -> Result<(), StoreError> {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        // `rename` is atomic on POSIX when source and target are on the same
        // filesystem (we always write the temp file in the same directory).
        std::fs::rename(&tmp_path, target)?;
        Ok(())
    })();

    if result.is_err() {
        // Best-effort cleanup of the temp file when the write/rename fails.
        let _ = std::fs::remove_file(&tmp_path);
    }

    result
}

/// RAII guard for an `flock`-acquired file descriptor on Unix. The lock is
/// released automatically when the guard is dropped (because the kernel
/// releases the lock when the underlying fd is closed).
#[cfg(unix)]
struct FlockGuard {
    _file: std::fs::File,
}

#[cfg(unix)]
fn acquire_flock(target: &std::path::Path) -> Result<Option<FlockGuard>, StoreError> {
    use std::os::unix::io::AsRawFd;

    if !target.exists() {
        // Nothing to lock yet — skip best-effort lock.
        return Ok(None);
    }

    let file = match std::fs::File::open(target) {
        Ok(f) => f,
        Err(e) => {
            // If we can't even open it (race with deletion etc.), skip the
            // lock rather than fail the write.
            tracing::warn!(
                "could not open {} for advisory lock: {} — proceeding without lock",
                target.display(),
                e
            );
            return Ok(None);
        }
    };

    let fd = file.as_raw_fd();
    // SAFETY: `flock` is a POSIX syscall; `fd` is valid because `file` is alive.
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.kind() == std::io::ErrorKind::WouldBlock {
            return Err(StoreError::FileLocked {
                path: target.to_path_buf(),
            });
        }
        // Lock not supported on this fs (e.g. NFS without lockd) — proceed.
        tracing::warn!(
            "advisory flock unavailable for {}: {} — proceeding without lock",
            target.display(),
            errno
        );
        return Ok(None);
    }

    Ok(Some(FlockGuard { _file: file }))
}

#[cfg(not(unix))]
struct FlockGuard;

#[cfg(not(unix))]
fn acquire_flock(_target: &std::path::Path) -> Result<Option<FlockGuard>, StoreError> {
    // No advisory locking outside Unix — caller proceeds without a lock.
    Ok(None)
}

/// Store for a single PO file
pub struct GettextStore {
    path: PathBuf,
    data: Arc<RwLock<GettextFile>>,
    /// The on-disk mtime captured the last time this store read or wrote the
    /// file. Used by [`GettextStoreManager`] to detect external modifications
    /// (e.g. Poedit, msgmerge, manual edits) so the cache can be invalidated
    /// instead of serving stale content. `None` means we never observed a
    /// modified time (file did not exist at load) — in that case the cache
    /// will reload on the next access if the file has since appeared.
    loaded_mtime: Arc<RwLock<Option<SystemTime>>>,
}

/// Best-effort `fs::metadata(path).modified()`. Returns `None` when the file
/// is missing or the platform does not report a mtime — callers treat that as
/// "no observable change", which keeps the cache valid rather than panicking.
fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

impl GettextStore {
    /// Create a new store for a PO file path
    pub async fn new(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();

        // Try to load existing file, or create new empty one
        let (data, loaded_mtime) = if path.exists() {
            let content = tokio::fs::read_to_string(&path).await?;
            // Strip leading UTF-8 BOM if present so the parser doesn't trip on it.
            let parsed = parser::parse_po(strip_bom(&content))?;
            // Capture mtime AFTER reading so a concurrent writer that lands
            // between read and stat is detected on the next access (the
            // recorded mtime will be older than what's on disk).
            (parsed, file_mtime(&path))
        } else {
            (GettextFile::new(), None)
        };

        Ok(Self {
            path,
            data: Arc::new(RwLock::new(data)),
            loaded_mtime: Arc::new(RwLock::new(loaded_mtime)),
        })
    }

    /// Return the mtime this store observed the last time it read or wrote
    /// its backing file, if known.
    pub async fn loaded_mtime(&self) -> Option<SystemTime> {
        *self.loaded_mtime.read().await
    }

    /// Get a translation entry
    pub async fn get(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
    ) -> Result<MessageEntry, StoreError> {
        let data = self.data.read().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        data.entries
            .get(&key)
            .cloned()
            .ok_or_else(|| StoreError::TranslationNotFound {
                key: msgid.to_string(),
                context: msgctxt.map(|s| s.to_string()),
            })
    }

    /// List all entries (excluding the header entry with empty msgid)
    pub async fn list_all(&self) -> Result<Vec<(String, Option<String>, MessageEntry)>, StoreError> {
        let data = self.data.read().await;
        Ok(data
            .entries
            .iter()
            .filter(|((msgid, msgctxt), _)| !(msgid.is_empty() && msgctxt.is_none()))
            .map(|((msgid, msgctxt), entry)| (msgid.clone(), msgctxt.clone(), entry.clone()))
            .collect())
    }

    /// Search entries by msgid or translation content
    pub async fn search(&self, query: &str, limit: Option<usize>) -> Result<Vec<MessageEntry>, StoreError> {
        let data = self.data.read().await;
        let query_lower = query.to_lowercase();

        let mut results: Vec<_> = data
            .entries
            .iter()
            .filter(|((msgid, _), entry)| {
                msgid.to_lowercase().contains(&query_lower)
                    || entry.msgstr.to_lowercase().contains(&query_lower)
            })
            .map(|(_, entry)| entry.clone())
            .collect();

        if let Some(limit) = limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Upsert (create or update) a translation entry
    pub async fn upsert(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
        msgstr: &str,
        flags: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        let entry = data.entries.entry(key).or_insert_with(|| MessageEntry {
            msgid: msgid.to_string(),
            msgctxt: msgctxt.map(|s| s.to_string()),
            ..Default::default()
        });

        entry.msgstr = msgstr.to_string();
        if let Some(flags) = flags {
            entry.flags = flags;
        }

        self.persist(&data).await?;
        Ok(())
    }

    /// Delete a specific translation
    pub async fn delete(&self, msgid: &str, msgctxt: Option<&str>) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        if data.entries.shift_remove(&key).is_none() {
            return Err(StoreError::TranslationNotFound {
                key: msgid.to_string(),
                context: msgctxt.map(|s| s.to_string()),
            });
        }
        self.persist(&data).await?;
        Ok(())
    }

    /// Delete all entries matching a msgid (across all contexts) atomically
    pub async fn delete_by_msgid(&self, msgid: &str) -> Result<usize, StoreError> {
        let mut data = self.data.write().await;
        let keys_to_remove: Vec<_> = data
            .entries
            .keys()
            .filter(|(id, _)| id == msgid)
            .cloned()
            .collect();

        let count = keys_to_remove.len();
        if count == 0 {
            return Err(StoreError::TranslationNotFound {
                key: msgid.to_string(),
                context: None,
            });
        }

        for key in keys_to_remove {
            data.entries.shift_remove(&key);
        }

        self.persist(&data).await?;
        Ok(count)
    }

    /// Persist changes to disk.
    ///
    /// Writes are atomic: the serialized content is first written to a
    /// sibling temp file (`.gettext-mcp-<pid>-<unix_millis>.tmp`), fsynced,
    /// and then renamed over the target. On Unix, the existing target is
    /// flocked for the duration of the write to prevent concurrent writers
    /// in the same process tree.
    async fn persist(&self, data: &GettextFile) -> Result<(), StoreError> {
        let content = parser::serialize_po(data);
        let path = self.path.clone();
        // Run the blocking file I/O on a dedicated thread so we don't stall
        // the async runtime.
        tokio::task::spawn_blocking(move || atomic_write(&path, &content))
            .await
            .map_err(|e| StoreError::IoError(std::io::Error::other(e)))??;
        // Refresh our recorded mtime to what we just wrote. This prevents the
        // `GettextStoreManager` staleness check from re-reading the file we
        // ourselves just authored. If the platform somehow refuses to stat
        // the file we just wrote, fall back to `now()` so the cache still
        // sees a fresh-looking timestamp rather than `None` (which would be
        // treated as "no recorded mtime" and risk a spurious reload).
        let new_mtime = file_mtime(&self.path).unwrap_or_else(SystemTime::now);
        *self.loaded_mtime.write().await = Some(new_mtime);
        Ok(())
    }

    /// Get file metadata
    pub async fn metadata(&self) -> Result<IndexMap<String, String>, StoreError> {
        let data = self.data.read().await;
        Ok(data.metadata.clone())
    }

    /// Get language from metadata
    pub async fn language(&self) -> Result<Option<String>, StoreError> {
        let data = self.data.read().await;
        Ok(data.language())
    }


    /// Update or set a metadata header value
    pub async fn set_header(&self, key: &str, value: &str) -> Result<(), StoreError> {
        if key.is_empty() || key.trim().is_empty() {
            return Err(StoreError::InvalidInput("Header key must not be empty".into()));
        }
        if key.contains('\n') || key.contains('\r') {
            return Err(StoreError::InvalidInput("Header key must not contain newlines".into()));
        }
        if key.contains(':') {
            return Err(StoreError::InvalidInput("Header key must not contain colons".into()));
        }
        if value.contains('\n') || value.contains('\r') {
            return Err(StoreError::InvalidInput("Header value must not contain newlines".into()));
        }
        let mut data = self.data.write().await;
        data.metadata.insert(key.to_string(), value.to_string());
        data.rebuild_header_entry();
        self.persist(&data).await?;
        Ok(())
    }

    /// Remove a metadata header key
    pub async fn remove_header(&self, key: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        data.metadata.shift_remove(key);
        data.rebuild_header_entry();
        self.persist(&data).await?;
        Ok(())
    }

    /// Extended upsert that handles plural forms
    pub async fn upsert_full(
        &self,
        msgid: &str,
        msgctxt: Option<&str>,
        msgstr: &str,
        msgid_plural: Option<&str>,
        msgstr_plural: Option<Vec<String>>,
        flags: Option<Vec<String>>,
    ) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));

        // Validate flags before mutating entry to avoid leaving corrupted in-memory state on error
        if let Some(ref flags) = flags {
            for flag in flags {
                if flag.is_empty() || !flag.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    return Err(StoreError::InvalidInput(
                        format!("Invalid flag '{}': only alphanumeric characters, hyphens, and underscores are allowed", flag)
                    ));
                }
            }
        }

        let entry = data.entries.entry(key).or_insert_with(|| MessageEntry {
            msgid: msgid.to_string(),
            msgctxt: msgctxt.map(|s| s.to_string()),
            ..Default::default()
        });

        entry.msgstr = msgstr.to_string();
        if let Some(plural) = msgid_plural {
            entry.msgid_plural = Some(plural.to_string());
            if let Some(plurals) = msgstr_plural {
                entry.msgstr_plural = plurals;
            } else {
                entry.msgstr_plural = Vec::new();
            }
        }
        // When msgid_plural is None, leave existing plural data untouched
        if let Some(flags) = flags {
            entry.flags = flags;
        }

        self.persist(&data).await?;
        Ok(())
    }

    /// Update an entry while preserving all metadata
    pub async fn update_entry(&self, msgid: &str, msgctxt: Option<&str>, mut entry: MessageEntry) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        let key = (msgid.to_string(), msgctxt.map(|s| s.to_string()));
        // Ensure entry fields match the key to prevent inconsistency
        entry.msgid = msgid.to_string();
        entry.msgctxt = msgctxt.map(|s| s.to_string());
        data.entries.insert(key, entry);
        self.persist(&data).await?;
        Ok(())
    }

    /// List all languages from metadata
    pub async fn list_languages(&self) -> Result<Vec<String>, StoreError> {
        let data = self.data.read().await;
        let mut languages = Vec::new();

        // Extract language from metadata
        if let Some(lang) = data.language() {
            languages.push(lang);
        }

        // PO files typically store just one language per file
        // If there are multiple languages, they would be in metadata
        Ok(languages)
    }

    /// Add a new language (updates metadata)
    pub async fn add_language(&self, language: &str) -> Result<(), StoreError> {
        self.set_header("Language", language).await
    }

    /// Remove a language (in single-language PO files, this just clears metadata)
    pub async fn remove_language(&self, language: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        // Only remove if the current language matches what was requested
        if data.metadata.get("Language").map(|l| l.as_str()) == Some(language) {
            data.metadata.shift_remove("Language");
            data.rebuild_header_entry();
            self.persist(&data).await?;
            Ok(())
        } else {
            Err(StoreError::InvalidInput(
                format!("Language '{}' does not match current file language", language)
            ))
        }
    }
}

/// Store manager for multiple PO files
pub struct GettextStoreManager {
    default_path: Option<PathBuf>,
    stores: Arc<RwLock<indexmap::IndexMap<PathBuf, Arc<GettextStore>>>>,
}

impl GettextStoreManager {
    /// Create a new store manager
    pub fn new(default_path: Option<PathBuf>) -> Self {
        Self {
            default_path,
            stores: Arc::new(RwLock::new(indexmap::IndexMap::new())),
        }
    }

    /// Scan the default path for `.po`/`.pot` files and pre-load them.
    /// Call this after construction when the default path is a directory.
    pub async fn scan_directory(&self) -> Result<usize, StoreError> {
        let dir = match self.default_path {
            Some(ref p) if p.is_dir() => p.clone(),
            _ => return Ok(0),
        };

        let po_files = Self::find_po_files(&dir).await?;
        let count = po_files.len();

        let mut stores = self.stores.write().await;
        for file_path in po_files {
            if !stores.contains_key(&file_path) {
                let store = Arc::new(GettextStore::new(&file_path).await?);
                stores.insert(file_path, store);
            }
        }

        Ok(count)
    }

    /// Recursively find all `.po` and `.pot` files under a directory.
    async fn find_po_files(dir: &std::path::Path) -> Result<Vec<PathBuf>, StoreError> {
        let mut result = Vec::new();
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&current).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(ext) = path.extension() {
                    if ext == "po" || ext == "pot" {
                        result.push(path);
                    }
                }
            }
        }

        result.sort();
        Ok(result)
    }

    /// Get or create a store for a path
    pub async fn store_for(&self, path: Option<&str>) -> Result<Arc<GettextStore>, StoreError> {
        let path_buf = if let Some(p) = path {
            let pb = PathBuf::from(p);
            // If the path is relative and we have a directory base, resolve against it
            if pb.is_relative() {
                if let Some(ref base) = self.default_path {
                    if base.is_dir() {
                        let resolved = base.join(&pb);
                        self.validate_path(&resolved)?;
                        resolved
                    } else {
                        self.validate_path(&pb)?;
                        pb
                    }
                } else {
                    self.validate_path(&pb)?;
                    pb
                }
            } else {
                self.validate_path(&pb)?;
                pb
            }
        } else if let Some(ref p) = self.default_path {
            if p.is_dir() {
                return Err(StoreError::PathRequired);
            }
            p.clone()
        } else {
            return Err(StoreError::PathRequired);
        };

        // Fast path: read lock for cache hits
        {
            let stores = self.stores.read().await;
            if let Some(store) = stores.get(&path_buf) {
                // Check whether the file has been modified out from under us
                // (e.g. Poedit, msgmerge, manual edit). If the on-disk mtime
                // differs from what the cached store recorded, drop the read
                // lock and fall through to the slow path to reload. If the
                // file is gone or stat fails, keep serving the cached copy
                // — the next persist will surface any real error.
                let current_mtime = file_mtime(&path_buf);
                let cached_mtime = store.loaded_mtime().await;
                let stale = match (current_mtime, cached_mtime) {
                    (Some(current), Some(cached)) => current != cached,
                    // First-time observation of an mtime (cached has None but
                    // the file now exists): treat as stale so we reload from
                    // disk and capture the mtime.
                    (Some(_), None) => true,
                    // File missing or stat failed: keep the cached copy.
                    (None, _) => false,
                };
                if !stale {
                    return Ok(Arc::clone(store));
                }
            }
        }

        // Slow path: write lock for cache misses OR stale invalidation.
        let mut stores = self.stores.write().await;
        // Re-check staleness after acquiring write lock — another task may
        // have already reloaded between our read-lock drop and write-lock
        // acquire, in which case we should reuse their fresh entry.
        if let Some(store) = stores.get(&path_buf) {
            let current_mtime = file_mtime(&path_buf);
            let cached_mtime = store.loaded_mtime().await;
            let stale = match (current_mtime, cached_mtime) {
                (Some(current), Some(cached)) => current != cached,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if !stale {
                return Ok(Arc::clone(store));
            }
            // Stale: drop the entry so we can replace it with a fresh load.
            stores.shift_remove(&path_buf);
        }

        let store = Arc::new(GettextStore::new(&path_buf).await?);
        stores.insert(path_buf, Arc::clone(&store));
        Ok(store)
    }

    fn validate_path(&self, path: &PathBuf) -> Result<(), StoreError> {
        // Check for path traversal attempts (.. components)
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                return Err(StoreError::InvalidPath("Path traversal not allowed".into()));
            }
        }

        // If a default path is set, ensure the requested path is within its directory
        if let Some(ref default) = self.default_path {
            // Use parent directory as base when default_path points to a file.
            // Use is_dir() to positively identify directories, treating everything
            // else (files and non-existent paths) as file paths.
            let base = if default.is_dir() {
                default.as_path()
            } else {
                default.parent().unwrap_or(default)
            };

            // Canonicalize both paths to resolve symlinks before comparison
            let canonical_base = base.canonicalize().map_err(|e| {
                StoreError::InvalidPath(format!("Cannot resolve base path: {}", e))
            })?;
            let canonical_path = path.canonicalize().or_else(|_| {
                // If file doesn't exist yet, canonicalize parent and append filename
                if let (Some(parent), Some(filename)) = (path.parent(), path.file_name()) {
                    parent
                        .canonicalize()
                        .map(|p| p.join(filename))
                } else {
                    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Cannot resolve path"))
                }
            }).map_err(|e| StoreError::InvalidPath(format!("Cannot resolve path: {}", e)))?;

            if !canonical_path.starts_with(&canonical_base) {
                return Err(StoreError::InvalidPath("Path must be within base directory".into()));
            }
        } else {
            // When no default path is set, reject absolute paths for safety
            if path.is_absolute() {
                return Err(StoreError::InvalidPath(
                    "Absolute paths not allowed without a configured base directory".into(),
                ));
            }
        }

        Ok(())
    }

    /// Get paths of all discovered or loaded .po/.pot files
    pub async fn discovered_paths(&self) -> Vec<PathBuf> {
        let stores = self.stores.read().await;
        stores.keys().cloned().collect()
    }

    /// Return whether the default path is a directory
    pub fn is_directory_mode(&self) -> bool {
        self.default_path.as_ref().map_or(false, |p| p.is_dir())
    }

    /// Return the base directory path, if set and is a directory
    pub fn base_dir(&self) -> Option<&std::path::Path> {
        self.default_path.as_deref().filter(|p| p.is_dir())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_po() {
        let content = r#"
msgid ""
msgstr ""
"Language: en\n"

msgid "Hello"
msgstr "Bonjour"

msgid "World"
msgstr "Monde"
"#;

        let file = parser::parse_po(content).expect("Failed to parse");
        assert_eq!(file.entries.len(), 3); // header + 2 messages

        // Verify actual entry content
        let hello = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(hello.msgstr, "Bonjour");
        let world = file.entries.get(&("World".to_string(), None)).unwrap();
        assert_eq!(world.msgstr, "Monde");

        // Verify metadata extraction
        assert_eq!(file.metadata.get("Language"), Some(&"en".to_string()));
    }

    #[test]
    fn test_parse_with_context() {
        let content = r#"
msgctxt "greeting"
msgid "Hello"
msgstr "Bonjour"

msgctxt "farewell"
msgid "Hello"
msgstr "Adieu"
"#;

        let file = parser::parse_po(content).expect("Failed to parse");
        assert_eq!(file.entries.len(), 2);

        // Verify context values are parsed correctly
        let greeting = file.entries.get(&("Hello".to_string(), Some("greeting".to_string()))).unwrap();
        assert_eq!(greeting.msgstr, "Bonjour");
        let farewell = file.entries.get(&("Hello".to_string(), Some("farewell".to_string()))).unwrap();
        assert_eq!(farewell.msgstr, "Adieu");
    }

    #[test]
    fn test_parse_with_flags() {
        let content = r#"
#, fuzzy, c-format
msgid "Hello %s"
msgstr "Bonjour %s"
"#;

        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.values().next().unwrap();
        assert!(entry.is_fuzzy());
        assert!(entry.flags.contains(&"c-format".to_string()));
    }

    #[test]
    fn test_serialize_po() {
        let mut file = GettextFile::new();
        let entry = MessageEntry {
            msgid: "Hello".to_string(),
            msgctxt: None,
            msgstr: "Bonjour".to_string(),
            msgid_plural: None,
            msgstr_plural: Vec::new(),
            extracted_comment: vec!["A greeting".to_string()],
            translator_comment: Vec::new(),
            source_locations: vec!["main.rs:42".to_string()],
            flags: Vec::new(),
            previous_msgid: None,
        };

        file.entries.insert(("Hello".to_string(), None), entry);
        let serialized = parser::serialize_po(&file);

        assert!(serialized.contains("msgid \"Hello\""));
        assert!(serialized.contains("msgstr \"Bonjour\""));
        assert!(serialized.contains("#. A greeting"));
        assert!(serialized.contains("#: main.rs:42"));
    }

    #[tokio::test]
    async fn test_store_upsert_and_get() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Bonjour");
    }

    #[tokio::test]
    async fn test_store_search() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        let results = store.search("Hello", None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].msgstr, "Bonjour");
    }

    #[tokio::test]
    async fn test_store_get_nonexistent_returns_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.get("nonexistent", None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_store_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("World", None, "Monde", None).await.unwrap();

        store.delete("Hello", None).await.unwrap();

        let result = store.get("Hello", None).await;
        assert!(result.is_err());

        let world = store.get("World", None).await.unwrap();
        assert_eq!(world.msgstr, "Monde");
    }

    #[tokio::test]
    async fn test_store_delete_nonexistent_returns_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.delete("nonexistent", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_delete_by_msgid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Save", Some("menu"), "Enregistrer", None).await.unwrap();
        store.upsert("Save", Some("toolbar"), "Sauvegarder", None).await.unwrap();
        store.upsert("Other", None, "Autre", None).await.unwrap();

        let count = store.delete_by_msgid("Save").await.unwrap();
        assert_eq!(count, 2);

        let result = store.get("Save", Some("menu")).await;
        assert!(result.is_err());

        let other = store.get("Other", None).await.unwrap();
        assert_eq!(other.msgstr, "Autre");
    }

    #[tokio::test]
    async fn test_store_update_entry_preserves_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", Some(vec!["c-format".to_string()])).await.unwrap();

        let mut entry = store.get("Hello", None).await.unwrap();
        entry.translator_comment = vec!["A greeting".to_string()];
        store.update_entry("Hello", None, entry).await.unwrap();

        let updated = store.get("Hello", None).await.unwrap();
        assert_eq!(updated.msgstr, "Bonjour");
        assert_eq!(updated.translator_comment, vec!["A greeting".to_string()]);
        assert!(updated.flags.contains(&"c-format".to_string()));
    }

    #[tokio::test]
    async fn test_set_header_rejects_newlines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.set_header("Bad\nKey", "value").await;
        assert!(result.is_err());

        let result = store.set_header("Key", "bad\nvalue").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_set_header_rejects_colons_in_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.set_header("Bad:Key", "value").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_upsert_full_rejects_invalid_flags() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.upsert_full(
            "Hello", None, "Bonjour", None, None,
            Some(vec!["valid-flag".to_string(), "invalid flag!".to_string()]),
        ).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid flag"));
    }

    #[test]
    fn test_parse_multiline_strings() {
        let content = r#"
msgid ""
"This is a long "
"multiline string"
msgstr ""
"Ceci est une longue "
"chaine multiligne"
"#;

        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("This is a long multiline string".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "Ceci est une longue chaine multiligne");
    }

    #[test]
    fn test_parse_obsolete_entries_not_misclassified() {
        let content = r#"
msgid "Active"
msgstr "Actif"

#~ msgid "Old entry"
#~ msgstr "Ancienne entree"

msgid "Another"
msgstr "Autre"
"#;

        let file = parser::parse_po(content).expect("Failed to parse");
        // Obsolete entries should be skipped, not added as translator comments
        let active = file.entries.get(&("Active".to_string(), None)).unwrap();
        assert!(active.translator_comment.is_empty());
        let another = file.entries.get(&("Another".to_string(), None)).unwrap();
        assert!(another.translator_comment.is_empty());
    }

    #[test]
    fn test_validate_path_rejects_traversal() {
        let manager = GettextStoreManager::new(None);
        let path = PathBuf::from("../etc/passwd");
        let result = manager.validate_path(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_rejects_absolute_without_base() {
        let manager = GettextStoreManager::new(None);
        let path = PathBuf::from("/etc/passwd");
        let result = manager.validate_path(&path);
        assert!(result.is_err());
    }

    // ==================== Parser Edge Cases ====================

    #[test]
    fn test_parse_escape_sequences() {
        let content = r#"
msgid "Line one\nLine two"
msgstr "Ligne un\nLigne deux"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("Line one\nLine two".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "Ligne un\nLigne deux");
    }

    #[test]
    fn test_parse_escaped_quotes() {
        let content = r#"
msgid "She said \"hello\""
msgstr "Elle a dit \"bonjour\""
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("She said \"hello\"".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "Elle a dit \"bonjour\"");
    }

    #[test]
    fn test_parse_escaped_backslash() {
        let content = r#"
msgid "path\\to\\file"
msgstr "chemin\\vers\\fichier"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("path\\to\\file".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "chemin\\vers\\fichier");
    }

    #[test]
    fn test_parse_tabs() {
        let content = "msgid \"col1\\tcol2\"\nmsgstr \"col1\\tcol2\"\n";
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("col1\tcol2".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "col1\tcol2");
    }

    #[test]
    fn test_parse_plural_forms() {
        let content = r#"
msgid "%d item"
msgid_plural "%d items"
msgstr[0] "%d élément"
msgstr[1] "%d éléments"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("%d item".to_string(), None)).unwrap();
        assert_eq!(entry.msgid_plural, Some("%d items".to_string()));
        assert_eq!(entry.msgstr_plural.len(), 2);
        assert_eq!(entry.msgstr_plural[0], "%d élément");
        assert_eq!(entry.msgstr_plural[1], "%d éléments");
    }

    #[test]
    fn test_parse_three_plural_forms() {
        // Some languages like Polish have 3 plural forms
        let content = r#"
msgid "%d file"
msgid_plural "%d files"
msgstr[0] "%d plik"
msgstr[1] "%d pliki"
msgstr[2] "%d plików"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("%d file".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr_plural.len(), 3);
        assert_eq!(entry.msgstr_plural[2], "%d plików");
    }

    #[test]
    fn test_parse_non_sequential_plural_index_fails() {
        let content = r#"
msgid "%d item"
msgid_plural "%d items"
msgstr[0] "zero"
msgstr[2] "skipped one"
"#;
        let result = parser::parse_po(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_extracted_comments() {
        let content = r#"
#. This is an extracted comment
#. Second line of extracted comment
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.extracted_comment.len(), 2);
        assert_eq!(entry.extracted_comment[0], "This is an extracted comment");
        assert_eq!(entry.extracted_comment[1], "Second line of extracted comment");
    }

    #[test]
    fn test_parse_source_locations() {
        let content = r#"
#: src/main.rs:42
#: src/lib.rs:100
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.source_locations.len(), 2);
        assert_eq!(entry.source_locations[0], "src/main.rs:42");
        assert_eq!(entry.source_locations[1], "src/lib.rs:100");
    }

    #[test]
    fn test_parse_previous_msgid() {
        let content = r#"
#| msgid "Old hello"
#, fuzzy
msgid "Hello"
msgstr "Bonjour"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.previous_msgid, Some("Old hello".to_string()));
        assert!(entry.is_fuzzy());
    }

    #[test]
    fn test_parse_empty_file() {
        let file = parser::parse_po("").expect("Failed to parse");
        assert!(file.entries.is_empty());
        assert!(file.metadata.is_empty());
    }

    #[test]
    fn test_parse_header_only() {
        let content = r#"
msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Language: fr\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        assert_eq!(file.metadata.get("Language"), Some(&"fr".to_string()));
        assert_eq!(file.metadata.get("Plural-Forms"), Some(&"nplurals=2; plural=(n != 1);".to_string()));
    }

    #[test]
    fn test_parse_multiple_flags_on_one_line() {
        let content = r#"
#, fuzzy, c-format, python-format
msgid "Hello %s"
msgstr "Bonjour %s"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        let entry = file.entries.values().next().unwrap();
        assert!(entry.flags.contains(&"fuzzy".to_string()));
        assert!(entry.flags.contains(&"c-format".to_string()));
        assert!(entry.flags.contains(&"python-format".to_string()));
    }

    #[test]
    fn test_parse_obsolete_lines_preserved() {
        let content = r#"
msgid "Active"
msgstr "Actif"

#~ msgid "Removed"
#~ msgstr "Supprimé"
"#;
        let file = parser::parse_po(content).expect("Failed to parse");
        assert_eq!(file.obsolete_lines.len(), 2);
        assert!(file.obsolete_lines[0].contains("Removed"));
        assert!(file.obsolete_lines[1].contains("Supprimé"));
    }

    // ==================== Serialization Round-Trip ====================

    #[test]
    fn test_serialize_roundtrip_with_escapes() {
        let mut file = GettextFile::new();
        file.entries.insert(("Line one\nLine two".to_string(), None), MessageEntry {
            msgid: "Line one\nLine two".to_string(),
            msgstr: "Ligne un\nLigne deux".to_string(),
            ..Default::default()
        });

        let serialized = parser::serialize_po(&file);
        // Strings with embedded newlines use the conventional multi-line form:
        //     msgid ""
        //     "Line one\n"
        //     "Line two"
        assert!(serialized.contains("msgid \"\"\n\"Line one\\n\"\n\"Line two\""));
        assert!(serialized.contains("msgstr \"\"\n\"Ligne un\\n\"\n\"Ligne deux\""));

        // Parse back and verify the value round-trips intact
        let reparsed = parser::parse_po(&serialized).expect("Failed to reparse");
        let entry = reparsed.entries.get(&("Line one\nLine two".to_string(), None)).unwrap();
        assert_eq!(entry.msgstr, "Ligne un\nLigne deux");
    }

    #[test]
    fn test_serialize_roundtrip_with_plurals() {
        let mut file = GettextFile::new();
        file.entries.insert(("%d item".to_string(), None), MessageEntry {
            msgid: "%d item".to_string(),
            msgid_plural: Some("%d items".to_string()),
            msgstr_plural: vec!["%d élément".to_string(), "%d éléments".to_string()],
            ..Default::default()
        });

        let serialized = parser::serialize_po(&file);
        let reparsed = parser::parse_po(&serialized).expect("Failed to reparse");
        let entry = reparsed.entries.get(&("%d item".to_string(), None)).unwrap();
        assert_eq!(entry.msgid_plural, Some("%d items".to_string()));
        assert_eq!(entry.msgstr_plural.len(), 2);
    }

    #[test]
    fn test_serialize_roundtrip_with_context() {
        let mut file = GettextFile::new();
        file.entries.insert(("OK".to_string(), Some("button".to_string())), MessageEntry {
            msgid: "OK".to_string(),
            msgctxt: Some("button".to_string()),
            msgstr: "D'accord".to_string(),
            ..Default::default()
        });

        let serialized = parser::serialize_po(&file);
        assert!(serialized.contains("msgctxt \"button\""));

        let reparsed = parser::parse_po(&serialized).expect("Failed to reparse");
        let entry = reparsed.entries.get(&("OK".to_string(), Some("button".to_string()))).unwrap();
        assert_eq!(entry.msgstr, "D'accord");
    }

    #[test]
    fn test_serialize_roundtrip_with_all_comment_types() {
        let mut file = GettextFile::new();
        file.entries.insert(("Hello".to_string(), None), MessageEntry {
            msgid: "Hello".to_string(),
            msgstr: "Bonjour".to_string(),
            extracted_comment: vec!["Extracted note".to_string()],
            translator_comment: vec!["Translator note".to_string()],
            source_locations: vec!["app.rs:10".to_string()],
            flags: vec!["fuzzy".to_string(), "c-format".to_string()],
            previous_msgid: Some("Old Hello".to_string()),
            ..Default::default()
        });

        let serialized = parser::serialize_po(&file);
        assert!(serialized.contains("# Translator note"));
        assert!(serialized.contains("#. Extracted note"));
        assert!(serialized.contains("#: app.rs:10"));
        assert!(serialized.contains("#, fuzzy, c-format"));
        assert!(serialized.contains("#| msgid \"Old Hello\""));

        let reparsed = parser::parse_po(&serialized).expect("Failed to reparse");
        let entry = reparsed.entries.get(&("Hello".to_string(), None)).unwrap();
        assert_eq!(entry.extracted_comment, vec!["Extracted note"]);
        assert_eq!(entry.translator_comment, vec!["Translator note"]);
        assert_eq!(entry.source_locations, vec!["app.rs:10"]);
        assert!(entry.flags.contains(&"fuzzy".to_string()));
        assert!(entry.flags.contains(&"c-format".to_string()));
        assert_eq!(entry.previous_msgid, Some("Old Hello".to_string()));
    }

    #[test]
    fn test_serialize_obsolete_lines_preserved() {
        let mut file = GettextFile::new();
        file.entries.insert(("Active".to_string(), None), MessageEntry {
            msgid: "Active".to_string(),
            msgstr: "Actif".to_string(),
            ..Default::default()
        });
        file.obsolete_lines = vec![
            "#~ msgid \"Old\"".to_string(),
            "#~ msgstr \"Ancien\"".to_string(),
        ];

        let serialized = parser::serialize_po(&file);
        assert!(serialized.contains("#~ msgid \"Old\""));
        assert!(serialized.contains("#~ msgstr \"Ancien\""));
    }

    #[test]
    fn test_full_header_metadata_roundtrip() {
        // Real-world Crowdin-managed Swedish PO header. Verifies every key is
        // parsed, the original insertion order is kept, the multi-line
        // serialization form is used, and the file is byte-stable across a
        // second serialize/parse cycle.
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

        let parsed = parser::parse_po(input).expect("parse");
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

        let serialized = parser::serialize_po(&parsed);
        // Multi-line header form must be preserved, not collapsed onto one line.
        assert!(serialized.contains("msgstr \"\"\n\"Language: sv\\n\""));
        assert!(serialized.contains("\"PO-Revision-Date: 2026-04-20 10:51\\n\""));
        assert!(!serialized.contains("Language: sv\\nPlural-Forms"));

        // Re-parsing the serialized output yields the same metadata in the same order.
        let reparsed = parser::parse_po(&serialized).expect("reparse");
        let reparsed_order: Vec<(&str, &str)> = reparsed
            .metadata
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(reparsed_order, expected_order);

        // serialize → parse → serialize is now a fixed point.
        assert_eq!(serialized, parser::serialize_po(&reparsed));
    }

    #[test]
    fn test_arbitrary_headers_roundtrip() {
        // Header storage is an open key/value map: the standard gettext fields,
        // arbitrary `X-*` extensions, and unknown vendor keys must all survive
        // parse + serialize without being dropped, reordered, or normalized.
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

        let parsed = parser::parse_po(input).expect("parse");
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

        // Spot-check awkward values: empty value, embedded colons in value,
        // angle-bracketed email, and a `;`-separated MIME content type.
        assert_eq!(parsed.metadata.get("X-Empty-Value").map(String::as_str), Some(""));
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

        // serialize → parse must be a fixed point in metadata order and values.
        let serialized = parser::serialize_po(&parsed);
        let reparsed = parser::parse_po(&serialized).expect("reparse");
        let reparsed_keys: Vec<&str> = reparsed.metadata.keys().map(String::as_str).collect();
        assert_eq!(reparsed_keys, expected_keys);
        for key in expected_keys {
            assert_eq!(parsed.metadata.get(key), reparsed.metadata.get(key), "key {key}");
        }
    }

    // ==================== Store Edge Cases ====================

    #[tokio::test]
    async fn test_store_upsert_overwrites_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        store.upsert("Hello", None, "Salut", None).await.unwrap();

        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Salut");
    }

    #[tokio::test]
    async fn test_store_set_header_and_metadata() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.set_header("Language", "es").await.unwrap();
        store.set_header("Plural-Forms", "nplurals=2; plural=(n != 1);").await.unwrap();

        let meta = store.metadata().await.unwrap();
        assert_eq!(meta.get("Language"), Some(&"es".to_string()));
        assert_eq!(meta.get("Plural-Forms"), Some(&"nplurals=2; plural=(n != 1);".to_string()));

        let lang = store.language().await.unwrap();
        assert_eq!(lang, Some("es".to_string()));
    }

    #[tokio::test]
    async fn test_store_remove_header() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.set_header("Custom-Key", "custom-value").await.unwrap();

        store.remove_header("Custom-Key").await.unwrap();

        let meta = store.metadata().await.unwrap();
        assert!(meta.get("Custom-Key").is_none());
        assert_eq!(meta.get("Language"), Some(&"fr".to_string()));
    }

    #[tokio::test]
    async fn test_store_add_and_remove_language() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.add_language("ja").await.unwrap();

        let langs = store.list_languages().await.unwrap();
        assert_eq!(langs, vec!["ja".to_string()]);

        store.remove_language("ja").await.unwrap();

        let langs = store.list_languages().await.unwrap();
        assert!(langs.is_empty());
    }

    #[tokio::test]
    async fn test_store_remove_wrong_language_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.add_language("fr").await.unwrap();

        let result = store.remove_language("de").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_set_header_empty_key_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        let result = store.set_header("", "value").await;
        assert!(result.is_err());

        let result = store.set_header("   ", "value").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_is_translated_semantics() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();

        // Empty msgstr -> not translated
        store.upsert("Empty", None, "", None).await.unwrap();
        let entry = store.get("Empty", None).await.unwrap();
        assert!(!entry.is_translated());

        // Has msgstr -> translated
        store.upsert("Full", None, "Complet", None).await.unwrap();
        let entry = store.get("Full", None).await.unwrap();
        assert!(entry.is_translated());

        // Fuzzy with msgstr -> not translated
        store.upsert("Fuzzy", None, "Flou", Some(vec!["fuzzy".to_string()])).await.unwrap();
        let entry = store.get("Fuzzy", None).await.unwrap();
        assert!(!entry.is_translated());
        assert!(entry.is_fuzzy());
    }

    #[tokio::test]
    async fn test_store_search_case_insensitive() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello World", None, "Bonjour Monde", None).await.unwrap();

        let results = store.search("hello", None).await.unwrap();
        assert_eq!(results.len(), 1);

        let results = store.search("MONDE", None).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_store_search_with_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Test A", None, "A", None).await.unwrap();
        store.upsert("Test B", None, "B", None).await.unwrap();
        store.upsert("Test C", None, "C", None).await.unwrap();

        let results = store.search("Test", Some(2)).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_store_manager_path_required_in_dynamic_mode() {
        let manager = GettextStoreManager::new(None);
        let result = manager.store_for(None).await;
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("Path required")),
            Ok(_) => panic!("Expected error"),
        }
    }

    #[tokio::test]
    async fn test_store_upsert_full_with_plurals() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert_full(
            "%d cat", None, "",
            Some("%d cats"),
            Some(vec!["%d chat".to_string(), "%d chats".to_string()]),
            Some(vec!["c-format".to_string()]),
        ).await.unwrap();

        let entry = store.get("%d cat", None).await.unwrap();
        assert_eq!(entry.msgid_plural, Some("%d cats".to_string()));
        assert_eq!(entry.msgstr_plural, vec!["%d chat", "%d chats"]);
        assert!(entry.flags.contains(&"c-format".to_string()));
    }

    #[tokio::test]
    async fn test_store_list_all_excludes_header() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.set_header("Language", "fr").await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        let entries = store.list_all().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "Hello");
    }

    // ==================== Atomic Write / Locking / BOM ====================

    #[tokio::test]
    async fn test_atomic_write_no_orphans() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("clean.po");

        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();
        // Trigger a second write (overwrite path) to exercise rename-over-existing.
        store.upsert("World", None, "Monde", None).await.unwrap();

        // No `.gettext-mcp-*.tmp` files (or any other `.tmp` siblings) should remain.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(TMP_FILE_SUFFIX))
            .collect();
        assert!(
            leftovers.is_empty(),
            "expected no orphan temp files, found: {:?}",
            leftovers.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_flock_blocks_concurrent_write() {
        use std::os::unix::io::AsRawFd;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("locked.po");

        // Create the file with an initial write so the lock target exists.
        let store = GettextStore::new(&path).await.unwrap();
        store.upsert("Hello", None, "Bonjour", None).await.unwrap();

        // Hold an exclusive non-blocking flock on the file.
        let lock_file = std::fs::File::open(&path).unwrap();
        let fd = lock_file.as_raw_fd();
        // SAFETY: `fd` is valid for the lifetime of `lock_file`.
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(ret, 0, "should have acquired lock");

        // Attempt to persist while the file is locked — expect FileLocked.
        let err = store
            .upsert("Hello", None, "Salut", None)
            .await
            .expect_err("write should fail while file is flocked");
        assert!(
            matches!(err, StoreError::FileLocked { .. }),
            "expected FileLocked, got: {err:?}"
        );

        // Release the lock and retry — should succeed.
        // SAFETY: `fd` is valid for the lifetime of `lock_file`.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        drop(lock_file);

        store.upsert("Hello", None, "Salut", None).await.unwrap();
        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Salut");
    }

    #[tokio::test]
    async fn test_bom_stripping() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bom.po");

        // Write a PO file that begins with a UTF-8 BOM.
        let body = "msgid \"\"\nmsgstr \"\"\n\"Language: en\\n\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n";
        let with_bom = format!("\u{feff}{body}");
        std::fs::write(&path, with_bom.as_bytes()).unwrap();

        // Loading should strip the BOM and parse successfully.
        let store = GettextStore::new(&path).await.unwrap();
        let entry = store.get("Hello", None).await.unwrap();
        assert_eq!(entry.msgstr, "Bonjour");

        // The store's in-memory msgid must not contain the BOM.
        assert!(!entry.msgid.contains('\u{feff}'));
        assert_eq!(entry.msgid, "Hello");

        // Quick direct check on the helper for completeness.
        assert_eq!(strip_bom("\u{feff}hi"), "hi");
        assert_eq!(strip_bom("hi"), "hi");
    }

    #[test]
    fn test_cleanup_orphan_tmps_removes_only_matching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let orphan = dir
            .path()
            .join(format!("{TMP_FILE_PREFIX}123-456{TMP_FILE_SUFFIX}"));
        let keep = dir.path().join("messages.po");
        std::fs::write(&orphan, b"junk").unwrap();
        std::fs::write(&keep, b"msgid \"\"\nmsgstr \"\"\n").unwrap();

        cleanup_orphan_tmps(dir.path());

        assert!(!orphan.exists(), "orphan temp file should be removed");
        assert!(keep.exists(), "unrelated .po file must be preserved");
    }

    // ==================== mtime-aware cache ====================

    /// Simulate an external editor (Poedit, msgmerge, manual edit) replacing
    /// the file on disk: the manager must observe the new mtime and reload
    /// rather than serving the previously-cached parse.
    #[tokio::test]
    async fn test_cache_invalidates_on_external_modification() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");

        // Seed the file with an initial entry and prime the cache.
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();
        assert_eq!(store1.get("Hello", None).await.unwrap().msgstr, "Bonjour");

        // Filesystem mtime resolution on some platforms (HFS+, ext3, FAT) is
        // ~1s, so a sub-millisecond rewrite can land on the same mtime as the
        // initial seed and defeat the staleness check. Sleep just long enough
        // to guarantee a distinct mtime, then rewrite the file.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Salut\"\n",
        )
        .unwrap();

        // store_for must observe the new mtime, evict the stale entry, and
        // return a fresh store reflecting the external edit.
        let store2 = manager.store_for(None).await.unwrap();
        assert_eq!(store2.get("Hello", None).await.unwrap().msgstr, "Salut");

        // The two stores should be distinct Arc instances — the cache was
        // genuinely replaced, not patched in place.
        assert!(
            !Arc::ptr_eq(&store1, &store2),
            "stale store should have been evicted, not reused"
        );
    }

    /// When the file on disk has not changed between calls, the manager must
    /// hand back the exact same `Arc<GettextStore>` — no reload, no re-parse,
    /// no fresh Arc allocation.
    #[tokio::test]
    async fn test_cache_serves_unchanged_file_without_rereading() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();
        let store2 = manager.store_for(None).await.unwrap();

        // Same Arc instance — proves the second call short-circuited on the
        // mtime check and never re-parsed the file.
        assert!(
            Arc::ptr_eq(&store1, &store2),
            "expected identical Arc on cache hit with unchanged file"
        );
    }

    /// A write through the store must update the cached mtime so the next
    /// `store_for` does not unnecessarily evict-and-reload the entry we just
    /// authored ourselves.
    #[tokio::test]
    async fn test_cache_survives_internal_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("messages.po");
        std::fs::write(
            &path,
            "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Hello\"\nmsgstr \"Bonjour\"\n",
        )
        .unwrap();

        let manager = GettextStoreManager::new(Some(path.clone()));
        let store1 = manager.store_for(None).await.unwrap();

        // Writing through the cached store bumps the on-disk mtime. The
        // store's recorded loaded_mtime must follow, otherwise the next
        // store_for would mistake our own write for an external edit.
        store1
            .upsert("Greeting", None, "Salutation", None)
            .await
            .unwrap();

        let store2 = manager.store_for(None).await.unwrap();
        assert!(
            Arc::ptr_eq(&store1, &store2),
            "internal write must not invalidate the cache entry"
        );
        assert_eq!(
            store2.get("Greeting", None).await.unwrap().msgstr,
            "Salutation"
        );
    }
}
