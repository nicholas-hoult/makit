//! UI language (#71): which language the interface uses, and the checks that keep the two locale files in step.
//!
//! Strings live in `locales/{zh,en}.json` (flat dotted keys, `%{name}` placeholders) and are looked up with
//! `t!("settings.language.title")` (rust-i18n). The preference is `system` (default), `zh` or `en`.
//!
//! Why the tests are separate: a missing English key silently falls back to Chinese on screen, and a
//! `%{name}` that only exists in one language shows up as a literal `%{name}` or an empty hole. Neither is
//! visible in a code review, so the locale files are compared mechanically.

use std::collections::BTreeSet;

/// Translate a key into a `String`; takes the same arguments as `t!` (`ts!("sidebar.count", n = 3)`,
/// `ts!(key_variable)`, `ts!("x", locale = "en")`). Always go through `ts!` / `tr!` instead of `t!` directly:
/// they make sure the locale is initialised (Chinese) even where `app::run` never ran, e.g. in unit tests.
#[macro_export]
macro_rules! ts {
    ($($args:tt)*) => {{
        $crate::i18n::ensure_init();
        rust_i18n::t!($($args)*).into_owned()
    }};
}

/// Same as `ts!` but a `SharedString`, ready for `.child(..)` and element labels.
#[macro_export]
macro_rules! tr {
    ($($args:tt)*) => {
        gpui::SharedString::from($crate::ts!($($args)*))
    };
}

/// Make Chinese the locale if nothing has chosen one yet (rust-i18n's own default would be English).
/// `apply` marks the locale as chosen, so this never overrides the user's preference. The "chosen" flag lives in
/// makit-core (it reads the same process-wide locale), so both crates agree.
pub fn ensure_init() {
    makit_core::i18n::ensure_init();
}

/// Preference values stored in `NativeState.language`.
pub const PREF_SYSTEM: &str = "system";
pub const PREF_ZH: &str = "zh";
pub const PREF_EN: &str = "en";

/// Resolve the stored preference to a locale code. An explicit `zh` / `en` wins; anything else (including
/// `system` and unknown values from a newer version) follows the system: the first preferred language
/// that is Chinese or English decides, and a system with neither gets English.
pub fn resolve(pref: &str, system_languages: &[String]) -> &'static str {
    match pref {
        PREF_ZH => return "zh",
        PREF_EN => return "en",
        _ => {}
    }
    for lang in system_languages {
        let lang = lang.to_ascii_lowercase();
        if lang.starts_with("zh") {
            return "zh";
        }
        if lang.starts_with("en") {
            return "en";
        }
    }
    "en"
}

/// Switch the process-wide locale to the given preference. Self-tests (`MAKIT_NATIVE_SELFTEST`) assert on
/// Chinese text, so they always run in Chinese whatever the preference or system language is.
pub fn apply(pref: &str) {
    let locale = if std::env::var_os("MAKIT_NATIVE_SELFTEST").is_some() {
        "zh"
    } else {
        resolve(pref, &crate::terminal::fonts::system_preferred_languages())
    };
    makit_core::i18n::apply(locale);
}

