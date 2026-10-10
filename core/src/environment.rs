//! Whether Claude Code / Codex is installed on this machine (#197): on first launch with no sessions, the user must be told whether it is "not installed" or "installed but never used".
//!
//! No shell is spawned to run `which`: an app launched from Finder gets a very short PATH (`/usr/bin:/bin:...`), while the user's claude / codex is mostly installed in directories
//! that only the shell config adds to PATH (nvm, ~/.local/bin...). So two things are checked: (1) whether the command file exists (current PATH + a set of common install directories),
//! (2) whether the session directories exist (`~/.claude/projects`, `~/.codex/sessions`, which is exactly where makit reads).
//! If nothing is detected we can only say "not detected", never "not installed": it may be installed somewhere we did not list.

use std::path::{Path, PathBuf};

/// Why a directory read failed, distinguishing only "permission" from "other" (it goes into a `Copy` struct, so no error text)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrKind {
    PermissionDenied,
    Other,
}

/// The state of one session directory (#256 B1). **"Unreadable" and "empty" must be kept apart**: when unreadable the UI must not say "no sessions",
/// otherwise the user would think they have no sessions, rather than knowing the system refused the read
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DirState {
    /// The directory does not exist (this tool was never used)
    #[default]
    Missing,
    /// The directory exists and has no entries
    Empty,
    HasEntries,
    /// The directory exists but cannot be read
    Unreadable(ErrKind),
}

impl DirState {
    /// Whether there are signs that this tool was used: the directory exists (even if empty or unreadable)
    pub fn exists(&self) -> bool {
        !matches!(self, DirState::Missing)
    }
}

/// Probe a directory: one `read_dir`. NotFound -> Missing; insufficient permission -> Unreadable(PermissionDenied); other errors -> Unreadable(Other)
pub fn probe_dir(path: &Path) -> DirState {
    match std::fs::read_dir(path) {
        Ok(mut it) => {
            if it.next().is_some() {
                DirState::HasEntries
            } else {
                DirState::Empty
            }
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => DirState::Missing,
            std::io::ErrorKind::PermissionDenied => DirState::Unreadable(ErrKind::PermissionDenied),
            _ => DirState::Unreadable(ErrKind::Other),
        },
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolPresence {
    pub claude_bin: bool,
    pub codex_bin: bool,
    pub claude_dir: DirState,
    pub codex_dir: DirState,
}

impl ToolPresence {
    /// Any sign of Claude Code
    pub fn claude(&self) -> bool {
        self.claude_bin || self.claude_dir.exists()
    }

    pub fn codex(&self) -> bool {
        self.codex_bin || self.codex_dir.exists()
    }

    pub fn any(&self) -> bool {
        self.claude() || self.codex()
    }

    /// Session directories that cannot be read: (path for display, reason). The welcome card and the sidebar use this to say "cannot read" instead of "no sessions"
    pub fn unreadable_dirs(&self) -> Vec<(&'static str, ErrKind)> {
        let mut v = Vec::new();
        if let DirState::Unreadable(k) = self.claude_dir {
            v.push(("~/.claude/projects", k));
        }
        if let DirState::Unreadable(k) = self.codex_dir {
            v.push(("~/.codex/sessions", k));
        }
        v
    }
}

/// Common system-wide install directories (Homebrew). Merged into PATH only in the real entry point `detect()`, not in `detect_in`:
/// otherwise tests would pick up a claude / codex really installed on the dev machine, and the "nothing installed" case would not hold
const SYSTEM_BIN_DIRS: &str = "/opt/homebrew/bin:/usr/local/bin";

/// Common command install directories under the home directory (usually not in an App's PATH). nvm's per-version node directories are expanded separately in `nvm_bin_dirs`
pub fn common_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        home.join(".volta/bin"),
        home.join(".cargo/bin"),
    ];
    v.extend(nvm_bin_dirs(home));
    v
}

/// `~/.nvm/versions/node/<version>/bin`: nvm has one directory per node version, and globally installed commands live in each one's bin
fn nvm_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".nvm/versions/node");
    std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path().join("bin")).collect()
}

fn has_command(name: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| d.join(name).is_file())
}

/// Detect under `home`; `path_env` is the value of `$PATH` (colon separated)
pub fn detect_in(home: &Path, path_env: &str) -> ToolPresence {
    let mut dirs: Vec<PathBuf> = path_env.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    dirs.extend(common_bin_dirs(home));
    ToolPresence {
        claude_bin: has_command("claude", &dirs),
        codex_bin: has_command("codex", &dirs),
        claude_dir: probe_dir(&home.join(".claude/projects")),
        codex_dir: probe_dir(&home.join(".codex/sessions")),
    }
}

/// For debugging / development: `MAKIT_TOOLS=none|claude|codex|both` specifies the detection result directly, ignoring the real PATH and directories.
/// Why it exists: dev's fake HOME isolates only the session directories, while command detection still reads the dev machine's real PATH, so the "not installed" hint can never be seen.
/// An unrecognized value returns None (falls through to real detection) without an error
pub fn parse_override(value: &str) -> Option<ToolPresence> {
    let on = |claude, codex| Some(ToolPresence { claude_bin: claude, codex_bin: codex, ..Default::default() });
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => on(false, false),
        "claude" => on(true, false),
        "codex" => on(false, true),
        "both" => on(true, true),
        _ => None,
    }
}

