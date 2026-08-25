//! Locale facts derivable from a catalog's own path, used to seed the header
//! of a PO file created from scratch.
//!
//! # Why this exists
//!
//! [`crate::service::merger::merge`] preserves the target's header so a
//! translator's `Language`, `Plural-Forms` and `Last-Translator` survive a
//! merge. When the target does not exist yet there is no header to preserve,
//! and the file lands with an empty one — no `Language`, no `Plural-Forms`.
//!
//! That is not cosmetic. A catalog with no `Plural-Forms` sends the consuming
//! toolchain looking for the rule elsewhere, and some of them raise rather than
//! guess: Elixir's Gettext compiler asks its pluralizer, and the default one
//! rejects a BCP 47 tag with a hyphen (`nb-NO`), so a headerless catalog stops
//! the application compiling. Every catalog written by GNU `msgmerge` carries
//! both headers; one written here should too.
//!
//! # What is deliberately NOT derived
//!
//! `Plural-Forms` is emitted only for the families whose expression is
//! unambiguous — a single form, and the two two-form conventions. Everything
//! else is left absent and reported, rather than guessed.
//!
//! The temptation is to cover Slavic and Celtic too, and the reason not to is
//! that the answer depends on a convention this crate cannot see. CLDR gives
//! Russian four categories (one/few/many/other); the `nplurals=3` expression
//! almost every real Russian PO file in the wild carries disagrees with it. A
//! header whose `nplurals` and whose expression describe different schemes is
//! worse than no header at all: `msgfmt` accepts it, and the catalog then
//! mis-selects a form at run time with nothing to indicate why.
//!
//! So the rule here is narrow on purpose — right for the locales it names, and
//! honest about the rest. Widening it is a decision about which convention to
//! follow, and it should be made deliberately rather than inherited from a
//! table somebody filled in once.

/// The language tag a catalog's path implies, by the GNU gettext directory
/// convention `<any>/<locale>/LC_MESSAGES/<domain>.po`.
///
/// Returns `None` for any path that does not have that shape — a caller should
/// leave the header alone rather than seed a guess.
pub fn language_from_path(path: &std::path::Path) -> Option<String> {
    let mut components = path.components().rev();

    // <domain>.po
    let _file = components.next()?;
    // LC_MESSAGES
    let messages_dir = components.next()?;
    if messages_dir.as_os_str() != "LC_MESSAGES" {
        return None;
    }

    let locale = components.next()?;
    let locale = locale.as_os_str().to_str()?;

    if locale.is_empty() {
        None
    } else {
        Some(locale.to_string())
    }
}

/// The `Plural-Forms` header for `language`, or `None` when this crate declines
/// to state one — see the module docs.
///
/// The region is stripped before the lookup (`nb-NO` → `nb`), with one
/// exception: `pt-BR` follows the `n > 1` convention while European Portuguese
/// follows `n != 1`, so it is matched on the full tag first.
pub fn plural_forms_header(language: &str) -> Option<&'static str> {
    const ONE_FORM: &str = "nplurals=1; plural=0;";
    const NE_ONE: &str = "nplurals=2; plural=(n != 1);";
    const GT_ONE: &str = "nplurals=2; plural=(n > 1);";

    let normalised = language.replace('_', "-").to_ascii_lowercase();

    // Region-sensitive cases, before the region is thrown away.
    match normalised.as_str() {
        "pt-br" => return Some(GT_ONE),
        "pt" | "pt-pt" => return Some(NE_ONE),
        _ => {}
    }

    let lang = normalised.split('-').next().unwrap_or(&normalised);

    match lang {
        // No plural distinction at all.
        "ja" | "ko" | "zh" | "vi" | "th" | "ms" | "id" => Some(ONE_FORM),

        // Two forms, singular at exactly one. The Nordic, Germanic and most
        // Romance languages, plus Finnish, Estonian, Greek, Hungarian.
        "en" | "nb" | "nn" | "no" | "da" | "sv" | "de" | "nl" | "fi" | "et" | "es" | "it"
        | "hu" | "el" | "bg" | "he" | "af" | "eu" | "sq" | "ca" | "sw" => Some(NE_ONE),

        // Two forms, singular at zero AND one — French and Brazilian
        // Portuguese treat 0 as singular, which `n != 1` gets wrong.
        "fr" => Some(GT_ONE),

        // Everything else — Slavic, Celtic, Baltic, Arabic, Romanian — is left
        // to a human. See the module docs for why guessing is worse than
        // abstaining here.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn language_comes_from_the_locale_directory() {
        assert_eq!(
            language_from_path(Path::new("priv/gettext/nb-NO/LC_MESSAGES/default.po")),
            Some("nb-NO".to_string())
        );
        assert_eq!(
            language_from_path(Path::new("/abs/locale/pt_BR/LC_MESSAGES/x.po")),
            Some("pt_BR".to_string())
        );
    }

    #[test]
    fn a_path_that_is_not_the_gettext_shape_yields_nothing() {
        // No guess is better than a wrong `Language`, which a translator would
        // then have to notice and correct.
        assert_eq!(language_from_path(Path::new("messages.po")), None);
        assert_eq!(language_from_path(Path::new("nb-NO/default.po")), None);
        assert_eq!(
            language_from_path(Path::new("priv/gettext/nb-NO/OTHER/default.po")),
            None
        );
    }

    #[test]
    fn the_region_is_stripped_for_the_plural_lookup() {
        assert_eq!(
            plural_forms_header("nb-NO"),
            Some("nplurals=2; plural=(n != 1);")
        );
        assert_eq!(
            plural_forms_header("nb_NO"),
            Some("nplurals=2; plural=(n != 1);")
        );
        assert_eq!(
            plural_forms_header("NB"),
            Some("nplurals=2; plural=(n != 1);")
        );
    }

    #[test]
    fn french_and_brazilian_portuguese_take_the_other_two_form_rule() {
        // `n != 1` would make zero plural, which both treat as singular.
        assert_eq!(
            plural_forms_header("fr"),
            Some("nplurals=2; plural=(n > 1);")
        );
        assert_eq!(
            plural_forms_header("pt-BR"),
            Some("nplurals=2; plural=(n > 1);")
        );
        // ...and European Portuguese does not.
        assert_eq!(
            plural_forms_header("pt-PT"),
            Some("nplurals=2; plural=(n != 1);")
        );
    }

    #[test]
    fn languages_with_one_form_get_one() {
        for lang in ["ja", "ko", "zh-Hans", "vi", "th"] {
            assert_eq!(plural_forms_header(lang), Some("nplurals=1; plural=0;"));
        }
    }

    #[test]
    fn the_ambiguous_families_are_declined_rather_than_guessed() {
        // Each of these has a CLDR category count that disagrees with the
        // expression conventional PO files carry. Emitting either would produce
        // a header whose two halves describe different schemes.
        for lang in ["ru", "uk", "pl", "cs", "lt", "ar", "ga", "cy", "ro"] {
            assert_eq!(plural_forms_header(lang), None, "{lang} should be declined");
        }
    }
}