/// `%{name}` placeholders in a translation, as a set.
pub fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                out.insert(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn langs(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn explicit_choice_wins_over_system() {
        assert_eq!(resolve("zh", &langs(&["en-US"])), "zh");
        assert_eq!(resolve("en", &langs(&["zh-Hans-CN"])), "en");
    }

    #[test]
    fn system_follows_first_chinese_or_english_language() {
        assert_eq!(resolve("system", &langs(&["zh-Hans-CN", "en-US"])), "zh");
        assert_eq!(resolve("system", &langs(&["en-GB", "zh-Hans-CN"])), "en");
        assert_eq!(resolve("system", &langs(&["ja-JP", "zh-Hant-TW"])), "zh", "other languages are skipped, not decisive");
        assert_eq!(resolve("system", &langs(&["zh-Hant-TW"])), "zh", "traditional Chinese uses the Chinese text for now");
    }

    #[test]
    fn system_without_chinese_or_english_gets_english() {
        assert_eq!(resolve("system", &langs(&["fr-FR", "de-DE"])), "en");
        assert_eq!(resolve("system", &[]), "en");
    }

    #[test]
    fn unknown_preference_behaves_like_system() {
        assert_eq!(resolve("fr", &langs(&["zh-Hans-CN"])), "zh");
        assert_eq!(resolve("", &langs(&["en-US"])), "en");
    }

    #[test]
    fn placeholders_are_extracted() {
        assert_eq!(placeholders("Hello, %{name}"), ["name".to_string()].into());
        assert_eq!(placeholders("%{a} and %{b} and %{a}"), ["a".to_string(), "b".to_string()].into());
        assert!(placeholders("no holes, 100%").is_empty());
        assert!(placeholders("broken %{open").is_empty());
    }

    /// Read a locale file as key → text (skipping `_version`).
    fn load(text: &str) -> BTreeMap<String, String> {
        let v: serde_json::Value = serde_json::from_str(text).expect("locale file is valid JSON");
        v.as_object()
            .expect("locale file is a flat object")
            .iter()
            .filter(|(k, _)| k.as_str() != "_version")
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_else(|| panic!("{k} must be a string")).to_string()))
            .collect()
    }

    fn locales() -> (BTreeMap<String, String>, BTreeMap<String, String>) {
        (load(include_str!("../locales/zh.json")), load(include_str!("../locales/en.json")))
    }

    #[test]
    fn zh_and_en_have_the_same_keys() {
        let (zh, en) = locales();
        let only_zh: Vec<_> = zh.keys().filter(|k| !en.contains_key(*k)).collect();
        let only_en: Vec<_> = en.keys().filter(|k| !zh.contains_key(*k)).collect();
        assert!(only_zh.is_empty(), "keys missing in en.json: {only_zh:?}");
        assert!(only_en.is_empty(), "keys missing in zh.json: {only_en:?}");
    }

    #[test]
    fn translations_are_not_empty_and_placeholders_match() {
        let (zh, en) = locales();
        for (k, z) in &zh {
            let e = &en[k];
            assert!(!z.trim().is_empty() && !e.trim().is_empty(), "{k}: empty translation");
            assert_eq!(placeholders(z), placeholders(e), "{k}: %{{}} placeholders differ between zh and en");
        }
    }

    /// No Chinese string literal may be left in production code (self-tests and `#[cfg(test)]` code are exempt,
    /// they assert on Chinese text): a leftover literal would show up untranslated in the English UI.
    /// Whitelist: stored / compared data values, not display text.
    #[test]
    fn no_chinese_string_literals_in_production_code() {
        const ALLOWED: &[(&str, &str)] = &[
            ("overlays/palette_logic.rs", "\"全部\""),       // persisted filter value for "all types"
        ];
        fn has_cjk(s: &str) -> bool {
            s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        }
        let mut found = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let name = path.to_string_lossy().replace('\\', "/");
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !name.ends_with(".rs") || name.contains("selftest") || name.ends_with("flood_test.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let production = src.split("#[cfg(test)]").next().unwrap();
                for (n, line) in production.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if !has_cjk(code) {
                        continue;
                    }
                    let allowed = ALLOWED.iter().any(|(f, lit)| name.ends_with(f) && code.contains(lit));
                    if !allowed {
                        found.push(format!("{}:{}: {}", name.rsplit("src/").next().unwrap(), n + 1, code.trim()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "Chinese literals left in production code:\n{}", found.join("\n"));
    }

    /// Every `t!("key")` in the source must exist, otherwise the screen shows the raw key.
    #[test]
    fn every_key_used_in_source_exists() {
        let (zh, _) = locales();
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                    let src = std::fs::read_to_string(&path).unwrap();
                    for (i, _) in src.match_indices("!(\"") {
                        let before = &src[..i];
                        let name_len = before.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').count();
                        let name = &before[before.len() - name_len..];
                        if name != "t" && name != "tr" && name != "ts" {
                            continue;
                        }
                        let after = &src[i + 3..];
                        if let Some(end) = after.find('"') {
                            let key = &after[..end];
                            if key.contains('.') && !zh.contains_key(key) {
                                missing.push(format!("{}: {key}", path.display()));
                            }
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "t!() keys without a translation: {missing:#?}");
    }
}
