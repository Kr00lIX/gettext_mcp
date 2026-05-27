---
name: gettext-mcp
description: >
  Use this skill for ANY task involving GNU gettext localization: translating .po/.pot files,
  adding or updating translations, managing fuzzy entries, handling plural forms (Plural-Forms
  header, msgstr[n]), working with msgctxt disambiguation, updating PO headers, auditing
  translation coverage, reviewing source locations (#:), inspecting obsolete entries (#~),
  or any task touching `messages.po`, `messages.pot`, `Localizable.po`, `<locale>.po`, or
  `.pot` templates. Activate whenever the user mentions gettext, xgettext, msgmerge, msgfmt,
  Poedit, Lokalise, msgid/msgstr/msgctxt, "translate", "add language", "i18n", "l10n",
  "локалізація", "переклади", "fix translations", "fuzzy", or anything related to making
  a project multilingual via the gettext toolchain.
---

# gettext-mcp Skill

## Trigger Conditions

### File Discovery
- any `.po` file mentioned (`messages.po`, `fr.po`, `uk_UA.po`, `Localizable.po`, ...)
- any `.pot` template mentioned (`messages.pot`, `app.pot`, ...)
- `LC_MESSAGES` directory tree mentioned (`locale/<lang>/LC_MESSAGES/*.po`)
- "find all gettext files / find all .po files in project"
- opening or editing any `.po` / `.pot` file

### Translation & Language Management
- "translate app / translate to [language]"
- "add [language] / add Ukrainian / add Spanish locale"
- "create new .po file for [language]"
- "переведи приложение / добавь украинский язык"
- "localize my project / i18n / l10n"
- "fix translations / fix incorrect translation / update existing translation"

### Coverage & Fuzzy Review
- "check translation coverage / how many strings are translated"
- "find untranslated strings / find missing translations"
- "review fuzzy translations / clear fuzzy flag / fix fuzzy entries"
- "translation progress / completion status"
- "audit translations / audit .po file"

### Keys & Content
- "add new translation / add new msgid / add string to .po"
- "rename msgid / delete msgid / remove translation key"
- "search msgid / find translation for / find string in .po"
- "add translator comment / add note for translator"
- "set source location / extracted comment"

### Plural Forms
- "plural forms in localization / pluralization in gettext"
- "Plural-Forms header / nplurals"
- "msgid_plural / msgstr[0] / msgstr[1] / msgstr[n]"
- "CLDR plural rules" applied to gettext
- string with number placeholder needing translation (e.g. "%d items")

### Contexts (msgctxt)
- "disambiguate translation / same word two meanings"
- "msgctxt / message context / Save (menu) vs Save (button)"
- "list contexts in .po file"

### Flags & Metadata
- "set c-format / python-format / no-wrap flag"
- "PO header / Content-Type / Language-Team / Plural-Forms"
- "set fuzzy flag / mark as fuzzy / clear fuzzy"
- "obsolete entries (`#~`) / stale msgids"
- "source locations (`#:`)"

### Toolchain
- `xgettext` mentioned (extracting strings from source code)
- `msgmerge` mentioned (merging .pot into existing .po)
- `msgfmt` mentioned (compiling .po to .mo)
- `msginit` / `msgcat` / `msgattrib` mentioned
- Poedit, Lokalise, Transifex, Weblate, Crowdin mentioned with `.po` context

---

## NEVER DO THIS

```sh
# NEVER read .po files directly — wastes context window, fragile escaping
Read("messages.po")
Bash("cat fr.po")
Bash("grep 'msgid' uk.po")
Bash("head -100 messages.po")

# NEVER edit .po files manually — escaping, multi-line strings, comments all fragile
Edit("messages.po", ...)
Write("messages.po", ...)
Bash("sed -i 's/msgstr \"\"/msgstr \"...\"/' fr.po")

# NEVER search for .po files with bash — use list_files instead
Bash("find . -name '*.po'")
Bash("ls **/*.po")
```

PO files look like plain text, but:

- `msgstr` values may span many physical lines (concatenated string literals)
- Comments (`#`, `#.`, `#:`, `#,`, `#|`, `#~`) each have distinct semantics
- Escapes (`\\`, `\"`, `\n`, `\r`, `\t`) must round-trip exactly
- Obsolete entries (`#~ msgid ...`) look like comments but are not
- Plural forms (`msgstr[0]`, `msgstr[1]`, ...) must match the file's `nplurals`

Manual edits silently corrupt files. **Always use gettext-mcp tools. No exceptions.**

---

## Prerequisites

**If gettext-mcp tools are unavailable**, tell the user:

```
gettext-mcp is not configured. Add it to Claude Code:

  # Directory mode (recommended) — every tool call requires `path`,
  # paths are constrained to the given directory:
  claude mcp add gettext-mcp -- gettext-mcp /path/to/locales

  # Single-file mode — `path` is optional in tool calls, defaults to this file:
  claude mcp add gettext-mcp -- gettext-mcp /path/to/messages.po
```

**Step 0 — Always run first in directory mode:**

```
list_files()
```

Returns every discovered `.po` / `.pot` file. Do not search the filesystem yourself.

In single-file mode `list_files` still works but only returns the bootstrapped file. Tool
calls may omit `path`.

---

## Core Principle

**Never read `.po` or `.pot` files directly.** Even modest catalogs are dense:

- A typical app catalog runs 500-5000 entries; loading one wastes most of the context window.
- Manual edits can break round-trip fidelity (escapes, multi-line, plural indexes).
- There is no in-editor validation of `Plural-Forms`, format flags, or msgctxt uniqueness.

The MCP server batches and filters server-side, persists writes atomically, and preserves
metadata around the field you change. Use it for every operation.

---

## Tool Quick Reference

| Tool | Purpose |
|---|---|
| `list_files` | **Always first in directory mode** — list discovered `.po` / `.pot` files |
| `list_translations` | List entries (optional `query`, `limit`); returns msgid/msgstr/flags/is_translated/is_fuzzy |
| `get_translation` | Inspect one entry by `msgid` (+ optional `msgctxt`); returns comments, source locations, flags, plurals |
| `upsert_translation` | Create or update an entry (msgstr, plurals, flags) |
| `delete_translation` | Clear `msgstr` for a specific (msgid, msgctxt) — keeps the key, resets translation |
| `delete_key` | Remove every entry (all contexts) with a given `msgid` |
| `set_comment` | Set or clear the translator comment (`# ...`) on an entry |
| `set_fuzzy` | Toggle the `fuzzy` flag on an entry |
| `set_flag` | Add or remove an arbitrary flag (`c-format`, `python-format`, `no-wrap`, ...) |
| `list_contexts` | List all distinct `msgctxt` values used in the file |
| `list_metadata` | Read PO header (`Language`, `Plural-Forms`, `Content-Type`, ...) |
| `set_header` | Set or remove a single PO header entry |

All tools accept `path` (optional in single-file mode, required in directory mode).

---

## Workflows

### Translating untranslated strings

There is no dedicated `get_untranslated` tool yet. Filter the full listing:

```
list_files()                                          # directory mode only
→ list_metadata(path: "fr.po")                        # confirm Language + Plural-Forms
→ list_translations(path: "fr.po")                    # returns is_translated per entry
→ filter client-side: entries where is_translated == false
→ for each untranslated entry:
    upsert_translation(
      path: "fr.po",
      msgid: "Save",
      msgctxt: <preserve from listing>,               # never drop msgctxt
      msgstr: "Enregistrer"
    )
```

Use `query` to narrow large catalogs by substring. Use `limit` to page through huge files.

### Reviewing and fixing fuzzy entries

`fuzzy` means `msgmerge` (or a translator) is unsure the translation still matches the source.
Always review before clearing.

```
list_translations(path: "uk.po")
→ filter client-side: entries where is_fuzzy == true
→ for each:
    get_translation(path, msgid, msgctxt)             # see previous msgid (#| msgid ...) and comments
    upsert_translation(path, msgid, msgctxt, msgstr: <corrected>)
    set_fuzzy(path, msgid, msgctxt, fuzzy: false)     # clear flag after confirming
```

### Adding a new translation with msgctxt

`msgctxt` disambiguates identical `msgid`s used in different UI locations (e.g. "Save" as a
menu item vs. a toolbar button label, "Open" the verb vs. "Open" the adjective).

```
list_contexts(path: "es.po")                          # see what contexts already exist
→ upsert_translation(
    path: "es.po",
    msgid: "Save",
    msgctxt: "menu",                                  # different context = different entry
    msgstr: "Guardar"
  )
→ upsert_translation(
    path: "es.po",
    msgid: "Save",
    msgctxt: "toolbar.tooltip",
    msgstr: "Guardar cambios"
  )
```

Two entries with the same `msgid` but different `msgctxt` are independent. Forgetting
`msgctxt` will silently overwrite or read the wrong entry.

### Plural forms

`msgid_plural` (singular reference + plural reference) plus `msgstr_plural` (array, one
string per CLDR plural category for the locale). The number of array entries must equal
the `nplurals` value in the file's `Plural-Forms` header.

```
list_metadata(path: "uk.po")                          # check Plural-Forms / nplurals
# Ukrainian: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : ...)
# English:   nplurals=2; plural=(n != 1);
# Japanese:  nplurals=1; plural=0;

→ upsert_translation(
    path: "uk.po",
    msgid: "%d item",
    msgid_plural: "%d items",
    msgstr_plural: [
      "%d елемент",                                   # one  (1, 21, 31, ...)
      "%d елементи",                                  # few  (2-4, 22-24, ...)
      "%d елементів"                                  # many (0, 5-20, 25-30, ...)
    ]
  )

→ upsert_translation(
    path: "en.po",
    msgid: "%d item",
    msgid_plural: "%d items",
    msgstr_plural: ["%d item", "%d items"]            # 2 entries for English
  )

→ upsert_translation(
    path: "ja.po",
    msgid: "%d item",
    msgid_plural: "%d items",
    msgstr_plural: ["%dアイテム"]                       # 1 entry for Japanese
  )
```

**Always read `Plural-Forms` from the target file via `list_metadata` first.** A mismatch
between array length and `nplurals` produces a runtime error in the consuming app.

### Inspecting and updating the PO header

```
list_metadata(path: "de.po")
# returns { metadata: { "Language": "de", "Plural-Forms": "...", "Content-Type": "...", ... } }

→ set_header(path: "de.po", key: "Language-Team", value: "German <de@li.org>")
→ set_header(path: "de.po", key: "Last-Translator", value: "Anna Müller <anna@example.com>")

# Remove a header entry:
→ set_header(path: "de.po", key: "X-Generator", value: null)
```

Header order is preserved by the store. Arbitrary keys are round-tripped.

### Adding flags (format specifier validation hints)

```
# Mark a string as containing C printf format specifiers:
set_flag(path, msgid: "Found %d files", flag: "c-format", enabled: true)

# Mark as Python format:
set_flag(path, msgid: "Welcome, {name}", flag: "python-format", enabled: true)

# Disable line wrapping for a long string:
set_flag(path, msgid: "...", flag: "no-wrap", enabled: true)
```

Use `set_fuzzy` for the `fuzzy` flag specifically — it's the most common one and has a
dedicated tool.

### Adding translator comments

```
set_comment(
  path: "fr.po",
  msgid: "Save",
  msgctxt: "menu",
  comment: "Keep verb form, not noun (sauvegarder vs. sauvegarde)"
)

# Clear a comment:
set_comment(path: "fr.po", msgid: "Save", msgctxt: "menu", comment: null)
```

Translator comments (`# ...`) are distinct from extracted comments (`#. ...` from
`xgettext`) and source locations (`#: file:line` from `xgettext`). This tool only manages
the translator comment.

### Deleting entries

```
# Reset the translation (msgstr) but keep the entry — next msgmerge can repopulate it:
delete_translation(path, msgid: "Cancel", msgctxt: null)

# Fully remove every entry with this msgid across all contexts:
delete_key(path, msgid: "OldButtonLabel")
```

`delete_key` returns `deleted_count` — useful when the same `msgid` exists under multiple
contexts.

### Working across multiple files (directory mode)

```
list_files()
# returns [{ path: "/locales/fr.po", relative_path: "fr.po" }, ...]

→ for each file:
    list_metadata(path: <file.path>)                  # group by Language
    list_translations(path: <file.path>)              # filter is_translated == false
    upsert_translation(path: <file.path>, ...)        # always pass full absolute path
```

**Always pass `path` in directory mode.** Omitting it produces an error.

### Typical extract / merge / translate cycle (with the gettext CLI)

The MCP server does not run `xgettext` / `msgmerge` itself. When the user is doing the full
extract-merge-translate dance, the workflow is:

```
# 1. Extract strings from source (run via Bash, not MCP):
xgettext -o locale/messages.pot --from-code=UTF-8 src/**/*.py

# 2. Merge updated template into existing translations (Bash):
msgmerge --update locale/fr/LC_MESSAGES/messages.po locale/messages.pot
msgmerge --update locale/uk/LC_MESSAGES/messages.po locale/messages.pot

# 3. NOW switch to MCP tools for the actual translation work:
list_files()
→ list_translations(path: ".../fr/LC_MESSAGES/messages.po")
→ upsert_translation(...)
→ set_fuzzy(..., fuzzy: false)                        # clear msgmerge-added fuzzy flags

# 4. Compile to .mo (Bash):
msgfmt locale/fr/LC_MESSAGES/messages.po -o locale/fr/LC_MESSAGES/messages.mo
```

Steps 1, 2, and 4 are CLI operations — invoke them via Bash. Step 3 is where this skill
matters: every read/write of `.po` content must go through the MCP tools.

---

## Common Pitfalls

| Pitfall | Why it bites | Fix |
|---|---|---|
| Forgetting `msgctxt` | Same `msgid` in two contexts → wrong entry read/overwritten | Always check `list_contexts`; pass `msgctxt` explicitly even when it's `null` |
| Wrong plural form count | App raises runtime error if `msgstr_plural.len()` != `nplurals` | Run `list_metadata` first; match `Plural-Forms` for the target locale |
| Editing fuzzy entry without clearing flag | App will keep ignoring the translation | After `upsert_translation`, call `set_fuzzy(fuzzy: false)` |
| Treating `#~` lines as comments | Obsolete entries get re-inserted on next `msgmerge` | Let `msgmerge` manage them; don't try to round-trip obsolete entries via upsert |
| Using `upsert_translation` to "edit" comments | `upsert` rewrites the entry's translation fields; metadata operations belong elsewhere | Use `set_comment` / `set_flag` / `set_fuzzy` for metadata; use `upsert_translation` only for msgstr / plural / flag arrays |
| Hardcoding paths in single-file mode | Tool calls without `path` only work in single-file mode | In directory mode every call needs `path`; check by running `list_files` |
| Bulk `list_translations` with no `limit` on a huge file | Floods the context window | Use `query` to narrow, or page with `limit` |
| Manually reformatting wrapped msgstr | Breaks round-trip if escapes don't match | Never edit the file by hand — let the server normalize on write |

---

## Error Handling

| Error | Action |
|---|---|
| `path` missing in directory mode | Re-run `list_files`, pass the returned `path` to every subsequent call |
| `path` outside base directory | Directory mode rejects paths above the base dir; ask user to widen scope at MCP startup |
| `get_translation` returns "not found" | Confirm `msgctxt` — same `msgid` with different `msgctxt` is a different entry |
| `upsert_translation` succeeds but app still shows English | Entry is `fuzzy`; call `set_fuzzy(fuzzy: false)` |
| `set_flag` rejects flag string | Flags must match `[A-Za-z0-9_-]+` and be non-empty |
| Plural mismatch warnings in `msgfmt` | `msgstr_plural` length disagrees with header `nplurals`; re-check `list_metadata` |
| MCP tool not found | Ask user to add the server: `claude mcp add gettext-mcp -- gettext-mcp <dir-or-file>` |

---

## Optimal Parameters

| Parameter | Recommended value | Reason |
|---|---|---|
| `query` (list_translations) | substring of msgid or English text | Narrows large catalogs server-side |
| `limit` (list_translations) | 50-100 | Keeps responses inside context budget |
| `msgctxt` | Always pass explicitly (even as `null`) | Avoids silently selecting the wrong entry |
| `path` | Absolute path returned by `list_files` | Avoids path-validation rejections in directory mode |