/// Detect with the real HOME and PATH (if `MAKIT_TOOLS` is set, use the result it specifies)
pub fn detect() -> ToolPresence {
    if let Some(p) = std::env::var("MAKIT_TOOLS").ok().and_then(|v| parse_override(&v)) {
        return p;
    }
    match dirs::home_dir() {
        Some(h) => detect_in(&h, &format!("{}:{SYSTEM_BIN_DIRS}", std::env::var("PATH").unwrap_or_default())),
        None => ToolPresence::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Each test's own temporary HOME (pid + nanoseconds + name, without pulling in tempfile)
    fn temp_home(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("makit-env-{}-{n}-{name}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    /// Why test this (#197): what goes wrong on screen is "Claude Code is clearly installed, yet first launch says not found" (scares users away),
    /// or "not installed but reported as detected" (makes people think makit is broken)
    #[test]
    fn nothing_installed_is_all_false() {
        let h = temp_home("none");
        assert_eq!(detect_in(&h, ""), ToolPresence::default());
        assert!(!detect_in(&h, "").any());
    }

    #[cfg(unix)]
    #[test]
    fn command_in_path_is_found() {
        let h = temp_home("path");
        let bin = h.join("somewhere/bin");
        touch(&bin.join("claude"));
        let p = detect_in(&h, &format!("/usr/bin:{}", bin.display()));
        assert!(p.claude_bin && !p.codex_bin, "{p:?}");
    }

    #[test]
    fn command_in_common_dir_outside_path_is_found() {
        // An app launched from Finder has a very short PATH: a command installed in ~/.local/bin must be recognized too
        let h = temp_home("common");
        touch(&h.join(".local/bin/codex"));
        let p = detect_in(&h, "/usr/bin:/bin");
        assert!(p.codex_bin && !p.claude_bin, "{p:?}");
    }

    #[test]
    fn nvm_node_version_bins_are_scanned() {
        let h = temp_home("nvm");
        touch(&h.join(".nvm/versions/node/v22.1.0/bin/claude"));
        assert!(detect_in(&h, "").claude_bin);
    }

    #[test]
    fn session_directories_count_as_traces() {
        let h = temp_home("data");
        fs::create_dir_all(h.join(".claude/projects")).unwrap();
        let p = detect_in(&h, "");
        assert!(p.claude_dir.exists() && !p.codex_dir.exists() && p.claude() && !p.codex() && p.any(), "{p:?}");
        fs::create_dir_all(h.join(".codex/sessions")).unwrap();
        assert!(detect_in(&h, "").codex_dir.exists());
    }

    #[test]
    fn a_directory_named_like_the_command_is_not_the_command() {
        let h = temp_home("dir");
        fs::create_dir_all(h.join(".local/bin/claude")).unwrap(); // a directory, not an executable
        assert!(!detect_in(&h, "").claude_bin);
    }

    /// Why test this: if the override switch is wrong, the hint wanted in dev cannot appear, and one would again suspect the hint itself is broken
    #[test]
    fn override_presets() {
        let on = |c, x| ToolPresence { claude_bin: c, codex_bin: x, ..Default::default() };
        assert_eq!(parse_override("none"), Some(on(false, false)));
        assert_eq!(parse_override("claude"), Some(on(true, false)));
        assert_eq!(parse_override("codex"), Some(on(false, true)));
        assert_eq!(parse_override("both"), Some(on(true, true)));
        assert_eq!(parse_override(" BOTH "), Some(on(true, true)), "忽略大小写和空白");
        assert_eq!(parse_override("乱写"), None, "认不出就按真实检测走");
        assert_eq!(parse_override(""), None);
    }

    // ---- probe_dir (#256 B1) ----

    #[test]
    fn missing_empty_and_populated_dirs_are_told_apart() {
        let h = temp_home("probe");
        assert_eq!(probe_dir(&h.join("没有这个")), DirState::Missing);
        let empty = h.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(probe_dir(&empty), DirState::Empty, "空目录不是读不了");
        fs::write(empty.join("a"), "").unwrap();
        assert_eq!(probe_dir(&empty), DirState::HasEntries);
    }

    /// Why test this: if the UI says "no sessions" when a directory is unreadable, the user thinks they have no sessions instead of checking permissions.
    /// chmod 000 does not block root, so this case is skipped when run as root -- the control group must not depend on the running identity
    #[cfg(unix)]
    #[test]
    fn unreadable_dir_is_not_reported_as_empty() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("以 root 运行，chmod 拦不住读取，跳过");
            return;
        }
        let h = temp_home("denied");
        let d = h.join(".claude/projects");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("x"), "").unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o000)).unwrap();
        let state = probe_dir(&d);
        let p = detect_in(&h, "");
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap(); // restore first, or cleanup would fail
        assert_eq!(state, DirState::Unreadable(ErrKind::PermissionDenied));
        assert!(p.claude(), "目录在就算有迹象");
        assert_eq!(p.unreadable_dirs(), vec![("~/.claude/projects", ErrKind::PermissionDenied)]);
    }

    #[test]
    fn nothing_unreadable_by_default() {
        assert!(ToolPresence::default().unreadable_dirs().is_empty());
        assert!(!DirState::Missing.exists() && DirState::Empty.exists() && DirState::Unreadable(ErrKind::Other).exists());
    }
}
