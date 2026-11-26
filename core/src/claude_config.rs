//! Pre-trusting a directory for Claude Code (#D-2168): `open`'s `trust: true` only ever does this for a
//! directory under one of the user's own configured auto-trust roots (`NativeState.trust_roots`, default
//! empty) -- never silently, never for anything else.
//!
//! Writes `~/.claude.json`'s `projects.<absolute path>.hasTrustDialogAccepted`: confirmed on a real
//! `~/.claude.json` on this machine that this is the exact field Claude Code itself sets once you click through
//! its own trust dialog -- not guessed, not a separate flag makit invented. Nothing else in that entry, or
//! anywhere else in the file, is touched.

use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn claude_json_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

/// Pure: is `cwd` equal to, or inside, one of `roots`? Both sides are expected already canonicalized by the
/// caller (`std::fs::canonicalize`, which also resolves `..` and symlinks) -- comparing by path *components*
/// (`Path::starts_with`), not a raw string prefix, so `/Users/me/RustProjects2` is not mistaken for being under
/// a root of `/Users/me/RustProjects`.
pub fn is_under_trusted_root(cwd: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| cwd == r || cwd.starts_with(r))
}

/// Marks `project_path` (expected already canonicalized, matching what [`is_under_trusted_root`] checked)
/// trusted in the JSON file at `claude_json_path`, creating its `projects` entry if there was none yet.
///
/// Read-modify-write on the whole file, same shape [`crate::hook::install_claude_hook_in`] already uses for
/// `settings.json`: Claude Code itself, and every other session of it, also read and write this file, so this
/// is not a way to never race another writer -- it is the same compromise already made there. A lost update
/// here costs nothing worse than the trust dialog showing once more; a parse failure is reported and changes
/// nothing (never half-written, via a temp file + rename).
pub fn trust_directory_in(claude_json_path: &Path, project_path: &str) -> Result<(), String> {
    let raw = std::fs::read_to_string(claude_json_path).map_err(|e| e.to_string())?;
    let mut doc: Value = serde_json::from_str(&raw).map_err(|e| format!("~/.claude.json did not parse: {e}"))?;
    let obj = doc.as_object_mut().ok_or("~/.claude.json is not a JSON object")?;
    let projects = obj.entry("projects").or_insert_with(|| Value::Object(Default::default()));
    let projects = projects.as_object_mut().ok_or("~/.claude.json's \"projects\" is not an object")?;
    let entry = projects.entry(project_path.to_string()).or_insert_with(|| Value::Object(Default::default()));
    let entry = entry.as_object_mut().ok_or("the project's own entry in ~/.claude.json is not an object")?;
    entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    let tmp = claude_json_path.with_extension("json.makit-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, claude_json_path).map_err(|e| e.to_string())
}

pub fn trust_directory(project_path: &str) -> Result<(), String> {
    let path = claude_json_path().ok_or("no home directory")?;
    trust_directory_in(&path, project_path)
}

/// Claude Code's own `--permission-mode` choices (from `claude --help` on this machine, not guessed). Codex has
/// an entirely different vocabulary (`--ask-for-approval`, `--sandbox`) -- this is Claude-only, by design.
pub const PERMISSION_MODES: &[&str] = &["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];

pub fn is_known_permission_mode(mode: &str) -> bool {
    PERMISSION_MODES.contains(&mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_under_trusted_root_matches_by_path_components_not_a_string_prefix() {
        let roots = vec![PathBuf::from("/Users/me/RustProjects")];
        assert!(is_under_trusted_root(Path::new("/Users/me/RustProjects"), &roots), "the root itself counts");
        assert!(is_under_trusted_root(Path::new("/Users/me/RustProjects/makit"), &roots), "a child counts");
        assert!(is_under_trusted_root(Path::new("/Users/me/RustProjects/a/b/c"), &roots), "a grandchild counts");
        assert!(!is_under_trusted_root(Path::new("/Users/me/RustProjects2"), &roots), "a string prefix match is not a path match");
        assert!(!is_under_trusted_root(Path::new("/Users/me/other"), &roots));
        assert!(!is_under_trusted_root(Path::new("/Users/me"), &roots), "the parent of a root is not under it");
        assert!(!is_under_trusted_root(Path::new("/Users/me/RustProjects/x"), &[]), "no roots configured -> nothing is ever trusted");
    }

    #[test]
    fn is_under_trusted_root_checks_every_configured_root() {
        let roots = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        assert!(is_under_trusted_root(Path::new("/b/c"), &roots));
        assert!(!is_under_trusted_root(Path::new("/c"), &roots));
    }

    #[test]
    fn trust_directory_in_creates_a_missing_project_entry() {
        let dir = std::env::temp_dir().join(format!("makit-claude-json-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        fs::write(&path, r#"{"projects":{}}"#).unwrap();
        trust_directory_in(&path, "/Users/me/RustProjects/makit").unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["projects"]["/Users/me/RustProjects/makit"]["hasTrustDialogAccepted"], true);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trust_directory_in_keeps_the_projects_other_fields_and_other_projects() {
        let dir = std::env::temp_dir().join(format!("makit-claude-json-test2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        fs::write(
            &path,
            r#"{"numStartups": 42, "projects": {"/already/trusted": {"hasTrustDialogAccepted": true, "allowedTools": ["Bash"]}, "/Users/me/p": {"lastCost": 1.5}}}"#,
        )
        .unwrap();
        trust_directory_in(&path, "/Users/me/p").unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["numStartups"], 42, "untouched top-level fields stay as they were");
        assert_eq!(v["projects"]["/already/trusted"]["allowedTools"], serde_json::json!(["Bash"]), "another project's own fields are untouched");
        assert_eq!(v["projects"]["/Users/me/p"]["lastCost"], 1.5, "this project's existing fields are kept, not replaced");
        assert_eq!(v["projects"]["/Users/me/p"]["hasTrustDialogAccepted"], true, "and the new field is added");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trust_directory_in_reports_a_parse_failure_instead_of_touching_the_file() {
        let dir = std::env::temp_dir().join(format!("makit-claude-json-test3-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".claude.json");
        fs::write(&path, "not json at all").unwrap();
        assert!(trust_directory_in(&path, "/Users/me/p").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json at all", "left exactly as it was");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permission_mode_rejects_anything_not_in_claudes_own_real_list() {
        for m in PERMISSION_MODES {
            assert!(is_known_permission_mode(m));
        }
        assert!(!is_known_permission_mode("default"), "not a real claude choice, even though it sounds like one");
        assert!(!is_known_permission_mode(""));
        assert!(!is_known_permission_mode("yolo"));
    }
}
