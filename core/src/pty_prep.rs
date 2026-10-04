//! Pure logic before PTY launch: cwd expansion / fallback, the cwd gate, the resume session id, correcting the session's start directory.
//! The spawn itself is implemented separately on each side (Tauri uses portable-pty, GPUI uses the alacritty tty).

use std::path::{Path, PathBuf};

// When the cwd does not exist (the session moved the directory itself, or it was deleted externally), look upward for the nearest existing ancestor
// if none is found fall back to home, and failing that "/" -- so the shell can always start
pub fn expand_tilde(cwd: &str) -> String {
    if cwd == "~" {
        return dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());
    }
    if let Some(rest) = cwd.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    cwd.to_string()
}

pub fn resolve_existing_cwd(cwd: &str) -> String {
    let expanded = expand_tilde(cwd);
    if Path::new(&expanded).is_dir() {
        return expanded;
    }
    let mut p = PathBuf::from(&expanded);
    while p.pop() {
        if p.is_dir() {
            return p.to_string_lossy().into_owned();
        }
    }
    dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

/// Whether this pane should refuse to start. It is pulled out as a pure predicate only so it can be tested -- `pty_spawn` itself needs
/// a `Window` + `State`, which tests cannot construct (the same reason `kill_tree` was
/// extracted from `kill_pty`).
pub fn must_refuse_cwd(cwd: &str, allow_fallback: Option<bool>) -> bool {
    allow_fallback == Some(false) && resolve_existing_cwd(cwd) != expand_tilde(cwd)
}

/// The session id inside `clear && claude -r <id>` (`resumeInitCommand` in `workspace-types.ts`).
/// The id gets spliced into a file path, so only the uuid shape (36 hex characters and `-`) is accepted.
pub fn resume_session_id(init_command: &str) -> Option<&str> {
    let id = init_command.split("claude -r ").nth(1)?.split_whitespace().next()?;
    let uuid_shaped = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    uuid_shaped.then_some(id)
}

/// The session's start directory: the cwd of the first main-line (non-sidechain) user record. Later records get rewritten by "`claude -r` in another directory"
/// and must not be used.
pub fn first_user_cwd(reader: impl std::io::BufRead) -> Option<String> {
    for line in reader.lines().map_while(Result::ok) {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if rec.get("type").and_then(|v| v.as_str()) != Some("user") { continue; }
        if rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) { continue; }
        if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()).filter(|c| !c.is_empty()) {
            return Some(c.to_string());
        }
    }
    None
}

/// Whether a resume tab should be launched in the session's start directory instead: only when the start directory **still exists** and differs from the requested one.
/// A start directory that is gone = the project was moved (the user may already have recovered with "point to new location"), so the new location recorded by the tab is the right one,
/// and forcing it back would fight with the cwd-missing recovery flow.
pub fn corrected_resume_cwd(requested: &str, home: Option<String>) -> Option<String> {
    home.filter(|h| Path::new(h).is_dir() && *h != expand_tilde(requested))
}

/// Find this session in `~/.claude/projects/*/<id>.jsonl` and return its start directory.
pub fn session_home_cwd(projects_dir: &Path, session_id: &str) -> Option<String> {
    let file = std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join(format!("{session_id}.jsonl")))
        .find(|p| p.is_file())?;
    first_user_cwd(std::io::BufReader::new(std::fs::File::open(file).ok()?))
}

/// The cwd gate for resume tabs (#173).
///
/// The invariant here is: **a resume tab whose directory is gone must fail to start, not quietly start in another directory**.
/// Changing the cwd changes the key -- claude only looks for the session at `projects/<encode(realpath(cwd))>/<id>.jsonl`,
/// and after degrading to an ancestor directory it reports "No conversation found", disguising a fixable problem
/// as an unrecoverable one.
/// Launch a resume tab in the session's own start directory (#190): when `init_command` has the shape `claude -r <uuid>`
/// and the session's start directory still exists and differs from `cwd`, return the directory to switch to; otherwise None (launch in `cwd`).
/// Both sides run this step before spawning, then pass through the `must_refuse_cwd` gate.
pub fn resume_cwd_correction(init_command: Option<&str>, cwd: &str) -> Option<String> {
    let home = init_command
        .and_then(resume_session_id)
        .and_then(|sid| dirs::home_dir().and_then(|h| session_home_cwd(&h.join(".claude").join("projects"), sid)));
    corrected_resume_cwd(cwd, home)
}

