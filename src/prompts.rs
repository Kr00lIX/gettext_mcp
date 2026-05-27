//! MCP prompt router for the gettext server.
//!
//! Exposes reusable workflow templates (translate a batch, review existing
//! translations, audit a whole file, etc.) that an MCP client can invoke
//! via the `prompts/get` capability. The actual instruction text inside
//! each prompt is what the assistant will see as its starting context, so
//! every prompt enumerates the concrete tool calls it should make.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{GetPromptResult, PromptMessage, PromptMessageRole},
    prompt, prompt_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::GettextMcpServer;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TranslateBatchParams {
    /// Target language (display name like "French" or BCP-47 code like "fr_FR").
    locale: String,
    /// Path to the .po file. Optional in single-file mode.
    path: Option<String>,
    /// How many untranslated entries to pull per batch (default: 30).
    count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReviewTranslationsParams {
    /// Path to the .po file. Optional in single-file mode.
    path: Option<String>,
    /// What subset to focus on: "fuzzy" (default), "all", or "untranslated".
    focus: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FullTranslateParams {
    /// Path to the .po file to translate (required).
    path: String,
    /// Target language (display name or code) to translate into.
    target_locale: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LocalizationAuditParams {
    /// Path to the .po file. Optional in single-file mode.
    path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CleanupStaleParams {
    /// Path to the .po file. Optional in single-file mode.
    path: Option<String>,
    /// If true (default), only list what would be removed; do not call any
    /// delete tools. Set to false to actually delete.
    dry_run: Option<bool>,
}

/// Render a small "Working file:" preamble shared by every prompt so the
/// assistant knows which `path` argument to pass to each tool call.
fn path_preamble(path: Option<&String>) -> String {
    match path {
        Some(p) => format!("Working file: {p}\nPass `path=\"{p}\"` to every tool call below.\n\n"),
        None => "Working file: use the default single-file mode (omit `path`) \
                 unless the server is in dynamic mode, in which case ask the user \
                 for a path before continuing.\n\n"
            .to_string(),
    }
}

#[prompt_router(vis = "pub(crate)")]
impl GettextMcpServer {
    /// Translate a batch of untranslated entries in a .po file.
    #[prompt(
        name = "translate_batch",
        description = "Translate a batch of untranslated entries in a .po file, \
                       preserving format specifiers and plural forms."
    )]
    fn translate_batch(
        &self,
        Parameters(params): Parameters<TranslateBatchParams>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let count = params.count.unwrap_or(30);
        let preamble = path_preamble(params.path.as_ref());
        let locale = &params.locale;

        let content = format!(
            "{preamble}\
            You are translating GNU gettext entries into {locale}.\n\
            \n\
            Step 1: Inspect the header\n\
            \x20 - Call `list_metadata` to read the `Language` and `Plural-Forms`\n\
            \x20   headers. The plural-forms expression tells you how many\n\
            \x20   `msgstr_plural` entries you need (look for `nplurals=N;`).\n\
            \x20 - If `Language` is missing or wrong for {locale}, fix it with\n\
            \x20   `set_header(key=\"Language\", value=\"<code>\")`.\n\
            \n\
            Step 2: Pull a batch\n\
            \x20 - Call `get_untranslated(batch_size={count}, offset=0,\n\
            \x20   include_fuzzy=true)` to get the next chunk. Each entry\n\
            \x20   includes `msgid`, `msgctxt`, `msgid_plural`, current `msgstr`,\n\
            \x20   `flags`, `needs_plural_forms`, and a `has_more` cursor.\n\
            \n\
            Step 3: Translate each entry\n\
            \x20 - Translate `msgid` naturally into {locale} \u{2014} idiomatic, not\n\
            \x20   word-for-word.\n\
            \x20 - Preserve every format specifier exactly: `%s`, `%d`, `%lld`,\n\
            \x20   `%1$s`, `{{name}}`, `${{0}}`, etc. Do not reorder positional\n\
            \x20   specifiers unless the target grammar requires it (then use\n\
            \x20   `%1$s`-style positional forms).\n\
            \x20 - Respect `msgctxt` \u{2014} the same `msgid` can mean different\n\
            \x20   things in different contexts.\n\
            \x20 - Keep escapes (`\\n`, `\\t`, `\\\"`) intact.\n\
            \n\
            Step 4: Handle plural forms\n\
            \x20 - When `msgid_plural` is set, supply a `msgstr_plural` array\n\
            \x20   with exactly `needs_plural_forms` entries, indexed by the\n\
            \x20   plural-forms expression from Step 1.\n\
            \x20 - Leave the singular `msgstr` empty (\"\") for pluralized\n\
            \x20   entries \u{2014} only `msgstr_plural` matters.\n\
            \n\
            Step 5: Submit\n\
            \x20 - Call `upsert_translation(msgid, msgctxt, msgstr,\n\
            \x20   msgid_plural, msgstr_plural, flags)` for each entry. Carry\n\
            \x20   over the existing `flags` array unchanged.\n\
            \x20 - If the entry was previously fuzzy, call\n\
            \x20   `set_fuzzy(msgid, msgctxt, fuzzy=false)` after upserting so\n\
            \x20   it counts as translated.\n\
            \n\
            Step 6: Loop\n\
            \x20 - Repeat Step 2 with an advancing `offset` until\n\
            \x20   `has_more` is false. Then call `get_coverage` to confirm\n\
            \x20   the new translated percentage for {locale}.\n",
        );

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )])
        .with_description(format!(
            "Translate a batch of {count} entries into {locale}"
        )))
    }

    /// Review existing translations for quality and correctness.
    #[prompt(
        name = "review_translations",
        description = "Review existing translations for format-specifier errors, missing plural \
                       forms, and prose quality."
    )]
    fn review_translations(
        &self,
        Parameters(params): Parameters<ReviewTranslationsParams>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let focus = params.focus.as_deref().unwrap_or("fuzzy");
        let preamble = path_preamble(params.path.as_ref());

        let focus_instructions = match focus {
            "all" => {
                "    - Iterate over every translated entry via repeated\n\
                     \x20     `list_translations(limit=50)` calls.\n"
            }
            "untranslated" => {
                "    - Use `get_untranslated` to enumerate the\n\
                              \x20     remaining work and triage which strings\n\
                              \x20     are highest-priority (UI surfaces vs.\n\
                              \x20     debug copy).\n"
            }
            _ => {
                "    - Use `list_translations(query=\"fuzzy\")` or scan the\n\
                 \x20     output of `validate_translations` for entries flagged\n\
                 \x20     fuzzy and re-check each one against its source.\n"
            }
        };

        let content = format!(
            "{preamble}\
            You are reviewing translations in a .po file. Focus area: {focus}.\n\
            \n\
            Step 1: Surface technical issues\n\
            \x20 - Call `validate_translations(severity_filter=null)` to get\n\
            \x20   format-specifier mismatches, missing plural forms, and\n\
            \x20   other structural errors.\n\
            \x20 - Sort findings by severity: errors first, then warnings.\n\
            \n\
            Step 2: Check overall coverage\n\
            \x20 - Call `get_coverage` to see translated / fuzzy /\n\
            \x20   untranslated counts. A high fuzzy ratio is a red flag.\n\
            \n\
            Step 3: Drill into the focus area\n\
            {focus_instructions}\
            \n\
            Step 4: Pull entries for human-readable review\n\
            \x20 - Use `list_translations(query=...)` or\n\
            \x20   `search_keys(pattern=..., match_in=\"both\")` to fetch the\n\
            \x20   specific msgids implicated by each finding.\n\
            \x20 - Call `get_translation(msgid, msgctxt)` for the full entry\n\
            \x20   when you need comments or source locations.\n\
            \n\
            Step 5: Classify each issue\n\
            \x20 - ERROR: format-specifier mismatch, missing required plural\n\
            \x20   form, untranslated placeholder. Fix immediately.\n\
            \x20 - WARNING: stylistic mismatch, inconsistent terminology,\n\
            \x20   suspected machine translation. Flag for human review.\n\
            \n\
            Step 6: Fix and clear fuzzy\n\
            \x20 - Apply fixes via `upsert_translation` (preserve existing\n\
            \x20   `flags` you don't want to drop).\n\
            \x20 - After a fix, call `set_fuzzy(fuzzy=false)` so the entry\n\
            \x20   counts as translated again.\n\
            \x20 - Re-run `validate_translations` at the end to confirm a\n\
            \x20   clean bill of health.\n",
        );

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )])
        .with_description(format!("Review translations (focus: {focus})")))
    }

    /// End-to-end workflow to translate a .po file to 100% coverage.
    #[prompt(
        name = "full_translate",
        description = "Translate a .po file end-to-end into a target locale, from header setup \
                       through validation and final coverage check."
    )]
    fn full_translate(
        &self,
        Parameters(params): Parameters<FullTranslateParams>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let path = &params.path;
        let locale = &params.target_locale;

        let content = format!(
            "Full translation workflow: {path} \u{2192} {locale}.\n\
            Pass `path=\"{path}\"` to every tool call below.\n\
            \n\
            Step 1: Snapshot the starting state\n\
            \x20 - Call `get_coverage` and record the current\n\
            \x20   translated/fuzzy/untranslated counts. This is your\n\
            \x20   baseline.\n\
            \n\
            Step 2: Verify and fix the header\n\
            \x20 - Call `list_metadata` and confirm:\n\
            \x20     - `Language` matches {locale}.\n\
            \x20     - `Plural-Forms` has a correct `nplurals=N; plural=...`\n\
            \x20       expression for {locale}.\n\
            \x20     - `Content-Type` is `text/plain; charset=UTF-8`.\n\
            \x20 - Fix anything wrong with\n\
            \x20   `set_header(key=..., value=...)`.\n\
            \n\
            Step 3: Translate in batches until empty\n\
            \x20 - Loop:\n\
            \x20     a. `get_untranslated(batch_size=30, offset=0,\n\
            \x20        include_fuzzy=true)`\n\
            \x20     b. Translate each entry into {locale} \u{2014} preserve\n\
            \x20        every `%s`, `%d`, `%lld`, `{{name}}`, `\\n`, etc.\n\
            \x20     c. For each entry call `upsert_translation`. For\n\
            \x20        entries with `msgid_plural`, supply a\n\
            \x20        `msgstr_plural` array of length\n\
            \x20        `needs_plural_forms`.\n\
            \x20     d. For any entry that came back fuzzy, call\n\
            \x20        `set_fuzzy(fuzzy=false)` after upserting.\n\
            \x20     e. Stop when `has_more` is false.\n\
            \n\
            Step 4: Validate\n\
            \x20 - Call `validate_translations` and fix every reported\n\
            \x20   error (format-specifier mismatch, missing plural forms,\n\
            \x20   etc.) via `upsert_translation`.\n\
            \x20 - Re-run `validate_translations` until it returns zero\n\
            \x20   issues.\n\
            \n\
            Step 5: Final coverage check\n\
            \x20 - Call `get_coverage` and confirm 100% translated, 0\n\
            \x20   fuzzy.\n\
            \x20 - If there are stragglers, loop back to Step 3.\n\
            \n\
            Step 6: Report\n\
            \x20 - Summarize: starting coverage, ending coverage, number\n\
            \x20   of entries translated, number of validation issues\n\
            \x20   fixed, and any entries you intentionally left fuzzy\n\
            \x20   for human review.\n",
        );

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )])
        .with_description(format!("Full translation of {path} into {locale}")))
    }

    /// Comprehensive localization audit producing a written report.
    #[prompt(
        name = "localization_audit",
        description = "Produce a comprehensive localization audit: coverage, validation, stale \
                       entries, context disambiguation, and prose-quality spot checks."
    )]
    fn localization_audit(
        &self,
        Parameters(params): Parameters<LocalizationAuditParams>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let preamble = path_preamble(params.path.as_ref());

        let content = format!(
            "{preamble}\
            Produce a localization audit for the .po file. The output is a\n\
            written report with the sections listed in Step 6.\n\
            \n\
            Step 1: Coverage numbers\n\
            \x20 - Call `get_coverage` and record translated, fuzzy,\n\
            \x20   untranslated, and total counts. Compute the\n\
            \x20   `translated / total` percentage.\n\
            \n\
            Step 2: Technical validation\n\
            \x20 - Call `validate_translations(severity_filter=null)` and\n\
            \x20   group findings by category:\n\
            \x20     - Format-specifier mismatches (errors).\n\
            \x20     - Missing or extra plural forms (errors).\n\
            \x20     - Other warnings.\n\
            \n\
            Step 3: Stale / obsolete entries\n\
            \x20 - Call `get_stale(batch_size=100, offset=0)` to enumerate\n\
            \x20   obsolete (`#~`-prefixed) entries. Decide which can be\n\
            \x20   removed and which should be revived because the source\n\
            \x20   string came back.\n\
            \n\
            Step 4: Context disambiguation\n\
            \x20 - Call `list_contexts` to see every `msgctxt` value in\n\
            \x20   use.\n\
            \x20 - Sample-list translations with\n\
            \x20   `list_translations(limit=200)` and look for msgids that\n\
            \x20   appear multiple times without `msgctxt`. These may\n\
            \x20   collide and need disambiguation.\n\
            \n\
            Step 5: Prose-quality spot check\n\
            \x20 - Pick 5-10 entries spanning short labels and long\n\
            \x20   sentences. Use `list_translations(limit=...)` plus\n\
            \x20   `get_translation` to pull them.\n\
            \x20 - Evaluate: naturalness, terminology consistency, tone,\n\
            \x20   correct handling of placeholders.\n\
            \n\
            Step 6: Write the report\n\
            \x20 Produce a markdown report with these sections:\n\
            \x20   1. Coverage summary (numbers from Step 1).\n\
            \x20   2. Validation issues by severity (from Step 2).\n\
            \x20   3. Stale entries and recommended action (from Step 3).\n\
            \x20   4. Context-collision risks (from Step 4).\n\
            \x20   5. Prose quality notes with cited msgids (from Step 5).\n\
            \x20   6. Top 3 recommended follow-ups, ordered by impact.\n",
        );

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )])
        .with_description("Comprehensive localization audit"))
    }

    /// Clean up obsolete (`#~`) entries from a .po file, with a dry-run mode.
    #[prompt(
        name = "cleanup_stale",
        description = "Identify and (optionally) delete obsolete entries from a .po file. Default \
                       is dry-run \u{2014} no deletions until you flip dry_run=false."
    )]
    fn cleanup_stale(
        &self,
        Parameters(params): Parameters<CleanupStaleParams>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let dry_run = params.dry_run.unwrap_or(true);
        let preamble = path_preamble(params.path.as_ref());

        let mode_block = if dry_run {
            "Mode: DRY-RUN. Do NOT call `delete_key` or `delete_translation`\n\
            in this run. Only list what would be removed.\n"
        } else {
            "Mode: LIVE. After confirming each entry is safe to delete you\n\
            may call `delete_key` to remove it.\n"
        };

        let action_block = if dry_run {
            "Step 4: Report (dry-run)\n\
            \x20 - Produce a list of msgids that WOULD be deleted, grouped\n\
            \x20   by reason (e.g. \"source string removed in v2.1\",\n\
            \x20   \"never translated\"). Do NOT call any delete tools.\n\
            \x20 - Recommend whether to re-run with `dry_run=false`.\n"
        } else {
            "Step 4: Delete confirmed-stale entries\n\
            \x20 - For each entry confirmed in Step 3, call\n\
            \x20   `delete_key(msgid=...)` to remove every context for\n\
            \x20   that msgid. (Use `delete_translation` instead if only\n\
            \x20   one specific `(msgid, msgctxt)` should go.)\n\
            \x20 - After deletion, call `get_coverage` to verify the new\n\
            \x20   totals and `get_stale` to confirm the queue is empty.\n"
        };

        let content = format!(
            "{preamble}\
            Clean up obsolete entries in a .po file.\n\
            {mode_block}\n\
            Step 1: Enumerate stale entries\n\
            \x20 - Call `get_stale(batch_size=100, offset=0)` and page\n\
            \x20   through until `has_more` is false. These are entries\n\
            \x20   that gettext has marked obsolete (`#~` prefix) because\n\
            \x20   the source string was removed or changed.\n\
            \n\
            Step 2: Pull context on each candidate\n\
            \x20 - For non-obvious cases, call `get_translation(msgid,\n\
            \x20   msgctxt)` to see the full entry, including source\n\
            \x20   locations (`#:` comments) and translator comments.\n\
            \n\
            Step 3: Decide per entry\n\
            \x20 - DELETE: the source string is genuinely gone (feature\n\
            \x20   removed, rewording finalized, never translated).\n\
            \x20 - REVIVE: the obsolete msgstr is still useful because\n\
            \x20   the source string came back \u{2014} re-add it via\n\
            \x20   `upsert_translation` so it is no longer obsolete.\n\
            \x20 - KEEP-OBSOLETE: leave as-is (rare; only when you want\n\
            \x20   to preserve historical translator notes).\n\
            \n\
            {action_block}",
        );

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            content,
        )])
        .with_description(format!(
            "Clean up stale entries ({})",
            if dry_run { "dry-run" } else { "live" }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::GettextStoreManager;
    use rmcp::model::PromptMessageContent;
    use std::sync::Arc;

    fn make_server() -> GettextMcpServer {
        let manager = Arc::new(GettextStoreManager::new(None));
        GettextMcpServer::new(manager)
    }

    fn text_of(result: &GetPromptResult) -> &str {
        let PromptMessageContent::Text { ref text } = result.messages[0].content else {
            panic!("expected text content");
        };
        text
    }

    #[test]
    fn translate_batch_mentions_key_tools_and_locale() {
        let server = make_server();
        let result = server
            .translate_batch(Parameters(TranslateBatchParams {
                locale: "French".into(),
                path: Some("/tmp/fr.po".into()),
                count: Some(15),
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("French"));
        assert!(text.contains("/tmp/fr.po"));
        assert!(text.contains("15"));
        assert!(text.contains("get_untranslated"));
        assert!(text.contains("list_metadata"));
        assert!(text.contains("upsert_translation"));
        assert!(text.contains("set_fuzzy"));
        assert!(text.contains("Plural-Forms"));
        assert!(text.contains("msgstr_plural"));
    }

    #[test]
    fn translate_batch_default_count_is_thirty() {
        let server = make_server();
        let result = server
            .translate_batch(Parameters(TranslateBatchParams {
                locale: "de".into(),
                path: None,
                count: None,
            }))
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains("batch_size=30"));
        // No path → assistant should be told about single-file mode.
        assert!(text.contains("single-file"));
    }

    #[test]
    fn review_translations_default_focus_is_fuzzy() {
        let server = make_server();
        let result = server
            .review_translations(Parameters(ReviewTranslationsParams {
                path: Some("/work/messages.po".into()),
                focus: None,
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("fuzzy"));
        assert!(text.contains("validate_translations"));
        assert!(text.contains("get_coverage"));
        assert!(text.contains("search_keys"));
        assert!(text.contains("upsert_translation"));
        assert!(text.contains("set_fuzzy"));
    }

    #[test]
    fn review_translations_alternative_focus_branches() {
        let server = make_server();
        let all = server
            .review_translations(Parameters(ReviewTranslationsParams {
                path: None,
                focus: Some("all".into()),
            }))
            .unwrap();
        assert!(text_of(&all).contains("list_translations"));

        let untranslated = server
            .review_translations(Parameters(ReviewTranslationsParams {
                path: None,
                focus: Some("untranslated".into()),
            }))
            .unwrap();
        assert!(text_of(&untranslated).contains("get_untranslated"));
    }

    #[test]
    fn full_translate_lists_every_phase() {
        let server = make_server();
        let result = server
            .full_translate(Parameters(FullTranslateParams {
                path: "/work/ja.po".into(),
                target_locale: "ja_JP".into(),
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("/work/ja.po"));
        assert!(text.contains("ja_JP"));
        assert!(text.contains("get_coverage"));
        assert!(text.contains("list_metadata"));
        assert!(text.contains("set_header"));
        assert!(text.contains("get_untranslated"));
        assert!(text.contains("upsert_translation"));
        assert!(text.contains("validate_translations"));
        // Final coverage verification at the end.
        assert!(text.contains("100%"));
    }

    #[test]
    fn localization_audit_covers_all_six_sections() {
        let server = make_server();
        let result = server
            .localization_audit(Parameters(LocalizationAuditParams {
                path: Some("/work/messages.po".into()),
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("get_coverage"));
        assert!(text.contains("validate_translations"));
        assert!(text.contains("get_stale"));
        assert!(text.contains("list_contexts"));
        assert!(text.contains("list_translations"));
        // Section headings for the final report.
        assert!(text.contains("Coverage summary"));
        assert!(text.contains("Stale entries"));
    }

    #[test]
    fn cleanup_stale_dry_run_does_not_mention_delete_call() {
        let server = make_server();
        let result = server
            .cleanup_stale(Parameters(CleanupStaleParams {
                path: Some("/work/messages.po".into()),
                dry_run: None, // default = true
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("DRY-RUN"));
        assert!(text.contains("get_stale"));
        assert!(text.contains("Do NOT call"));
        // The dry-run report section should not instruct the model to call delete_key.
        assert!(!text
            .contains("call\n\x20 - For each entry confirmed in Step 3, call\n\x20   `delete_key"));
    }

    #[test]
    fn cleanup_stale_live_mentions_delete_tools() {
        let server = make_server();
        let result = server
            .cleanup_stale(Parameters(CleanupStaleParams {
                path: None,
                dry_run: Some(false),
            }))
            .unwrap();
        let text = text_of(&result);

        assert!(text.contains("LIVE"));
        assert!(text.contains("delete_key"));
        assert!(text.contains("delete_translation"));
        assert!(text.contains("get_stale"));
        assert!(text.contains("get_coverage"));
    }
}
