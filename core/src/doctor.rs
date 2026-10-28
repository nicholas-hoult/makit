//! Environment check (#283): a read-only look at the local setup, one verdict per item, with the reason and a manual fix.
//!
//! Split in two so it can be tested without the machine it runs on:
//! - `collect()` gathers a `Snapshot` (runs `--version`, asks the login shell for its PATH, probes folders ...), slow, call it off the UI thread;
//! - `run_checks(&Snapshot)` is pure: snapshot in, verdicts out. Every verdict has a test built from a hand-made snapshot.
//!
//! Not checked on purpose (see the issue): exit IP / region / locale consistency, request-side "does it look like the official client".
//! Nothing is ever repaired here; the fix is text for the user.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::environment::{common_bin_dirs, probe_dir, DirState, ErrKind};
use crate::opencode::DbProbe;
use crate::ts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Pass,
    Warn,
    Fail,
    Unknown,
}

impl Level {
    pub fn label(self) -> String {
        match self {
            Level::Pass => ts!("doctor.level.pass"),
            Level::Warn => ts!("doctor.level.warn"),
            Level::Fail => ts!("doctor.level.fail"),
            Level::Unknown => ts!("doctor.level.unknown"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub id: &'static str,
    pub level: Level,
    pub title: String,
    pub detail: String,
    /// What the user can do by hand; empty when there is nothing to do
    pub fix: String,
}

fn check(id: &'static str, level: Level, title: String, detail: String, fix: String) -> Check {
    Check { id, level, title, detail, fix }
}

// ---------------------------------------------------------------- snapshot

#[derive(Clone, Debug, PartialEq)]
pub struct ToolProbe {
    pub name: &'static str,
    pub path: Option<PathBuf>,
    /// First line of `--version`; None when it did not answer (only meaningful if `path` is Some)
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketProbe {
    /// Something accepted our connection
    Listening,
    Missing,
    /// The file is there, nobody answers
    Stale,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookConfig {
    NoFile,
    Invalid,
    /// How many of `hook::HOOK_EVENTS` carry a makit hook
    Installed(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataDir {
    Missing,
    Writable,
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// PATH of this process (what an app started from the Dock gets)
    pub gui_path: String,
    /// PATH of the login shell, if it could be read
    pub login_path: Option<String>,
    /// Proxy variables that are set: (name, value)
    pub proxy_env: Vec<(String, String)>,
    pub tools: Vec<ToolProbe>,
    pub data_dir: DataDir,
    pub hook_socket: SocketProbe,
    pub hook_config: HookConfig,
    pub claude_projects: DirState,
    pub codex_sessions: DirState,
    /// (files sampled, files whose first line is not valid JSON)
    pub jsonl_sample: (usize, usize),
    pub opencode_db: DbProbe,
}

// ---------------------------------------------------------------- pure checks

pub fn run_checks(s: &Snapshot) -> Vec<Check> {
    let mut v = check_tools(&s.tools);
    v.push(check_path(&s.gui_path, s.login_path.as_deref(), &s.tools));
    v.push(check_proxy(&s.proxy_env));
    v.push(check_data_dir(s.data_dir));
    v.push(check_hook_socket(s.hook_socket));
    v.push(check_hook_config(s.hook_config));
    v.push(check_dir("dir.claude", ts!("doctor.dir.claude"), s.claude_projects));
    v.push(check_dir("dir.codex", ts!("doctor.dir.codex"), s.codex_sessions));
    v.push(check_jsonl(s.jsonl_sample));
    v.push(check_opencode(&s.opencode_db));
    v
}

/// Worst level of a set (Fail > Warn > Pass; Unknown does not count against it)
pub fn worst(checks: &[Check]) -> Level {
    if checks.iter().any(|c| c.level == Level::Fail) {
        Level::Fail
    } else if checks.iter().any(|c| c.level == Level::Warn) {
        Level::Warn
    } else {
        Level::Pass
    }
}

pub fn check_tools(tools: &[ToolProbe]) -> Vec<Check> {
    let any_found = tools.iter().any(|t| t.path.is_some());
    tools
        .iter()
        .map(|t| {
            let title = ts!("doctor.tool.title", name = t.name);
            let id = match t.name {
                "claude" => "tool.claude",
                "codex" => "tool.codex",
                _ => "tool.opencode",
            };
            match (&t.path, &t.version) {
                (Some(p), Some(ver)) => check(id, Level::Pass, title, ts!("doctor.tool.found", path = p.display(), version = ver), String::new()),
                (Some(p), None) => check(
                    id,
                    Level::Warn,
                    title,
                    ts!("doctor.tool.found_no_version", path = p.display()),
                    ts!("doctor.tool.fix_version", name = t.name),
                ),
                (None, _) if any_found => check(id, Level::Unknown, title, ts!("doctor.tool.optional_missing"), String::new()),
                (None, _) => check(id, Level::Warn, title, ts!("doctor.tool.none_found"), ts!("doctor.tool.fix_install")),
            }
        })
        .collect()
}

fn path_dirs(path: &str) -> Vec<&str> {
    path.split(':').filter(|d| !d.is_empty()).collect()
}

/// A GUI app often gets a short PATH. Only folders that hold a supported tool matter, so only those are reported.
pub fn check_path(gui: &str, login: Option<&str>, tools: &[ToolProbe]) -> Check {
    let title = ts!("doctor.path.title");
    let Some(login) = login else {
        return check("path", Level::Unknown, title, ts!("doctor.path.unknown"), String::new());
    };
    let gui_dirs = path_dirs(gui);
    let login_dirs = path_dirs(login);
    let mut missing: Vec<String> = Vec::new();
    for t in tools {
        let Some(dir) = t.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_string_lossy().into_owned()) else { continue };
        if login_dirs.contains(&dir.as_str()) && !gui_dirs.contains(&dir.as_str()) && !missing.contains(&dir) {
            missing.push(dir);
        }
    }
    if missing.is_empty() {
        check("path", Level::Pass, title, ts!("doctor.path.same"), String::new())
    } else {
        check("path", Level::Warn, title, ts!("doctor.path.differs", dirs = missing.join(", ")), ts!("doctor.path.fix"))
    }
}

/// `user:pass@` must never end up in a report that gets pasted into an issue.
pub fn mask_credentials(value: &str) -> String {
    match (value.find("://"), value.rfind('@')) {
        (Some(i), Some(at)) if at > i + 3 => format!("{}***@{}", &value[..i + 3], &value[at + 1..]),
        (None, Some(at)) => format!("***@{}", &value[at + 1..]),
        _ => value.to_string(),
    }
}

/// `scheme://host[:port]`, or bare `host:port`; host non-empty, port numeric when present.
pub fn proxy_looks_valid(value: &str) -> bool {
    let rest = match value.split_once("://") {
        Some((scheme, rest)) => {
            if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "socks5" | "socks5h" | "socks4" | "socks") {
                return false;
            }
            rest
        }
        None => value,
    };
    let rest = rest.rsplit('@').next().unwrap_or(rest).trim_end_matches('/');
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return false;
    }
    match rest.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && port.parse::<u16>().is_ok(),
        None => true,
    }
}

pub fn check_proxy(env: &[(String, String)]) -> Check {
    let title = ts!("doctor.proxy.title");
    if env.is_empty() {
        return check("proxy", Level::Pass, title, ts!("doctor.proxy.none"), String::new());
    }
    let show = |items: &[&(String, String)]| items.iter().map(|(k, v)| format!("{k}={}", mask_credentials(v))).collect::<Vec<_>>().join(", ");
    // NO_PROXY is a comma separated host list, not a proxy URL: it is shown but never validated as one
    let is_url_var = |k: &str| !k.eq_ignore_ascii_case("no_proxy");
    let bad: Vec<&(String, String)> = env.iter().filter(|(k, v)| is_url_var(k) && !proxy_looks_valid(v)).collect();
    if !bad.is_empty() {
        return check("proxy", Level::Warn, title, ts!("doctor.proxy.bad", list = show(&bad)), ts!("doctor.proxy.fix"));
    }
    let mismatched: Vec<&(String, String)> = env
        .iter()
        .filter(|(k, v)| env.iter().any(|(k2, v2)| k2 != k && k2.eq_ignore_ascii_case(k) && v2 != v))
        .collect();
    if !mismatched.is_empty() {
        return check("proxy", Level::Warn, title, ts!("doctor.proxy.mismatch", list = show(&mismatched)), ts!("doctor.proxy.fix"));
    }
    let all: Vec<&(String, String)> = env.iter().collect();
    check("proxy", Level::Pass, title, ts!("doctor.proxy.set", list = show(&all)), String::new())
}

pub fn check_data_dir(d: DataDir) -> Check {
    let title = ts!("doctor.data.title");
    match d {
        DataDir::Writable => check("data", Level::Pass, title, ts!("doctor.data.ok"), String::new()),
        DataDir::Missing => check("data", Level::Pass, title, ts!("doctor.data.missing"), String::new()),
        DataDir::ReadOnly => check("data", Level::Fail, title, ts!("doctor.data.readonly"), ts!("doctor.data.fix")),
    }
}

pub fn check_hook_socket(s: SocketProbe) -> Check {
    let title = ts!("doctor.hook.socket.title");
    let path = "~/.claude/makit/hook.sock";
    match s {
        SocketProbe::Listening => check("hook.socket", Level::Pass, title, ts!("doctor.hook.socket.ok", path = path), String::new()),
        SocketProbe::Missing => check("hook.socket", Level::Fail, title, ts!("doctor.hook.socket.missing", path = path), ts!("doctor.hook.socket.fix")),
        SocketProbe::Stale => check("hook.socket", Level::Fail, title, ts!("doctor.hook.socket.stale", path = path), ts!("doctor.hook.socket.fix")),
        SocketProbe::Unsupported => check("hook.socket", Level::Unknown, title, ts!("doctor.hook.socket.unsupported"), String::new()),
    }
}

pub fn check_hook_config(c: HookConfig) -> Check {
    let title = ts!("doctor.hook.config.title");
    let total = crate::hook::HOOK_EVENTS.len();
    let fix = ts!("doctor.hook.config.fix");
    match c {
        HookConfig::Installed(n) if n >= total => check("hook.config", Level::Pass, title, ts!("doctor.hook.config.ok"), String::new()),
        HookConfig::Installed(0) => check("hook.config", Level::Warn, title, ts!("doctor.hook.config.none"), fix),
        HookConfig::Installed(n) => check("hook.config", Level::Warn, title, ts!("doctor.hook.config.partial", n = n, total = total), fix),
        HookConfig::NoFile => check("hook.config", Level::Warn, title, ts!("doctor.hook.config.nofile"), fix),
        HookConfig::Invalid => check("hook.config", Level::Fail, title, ts!("doctor.hook.config.invalid"), fix),
    }
}

pub fn check_dir(id: &'static str, title: String, d: DirState) -> Check {
    match d {
        DirState::HasEntries => check(id, Level::Pass, title, ts!("doctor.dir.ok"), String::new()),
        DirState::Empty => check(id, Level::Pass, title, ts!("doctor.dir.empty"), String::new()),
        DirState::Missing => check(id, Level::Unknown, title, ts!("doctor.dir.missing"), String::new()),
        DirState::Unreadable(ErrKind::PermissionDenied) => check(id, Level::Fail, title, ts!("doctor.dir.unreadable"), ts!("doctor.dir.fix")),
        DirState::Unreadable(ErrKind::Other) => check(id, Level::Fail, title, ts!("doctor.dir.unreadable_other"), ts!("doctor.dir.fix")),
    }
}

pub fn check_jsonl((n, broken): (usize, usize)) -> Check {
    let title = ts!("doctor.jsonl.title");
    if n == 0 {
        check("jsonl", Level::Unknown, title, ts!("doctor.jsonl.none"), String::new())
    } else if broken == 0 {
        check("jsonl", Level::Pass, title, ts!("doctor.jsonl.ok", n = n), String::new())
    } else {
        check("jsonl", Level::Warn, title, ts!("doctor.jsonl.broken", n = n, broken = broken), ts!("doctor.jsonl.fix"))
    }
}

pub fn check_opencode(p: &DbProbe) -> Check {
    let title = ts!("doctor.opencode.title");
    match p {
        DbProbe::Ok(n) => check("opencode", Level::Pass, title, ts!("doctor.opencode.ok", n = n), String::new()),
        DbProbe::Missing => check("opencode", Level::Unknown, title, ts!("doctor.opencode.missing"), String::new()),
        DbProbe::BadSchema(why) => check("opencode", Level::Warn, title, ts!("doctor.opencode.bad_schema", why = why), ts!("doctor.opencode.fix_schema")),
        DbProbe::OpenFailed(why) => check("opencode", Level::Fail, title, ts!("doctor.opencode.open_failed", why = why), ts!("doctor.opencode.fix_open")),
    }
}

/// Plain text for the clipboard. The caller redacts the home folder (same rule as the log diagnostics).
pub fn report_text(checks: &[Check]) -> String {
    let mut s = ts!("doctor.report.header");
    for c in checks {
        s.push_str(&format!("[{}] {}: {}\n", c.level.label(), c.title, c.detail));
        if !c.fix.is_empty() {
            s.push_str(&format!("      -> {}\n", c.fix));
        }
    }
    s
}

// ---------------------------------------------------------------- collecting the real environment

/// Run a command, give up after `limit`. Returns stdout of a command that exited successfully.
fn run_with_timeout(cmd: &mut Command, limit: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    use std::io::Read;
                    let _ = so.read_to_string(&mut out);
                }
                return Some(out);
            }
            None if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn find_command(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

fn login_shell_path() -> Option<String> {
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;
        // -i so .zshrc is read too: that is where most people extend PATH (nvm, ~/.local/bin ...)
        let out = run_with_timeout(Command::new(shell).args(["-ilc", "printf %s \"$PATH\""]), Duration::from_secs(3))?;
        // an interactive shell may print banners before our output: PATH is the last line
        out.lines().last().map(|l| l.trim().to_string()).filter(|l| l.contains('/'))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn probe_tool(name: &'static str, dirs: &[PathBuf]) -> ToolProbe {
    let path = find_command(name, dirs);
    let version = path.as_ref().and_then(|p| {
        run_with_timeout(Command::new(p).arg("--version"), Duration::from_secs(5))
            .and_then(|o| o.lines().map(str::trim).find(|l| !l.is_empty()).map(String::from))
    });
    ToolProbe { name, path, version }
}

fn probe_data_dir(dir: &Path) -> DataDir {
    if !dir.is_dir() {
        return DataDir::Missing;
    }
    let probe = dir.join(format!(".doctor-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            DataDir::Writable
        }
        Err(_) => DataDir::ReadOnly,
    }
}

#[cfg(unix)]
fn probe_socket(path: &Path) -> SocketProbe {
    if !path.exists() {
        return SocketProbe::Missing;
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => SocketProbe::Listening,
        Err(_) => SocketProbe::Stale,
    }
}

#[cfg(not(unix))]
fn probe_socket(_path: &Path) -> SocketProbe {
    SocketProbe::Unsupported
}

/// How many of `HOOK_EVENTS` have a makit hook in `settings.json` (same marker `install_claude_hook` writes).
pub fn hook_config_of(settings: Option<&str>) -> HookConfig {
    let Some(raw) = settings else { return HookConfig::NoFile };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else { return HookConfig::Invalid };
    let has_makit = |groups: &serde_json::Value| {
        groups.as_array().is_some_and(|gs| {
            gs.iter().any(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .is_some_and(|cmds| cmds.iter().any(|c| c.get("command").and_then(|x| x.as_str()).is_some_and(|s| s.contains("makit-hook.sh"))))
            })
        })
    };
    let n = crate::hook::HOOK_EVENTS.iter().filter(|e| v.pointer(&format!("/hooks/{e}")).is_some_and(&has_makit)).count();
    HookConfig::Installed(n)
}

/// Sample the newest session files: does the first non-empty line parse as JSON? (a crashed write leaves garbage or nothing)
fn sample_jsonl(projects: &Path, limit: usize) -> (usize, usize) {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for proj in std::fs::read_dir(projects).into_iter().flatten().flatten() {
        if !proj.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for f in std::fs::read_dir(proj.path()).into_iter().flatten().flatten() {
            if crate::sessions::is_scannable_jsonl(&f) {
                if let Ok(m) = f.metadata().and_then(|m| m.modified()) {
                    files.push((m, f.path()));
                }
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut checked = 0;
    let mut broken = 0;
    for (_, p) in files.into_iter().take(limit) {
        use std::io::BufRead;
        let Ok(f) = std::fs::File::open(&p) else { continue };
        let first = std::io::BufReader::new(f).lines().map_while(Result::ok).find(|l| !l.trim().is_empty());
        // an empty file is a session that never got a message: not damage
        let Some(line) = first else { continue };
        checked += 1;
        if serde_json::from_str::<serde_json::Value>(&line).is_err() {
            broken += 1;
        }
    }
    (checked, broken)
}

/// Gather the real environment. Slow (login shell, `--version` x3): call it off the UI thread.
pub fn collect() -> Snapshot {
    let home = dirs::home_dir().unwrap_or_default();
    let gui_path = std::env::var("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = path_dirs(&gui_path).into_iter().map(PathBuf::from).collect();
    // the same fallbacks `environment::detect` uses, so the check and the app's own detection agree
    dirs.extend(common_bin_dirs(&home));
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));

    let proxy_env = ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()).map(|v| (k.to_string(), v)))
        .collect();

    let makit_dir = home.join(".claude").join("makit");
    let projects = home.join(".claude").join("projects");
    let settings = std::fs::read_to_string(home.join(".claude").join("settings.json")).ok();
    Snapshot {
        gui_path,
        login_path: login_shell_path(),
        proxy_env,
        tools: ["claude", "codex", "opencode"].into_iter().map(|n| probe_tool(n, &dirs)).collect(),
        data_dir: probe_data_dir(&makit_dir),
        hook_socket: probe_socket(&makit_dir.join("hook.sock")),
        hook_config: hook_config_of(settings.as_deref()),
        claude_projects: probe_dir(&projects),
        codex_sessions: probe_dir(&home.join(".codex").join("sessions")),
        jsonl_sample: sample_jsonl(&projects, 20),
        opencode_db: crate::opencode::db_path().map(|p| crate::opencode::probe(&p)).unwrap_or(DbProbe::Missing),
    }
}

pub fn run() -> Vec<Check> {
    run_checks(&collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &'static str, path: Option<&str>, version: Option<&str>) -> ToolProbe {
        ToolProbe { name, path: path.map(PathBuf::from), version: version.map(String::from) }
    }

    fn healthy() -> Snapshot {
        Snapshot {
            gui_path: "/usr/bin:/bin".into(),
            login_path: Some("/usr/bin:/bin".into()),
            proxy_env: vec![],
            tools: vec![tool("claude", Some("/usr/local/bin/claude"), Some("2.1.0")), tool("codex", None, None), tool("opencode", None, None)],
            data_dir: DataDir::Writable,
            hook_socket: SocketProbe::Listening,
            hook_config: HookConfig::Installed(4),
            claude_projects: DirState::HasEntries,
            codex_sessions: DirState::Missing,
            jsonl_sample: (20, 0),
            opencode_db: DbProbe::Missing,
        }
    }

    fn level_of(checks: &[Check], id: &str) -> Level {
        checks.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no check {id}")).level
    }

    /// Why: the baseline. A healthy machine must come out clean, or every report starts with noise and people stop reading it.
    #[test]
    fn a_healthy_machine_has_no_warnings_or_failures() {
        let checks = run_checks(&healthy());
        assert_eq!(worst(&checks), Level::Pass, "{checks:#?}");
        assert_eq!(level_of(&checks, "tool.codex"), Level::Unknown, "an unused optional tool is not a problem");
    }

    #[test]
    fn no_tool_at_all_is_a_warning_with_an_install_hint() {
        let c = check_tools(&[tool("claude", None, None), tool("codex", None, None), tool("opencode", None, None)]);
        assert!(c.iter().all(|x| x.level == Level::Warn && !x.fix.is_empty()), "{c:?}");
    }

    #[test]
    fn a_tool_that_does_not_answer_version_is_a_warning() {
        let c = check_tools(&[tool("claude", Some("/x/claude"), None)]);
        assert_eq!(c[0].level, Level::Warn);
        assert!(c[0].fix.contains("claude --version"), "{}", c[0].fix);
    }

    /// Why: the classic "works in my terminal, not from the Dock" case. Only folders that hold a tool are worth reporting.
    #[test]
    fn path_difference_matters_only_for_folders_that_hold_a_tool() {
        let tools = [tool("claude", Some("/Users/me/.nvm/bin/claude"), Some("1"))];
        let c = check_path("/usr/bin:/bin", Some("/usr/bin:/bin:/Users/me/.nvm/bin:/Users/me/.other"), &tools);
        assert_eq!(c.level, Level::Warn);
        assert!(c.detail.contains("/Users/me/.nvm/bin") && !c.detail.contains(".other"), "{}", c.detail);
        let same = check_path("/usr/bin:/Users/me/.nvm/bin", Some("/usr/bin:/Users/me/.nvm/bin"), &tools);
        assert_eq!(same.level, Level::Pass);
        assert_eq!(check_path("/usr/bin", None, &tools).level, Level::Unknown, "shell did not answer: say so, do not guess");
    }

    #[test]
    fn proxy_value_validation() {
        for ok in ["http://127.0.0.1:7890", "https://proxy.corp:8080", "socks5://localhost:1080", "127.0.0.1:7890", "http://user:pw@host:3128/", "http://host"] {
            assert!(proxy_looks_valid(ok), "{ok}");
        }
        for bad in ["", "http://", "http://host:notaport", "ftp://host:21", "host with space:80", "http://:8080", "127.0.0.1:99999"] {
            assert!(!proxy_looks_valid(bad), "{bad}");
        }
    }

    /// Why: this text gets pasted into public issues. A password in a proxy URL must never go with it.
    #[test]
    fn credentials_in_proxy_urls_are_masked() {
        assert_eq!(mask_credentials("http://user:secret@host:8080"), "http://***@host:8080");
        assert_eq!(mask_credentials("user:secret@host:8080"), "***@host:8080");
        assert_eq!(mask_credentials("http://host:8080"), "http://host:8080");
        let c = check_proxy(&[("HTTPS_PROXY".into(), "http://u:hunter2@h:1".into())]);
        assert!(!c.detail.contains("hunter2") && !c.detail.contains("u:"), "{}", c.detail);
    }

    #[test]
    fn proxy_check_levels() {
        assert_eq!(check_proxy(&[]).level, Level::Pass);
        let ok = check_proxy(&[("HTTPS_PROXY".into(), "http://127.0.0.1:7890".into()), ("NO_PROXY".into(), "localhost,127.0.0.1".into())]);
        assert_eq!(ok.level, Level::Pass, "NO_PROXY is a host list, not a URL: {ok:?}");
        assert_eq!(check_proxy(&[("http_proxy".into(), "garbage://".into())]).level, Level::Warn);
        let mismatch = check_proxy(&[("HTTP_PROXY".into(), "http://a:1".into()), ("http_proxy".into(), "http://b:2".into())]);
        assert_eq!(mismatch.level, Level::Warn);
        assert!(mismatch.detail.contains("HTTP_PROXY") && mismatch.detail.contains("http_proxy"));
        let same = check_proxy(&[("HTTP_PROXY".into(), "http://a:1".into()), ("http_proxy".into(), "http://a:1".into())]);
        assert_eq!(same.level, Level::Pass);
    }

    #[test]
    fn data_dir_levels() {
        assert_eq!(check_data_dir(DataDir::Writable).level, Level::Pass);
        assert_eq!(check_data_dir(DataDir::Missing).level, Level::Pass, "created on first use");
        assert_eq!(check_data_dir(DataDir::ReadOnly).level, Level::Fail);
    }

    /// Why: "notifications do not come" has exactly two causes here: the channel is down, or Claude Code was never told about it.
    #[test]
    fn hook_channel_and_config_are_judged_separately() {
        assert_eq!(check_hook_socket(SocketProbe::Listening).level, Level::Pass);
        assert_eq!(check_hook_socket(SocketProbe::Missing).level, Level::Fail);
        assert_eq!(check_hook_socket(SocketProbe::Stale).level, Level::Fail);
        assert_eq!(check_hook_socket(SocketProbe::Unsupported).level, Level::Unknown);
        let total = crate::hook::HOOK_EVENTS.len();
        assert_eq!(check_hook_config(HookConfig::Installed(total)).level, Level::Pass);
        assert_eq!(check_hook_config(HookConfig::Installed(0)).level, Level::Warn);
        assert_eq!(check_hook_config(HookConfig::Installed(1)).level, Level::Warn);
        assert_eq!(check_hook_config(HookConfig::NoFile).level, Level::Warn);
        assert_eq!(check_hook_config(HookConfig::Invalid).level, Level::Fail);
    }

    #[test]
    fn hook_config_is_read_from_settings_json() {
        assert_eq!(hook_config_of(None), HookConfig::NoFile);
        assert_eq!(hook_config_of(Some("{not json")), HookConfig::Invalid);
        assert_eq!(hook_config_of(Some("{}")), HookConfig::Installed(0));
        let mk = |events: &[&str]| {
            let hooks: serde_json::Map<String, serde_json::Value> = events
                .iter()
                .map(|e| (e.to_string(), serde_json::json!([{"hooks": [{"type": "command", "command": "/h/.claude/hooks/makit-hook.sh 2>/dev/null || echo '{}'"}]}])))
                .collect();
            serde_json::json!({"hooks": hooks}).to_string()
        };
        assert_eq!(hook_config_of(Some(&mk(&["Notification", "Stop"]))), HookConfig::Installed(2));
        assert_eq!(hook_config_of(Some(&mk(&crate::hook::HOOK_EVENTS))), HookConfig::Installed(crate::hook::HOOK_EVENTS.len()));
        // someone else's hook on the same event is not ours
        let other = serde_json::json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]}}).to_string();
        assert_eq!(hook_config_of(Some(&other)), HookConfig::Installed(0));
    }

    /// Why: "no sessions" and "cannot read sessions" are different problems (#256 B1); the check must not blur them.
    #[test]
    fn session_folder_levels() {
        let t = || "t".to_string();
        assert_eq!(check_dir("d", t(), DirState::HasEntries).level, Level::Pass);
        assert_eq!(check_dir("d", t(), DirState::Empty).level, Level::Pass);
        assert_eq!(check_dir("d", t(), DirState::Missing).level, Level::Unknown);
        assert_eq!(check_dir("d", t(), DirState::Unreadable(ErrKind::PermissionDenied)).level, Level::Fail);
        assert_eq!(check_dir("d", t(), DirState::Unreadable(ErrKind::Other)).level, Level::Fail);
    }

    #[test]
    fn jsonl_levels() {
        assert_eq!(check_jsonl((0, 0)).level, Level::Unknown);
        assert_eq!(check_jsonl((20, 0)).level, Level::Pass);
        let c = check_jsonl((20, 3));
        assert_eq!(c.level, Level::Warn);
        assert!(c.detail.contains('3') && c.detail.contains("20"), "{}", c.detail);
    }

    #[test]
    fn opencode_levels() {
        assert_eq!(check_opencode(&DbProbe::Ok(5)).level, Level::Pass);
        assert_eq!(check_opencode(&DbProbe::Missing).level, Level::Unknown);
        assert_eq!(check_opencode(&DbProbe::BadSchema("no column".into())).level, Level::Warn);
        assert_eq!(check_opencode(&DbProbe::OpenFailed("denied".into())).level, Level::Fail);
    }

    #[test]
    fn worst_ignores_unknown_and_ranks_fail_over_warn() {
        let mk = |l| Check { id: "x", level: l, title: String::new(), detail: String::new(), fix: String::new() };
        assert_eq!(worst(&[mk(Level::Unknown), mk(Level::Pass)]), Level::Pass);
        assert_eq!(worst(&[mk(Level::Warn), mk(Level::Pass)]), Level::Warn);
        assert_eq!(worst(&[mk(Level::Warn), mk(Level::Fail)]), Level::Fail);
    }

    #[test]
    fn report_text_has_a_line_per_check_and_the_fix_under_it() {
        let mut s = healthy();
        s.data_dir = DataDir::ReadOnly;
        let text = report_text(&run_checks(&s));
        assert!(text.lines().filter(|l| l.starts_with('[')).count() == run_checks(&s).len());
        assert!(text.contains("->"), "a failing item carries its fix: {text}");
    }

    /// Why: `collect` must work on a real machine without panicking or hanging. It cannot assert what it finds (that is the machine's business).
    #[test]
    fn collect_runs_here_and_yields_a_check_for_every_item() {
        let snap = collect();
        assert_eq!(snap.tools.len(), 3);
        let checks = run_checks(&snap);
        assert!(checks.len() >= 11, "{}", checks.len());
    }

    #[test]
    fn a_command_that_hangs_is_killed_at_the_deadline() {
        let t = Instant::now();
        assert!(run_with_timeout(Command::new("sleep").arg("5"), Duration::from_millis(200)).is_none());
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert_eq!(run_with_timeout(Command::new("echo").arg("hi"), Duration::from_secs(2)).as_deref(), Some("hi\n"));
    }

    /// Look at the real report: `cargo test doctor::tests::print_real_report -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_real_report() {
        println!("{}", report_text(&run()));
    }
}