/// The value to inject as `MAKIT_SESSION_ID`: the session id in the resume session's initCommand (claude or codex).
///
/// #223: it used to accept only strings "starting with `claude -r `", but the frontend assembles `clear && claude -r <id>`, so this variable was never
/// injected. Now it shares the parsing of `resume_session_id` with the cwd correction, and likewise accepts only uuid-shaped ids.
pub fn env_session_id(init_command: &str) -> Option<&str> {
    resume_session_id(init_command).or_else(|| {
        let id = init_command.split("codex resume ").nth(1)?.split_whitespace().next()?;
        let uuid_shaped = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        uuid_shaped.then_some(id)
    })
}

/// Shell integration is done only for zsh (macOS's default shell is zsh)
pub fn is_zsh(shell: &str) -> bool {
    shell.ends_with("zsh") || shell.ends_with("/zsh")
}

/// Shell integration: automatically send OSC 7 to notify cwd changes.
///
/// Take over zsh's .zshrc loading point with ZDOTDIR: first source the user's original .zshrc, then add the OSC 7 hook.
/// Write `.zprofile` / `.zshenv` / `.zshrc` under `<home>/.cache/makit/shell-integration/`
/// (not written if the content is unchanged) and return that directory -- the caller sets it as the child process's `ZDOTDIR`.
pub fn prepare_zsh_integration(home: &Path) -> PathBuf {
    let integ_dir = home.join(".cache").join("makit").join("shell-integration");
    let _ = std::fs::create_dir_all(&integ_dir);

    let write_if_changed = |path: &std::path::Path, content: &str| {
        let current = std::fs::read_to_string(path).unwrap_or_default();
        if current != content {
            let _ = std::fs::write(path, content);
        }
    };

    let user_zprofile = home.join(".zprofile");
    write_if_changed(&integ_dir.join(".zprofile"), &format!(
        "# makit: source user .zprofile for PATH (brew/cargo/nvm)\n\
         [ -f {p} ] && source {p}\n",
        p = user_zprofile.display(),
    ));

    let user_zshenv = home.join(".zshenv");
    write_if_changed(&integ_dir.join(".zshenv"), &format!(
        "[ -f {p} ] && source {p}\n",
        p = user_zshenv.display(),
    ));

    let user_zshrc = home.join(".zshrc");
    write_if_changed(&integ_dir.join(".zshrc"), &format!(
        "# makit shell integration (auto-generated)\n\
         [ -f {user_rc} ] && source {user_rc}\n_makit_emit_cwd() {{ printf '\\033]7;file://%s%s\\033\\\\' \"$HOST\" \"$PWD\" }}\n\
         typeset -ga chpwd_functions precmd_functions\n\
         chpwd_functions+=(_makit_emit_cwd)\n\
         precmd_functions+=(_makit_emit_cwd)\n\
         _makit_emit_cwd\n",
        user_rc = user_zshrc.display(),
    ));
    integ_dir
}

#[cfg(test)]
mod cwd_gate_tests {
    use super::{expand_tilde, must_refuse_cwd, resolve_existing_cwd};

    #[test]
    fn refuses_only_when_fallback_is_forbidden_and_dir_is_gone() {
        let gone = "/definitely/not/a/path/makit-cwd-gate";
        // Precondition: this path really does trigger degradation, otherwise the assertions below test nothing
        assert_ne!(resolve_existing_cwd(gone), expand_tilde(gone));

        // resume tab: must refuse
        assert!(must_refuse_cwd(gone, Some(false)));

        // shell / new tab: degrading is right (the goal is just to get a shell up; any directory will do)
        assert!(!must_refuse_cwd(gone, Some(true)));
        // an old caller does not pass this parameter -> None -> keep the original degrading behaviour, backward compatible
        assert!(!must_refuse_cwd(gone, None));
    }

