//! Translations for the strings makit-core hands to the UI (error messages, recovery details, relative times,
//! transcript notices). The locale is process-wide and set by the UI (`apply`); core only reads it.
//!
//! `ts!("key", name = value)` looks a key up in `core/locales/{zh,en}.json` (Chinese is the fallback). Go through
//! `ts!` rather than `t!` so the locale is initialised (Chinese) when nothing has chosen one, e.g. in unit tests.

static INIT: std::sync::Once = std::sync::Once::new();

/// Make Chinese the locale if nothing has chosen one yet (rust-i18n's own default would be English).
pub fn ensure_init() {
    INIT.call_once(|| rust_i18n::set_locale("zh"));
}

/// Set the process-wide locale (`zh` / `en`) and mark it as chosen, so `ensure_init` never overrides it.
pub fn apply(locale: &str) {
    INIT.call_once(|| {});
    rust_i18n::set_locale(locale);
}

/// Translate a key into a `String`; same arguments as `rust_i18n::t!`.
#[macro_export]
macro_rules! ts {
    ($($args:tt)*) => {{
        $crate::i18n::ensure_init();
        rust_i18n::t!($($args)*).into_owned()
    }};
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    fn load(text: &str) -> BTreeMap<String, String> {
        let v: serde_json::Value = serde_json::from_str(text).expect("locale file is valid JSON");
        v.as_object()
            .expect("locale file is a flat object")
            .iter()
            .filter(|(k, _)| k.as_str() != "_version")
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_else(|| panic!("{k} must be a string")).to_string()))
            .collect()
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = text;
        while let Some(i) = rest.find("%{") {
            let after = &rest[i + 2..];
            let Some(end) = after.find('}') else { break };
            out.insert(after[..end].to_string());
            rest = &after[end + 1..];
        }
        out
    }

    /// A missing English key silently falls back to Chinese on screen, a `%{x}` that exists in one language only
    /// shows up as a literal `%{x}`: compare the two files mechanically.
    #[test]
    fn zh_and_en_have_the_same_keys_and_placeholders() {
        let (zh, en) = (load(include_str!("../locales/zh.json")), load(include_str!("../locales/en.json")));
        let only_zh: Vec<_> = zh.keys().filter(|k| !en.contains_key(*k)).collect();
        let only_en: Vec<_> = en.keys().filter(|k| !zh.contains_key(*k)).collect();
        assert!(only_zh.is_empty(), "keys missing in en.json: {only_zh:?}");
        assert!(only_en.is_empty(), "keys missing in zh.json: {only_en:?}");
        for (k, z) in &zh {
            let e = &en[k];
            assert!(!z.trim().is_empty() && !e.trim().is_empty(), "{k}: empty translation");
            assert_eq!(placeholders(z), placeholders(e), "{k}: %{{}} placeholders differ between zh and en");
        }
    }

    /// No Chinese string literal may be left in production code (code under `#[cfg(...test...)]` is exempt).
    #[test]
    fn no_chinese_string_literals_in_production_code() {
        fn has_cjk(s: &str) -> bool {
            s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        }
        let mut found = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let name = path.to_string_lossy().to_string();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !name.ends_with(".rs") || name.ends_with("tests.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let mut production = String::new();
                for line in src.lines() {
                    if line.contains("#[cfg(") && line.contains("test") {
                        break;
                    }
                    production.push_str(line);
                    production.push('\n');
                }
                for (n, line) in production.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or("");
                    if has_cjk(code) {
                        found.push(format!("{}:{}: {}", name.rsplit("src/").next().unwrap(), n + 1, code.trim()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "Chinese literals left in production code:\n{}", found.join("\n"));
    }

    /// Every `ts!("key")` in the source must exist.
    #[test]
    fn every_key_used_in_source_exists() {
        let zh = load(include_str!("../locales/zh.json"));
        let mut missing = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                    let src = std::fs::read_to_string(&path).unwrap();
                    for (i, _) in src.match_indices("ts!(\"") {
                        let after = &src[i + 5..];
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
        assert!(missing.is_empty(), "ts!() keys without a translation: {missing:#?}");
    }
}