    #[test]
    fn never_refuses_when_the_dir_is_actually_there() {
        let here = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(!must_refuse_cwd(&here, Some(false)), "目录在就不该拦");
    }
}

/// A resume tab must launch in the session's own start directory (#190).
///
/// Cause: after typing `claude -r` by hand in a shell, the tab is upgraded in place to a resume tab, but the cwd the tab recorded is still that shell's
/// directory; and a newly opened terminal inherits the current tab's cwd -- one wrong directory propagates to every tab. On the next launch `claude -r`
/// runs in the wrong directory and claude continues this session inside that repository (the cwd in the records, the git branch and the Bash default directory all change).
/// The session's start directory = the cwd of the first user record in the jsonl = the directory claude stores it under, the only trustworthy source.
#[cfg(test)]
mod resume_cwd_tests {
    use super::{corrected_resume_cwd, first_user_cwd, resume_session_id, session_home_cwd};
    use std::io::Cursor;

    const SID: &str = "73ec5479-5b96-494c-919c-1f36a7e192fd";

    #[test]
    fn parses_session_id_from_resume_init_command() {
        assert_eq!(resume_session_id(&format!("clear && claude -r {SID}")), Some(SID));
        assert_eq!(resume_session_id(&format!("claude -r {SID}")), Some(SID));
        // codex sessions are not in ~/.claude/projects, ignore them
        assert_eq!(resume_session_id(&format!("codex resume {SID}")), None);
        assert_eq!(resume_session_id("clear && claude"), None);
        assert_eq!(resume_session_id("zsh"), None);
        // The id gets spliced into a file path: anything not uuid-shaped is rejected, ruling out ../ and the like
        assert_eq!(resume_session_id("claude -r ../../etc/passwd"), None);
        assert_eq!(resume_session_id("claude -r 73ec5479"), None);
    }

    #[test]
    fn first_user_cwd_skips_non_user_and_sidechain_records() {
        let jsonl = [
            r#"{"type":"summary","summary":"x"}"#,
            r#"{"type":"user","isSidechain":true,"cwd":"/tmp/sidechain"}"#,
            r#"not json"#,
            r#"{"type":"user","cwd":""}"#,
            r#"{"type":"user","cwd":"/Users/me/RustProjects/makit"}"#,
            r#"{"type":"user","cwd":"/Users/me/other-project"}"#,
        ]
        .join("\n");
        assert_eq!(
            first_user_cwd(Cursor::new(jsonl)).as_deref(),
            Some("/Users/me/RustProjects/makit"),
            "起始目录是第一条有 cwd 的主线用户记录，后面被改写的不算"
        );
        assert_eq!(first_user_cwd(Cursor::new(r#"{"type":"summary"}"#)), None);
    }

    #[test]
    fn corrects_only_to_an_existing_home_that_differs() {
        let home = std::env::temp_dir().to_string_lossy().trim_end_matches('/').to_string();
        // resumed in another directory -> change back to the start directory
        assert_eq!(corrected_resume_cwd("/Users/me/other-project", Some(home.clone())), Some(home.clone()));
        // already in the start directory -> no change (the frontend is not notified)
        assert_eq!(corrected_resume_cwd(&home, Some(home.clone())), None);
        // the start directory is gone (directory moved, the user recovered with "point to new location") -> no change,
        // otherwise it would fight with the cwd-missing recovery flow: change back -> refused -> relink to the new directory -> changed back again
        assert_eq!(corrected_resume_cwd("/Users/me/new-place", Some("/definitely/not/a/path/makit-home".into())), None);
        // session record not found -> no change
        assert_eq!(corrected_resume_cwd("/Users/me/x", None), None);
    }

    #[test]
    fn finds_session_file_under_any_project_folder() {
        let root = std::env::temp_dir().join(format!("makit-resume-cwd-{}", std::process::id()));
        let proj = root.join("-Users-me-RustProjects-makit");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(root.join("-Users-me-other")).unwrap();
        std::fs::write(
            proj.join(format!("{SID}.jsonl")),
            r#"{"type":"user","cwd":"/Users/me/RustProjects/makit"}"#,
        )
        .unwrap();

        assert_eq!(session_home_cwd(&root, SID).as_deref(), Some("/Users/me/RustProjects/makit"));
        assert_eq!(session_home_cwd(&root, "00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(session_home_cwd(&root.join("nope"), SID), None, "projects 目录不存在也不 panic");
        std::fs::remove_dir_all(&root).ok();
    }
}

/// Pre-spawn preparation extracted in #226 (shared by Tauri and GPUI).
/// What goes wrong on screen if this breaks: a terminal tab's cwd does not follow `cd` (the OSC 7 integration files are wrong),
/// or the user's own .zshrc / PATH is not loaded (the integration files did not source the original).
#[cfg(test)]
mod spawn_prep_tests {
    use super::*;

    #[test]
    /// #223: a resume tab's initCommand is actually `clear && claude -r <id>` (`resumeInitCommand` in workspace-types.ts);
    /// it used to accept only strings "starting with `claude -r`", MAKIT_SESSION_ID was never injected,
    /// and looking up orphan processes by session failed for every resumed tab. What goes wrong on screen: after closing a resumed tab,
    /// the child processes it setsid'ed away are not reaped as that session's children.
    fn env_session_id_for_real_resume_commands() {
        const SID: &str = "73ec5479-5b96-494c-919c-1f36a7e192fd";
        assert_eq!(env_session_id(&format!("clear && claude -r {SID}")), Some(SID), "前端实际拼的形状");
        assert_eq!(env_session_id(&format!("claude -r {SID}")), Some(SID));
        assert_eq!(env_session_id(&format!("clear && codex resume {SID}")), Some(SID), "codex 也要");
        assert_eq!(env_session_id("claude -r "), None);
        assert_eq!(env_session_id("clear && claude"), None, "新会话没有 id");
        assert_eq!(env_session_id("zsh"), None);
        // It goes into the environment and is also used to match processes: anything not uuid-shaped is rejected
        assert_eq!(env_session_id("claude -r abc"), None);
        assert_eq!(env_session_id("codex resume ../../x"), None);
    }

    #[test]
    fn zsh_integration_sources_user_files_and_emits_osc7() {
        let home = std::env::temp_dir().join(format!("makit-zdot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let dir = prepare_zsh_integration(&home);
        assert_eq!(dir, home.join(".cache/makit/shell-integration"));
        let rc = std::fs::read_to_string(dir.join(".zshrc")).unwrap();
        assert!(rc.contains(&format!("source {}", home.join(".zshrc").display())), "先 source 用户原 .zshrc");
        assert!(rc.contains("\\033]7;file://%s%s\\033\\\\"), "OSC 7：ESC ] 7 ; file://主机路径 ESC \\\\，实际:\n{rc}");
        assert!(rc.contains("chpwd_functions+=(_makit_emit_cwd)") && rc.contains("precmd_functions+=(_makit_emit_cwd)"));
        assert!(std::fs::read_to_string(dir.join(".zprofile")).unwrap().contains(".zprofile"));
        assert!(std::fs::read_to_string(dir.join(".zshenv")).unwrap().contains(".zshenv"));
        // second call: content unchanged
        assert_eq!(prepare_zsh_integration(&home), dir);
        assert_eq!(std::fs::read_to_string(dir.join(".zshrc")).unwrap(), rc);
        assert!(is_zsh("/bin/zsh") && !is_zsh("/bin/bash"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
