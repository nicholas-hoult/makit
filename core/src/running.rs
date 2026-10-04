//! Running state: `~/.claude/sessions/*.json` (pid / status / name written by claude while running),
//! and the pty_id -> session binding.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::process::get_env_var_of_pid;

#[derive(Clone, Debug)]
pub struct RunningInfo {
    pub name: String,
    pub status: String,
    pub pty_id: String,
    pub waiting_for: String,
    pub pid: u32,
}

pub fn load_running_info() -> HashMap<String, RunningInfo> {
    match dirs::home_dir() {
        Some(h) => load_running_info_in(&h.join(".claude").join("sessions"), &pid_alive),
        None => HashMap::new(),
    }
}

/// Testable version of `load_running_info`: a directory + "is this pid still alive?".
/// When a process is killed / crashes / its terminal is closed, claude does not delete `sessions/<pid>.json`: the file stays but the process is gone --
/// such an entry must not count as "running", otherwise the sidebar shows running forever and even blocks reopening ("already running, cannot start twice")
pub fn load_running_info_in(dir: &Path, alive: &dyn Fn(u32) -> bool) -> HashMap<String, RunningInfo> {
    let mut map = HashMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return map,
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let status = v
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let waiting_for = v
            .get("waitingFor")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if pid > 0 && !alive(pid) {
            continue; // file left behind by a dead process
        }
        // Check the claude process's environment variables to get MAKIT_PTY_ID
        let pty_id = if pid > 0 {
            get_env_var_of_pid(pid, "MAKIT_PTY_ID").unwrap_or_default()
        } else {
            String::new()
        };
        map.insert(
            session_id,
            RunningInfo {
                name,
                status,
                waiting_for,
                pid,
                pty_id,
            },
        );
    }
    map
}

// Lightweight scan: reads only the running state (~/.claude/sessions/*.json), does not scan projects
pub fn list_running_sessions() -> Vec<RunningMeta> {
    match dirs::home_dir() {
        Some(h) => list_running_sessions_in(&h.join(".claude").join("sessions"), &pid_alive),
        None => vec![],
    }
}

/// Testable version of `list_running_sessions`; likewise skips files left behind by dead processes
pub fn list_running_sessions_in(sessions_dir: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<RunningMeta> {
    let mut result = vec![];
    if let Ok(entries) = std::fs::read_dir(sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "json") { continue; }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    let session_id = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let waiting_for = v.get("waitingFor").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                    // name must be included: a rename writes exactly this file, and writing it only triggers `running-changed`.
                    // Without this field that path **structurally** cannot carry the title, and a rename would only take effect after a manual refresh (#4 / #8).
                    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if pid > 0 && !alive(pid) {
                        continue;
                    }
                    if !session_id.is_empty() {
                        result.push(RunningMeta { session_id, status, waiting_for, pid, name });
                    }
                }
            }
        }
    }
    result
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct RunningMeta {
    pub session_id: String,
    pub status: String,
    pub waiting_for: String,
    pub pid: u32,
    /// The session name claude wrote itself (`name` in `~/.claude/sessions/<pid>.json`). May be empty.
    /// Same source as `RunningInfo.name`; in `parse_session` it has the **highest priority** for display_name.
    pub name: String,
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct PtyBinding {
    pub pty_id: String,
    pub session_id: String,
    pub short_id: String,
    /// The session name claude wrote itself (`name` in `~/.claude/sessions/<pid>.json`). May be empty.
    pub name: String,
}

/// Match "tabs not yet bound to a session" with "running claude processes": pty_id -> session_id.
///
/// Why not reuse `list_sessions`: the only fact binding needs is `(pty_id, session_id)`, which claude
/// writes into `~/.claude/sessions/<pid>.json` the moment it starts. `list_sessions` scans
/// `~/.claude/projects/**/*.jsonl` -- **and that file does not exist until the user sends the first message**.
/// So a tab where "a new shell was opened and claude was typed by hand" is not in the sessions array at all before a message is sent,
/// `s.pty_id === t.id` can never match, and the title stays at "New session"; the manual refresh still goes through `list_sessions`,
/// so refreshing does not help either; while a resumed tab already has a jsonl and looks "fine".
/// The three symptoms share one root cause: the binding hung off a data source that appears later than itself.
///
/// It works only on the pty_ids passed in by the frontend: with no tab awaiting binding the frontend does not call this command, and not a single ps is run.
/// It returns early once the quota is filled; in the normal case (one new tab) it stops at the first hit.
pub fn resolve_pty_bindings(pty_ids: Vec<String>) -> Vec<PtyBinding> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    resolve_bindings_in(
        &home.join(".claude").join("sessions"),
        &pty_ids,
        // Fold "which pty does this pid belong to now" into one question: a process that is already gone counts as having no marker.
        // sessions/*.json is not guaranteed to be cleaned when claude exits, so kill(0) first rules out dead pids,
        // saving a ps.
        &|pid| {
            if !pid_alive(pid) {
                return None;
            }
            get_env_var_of_pid(pid, "MAKIT_PTY_ID")
        },
    )
}

/// The pure-logic part of `resolve_pty_bindings`: a directory + the wanted pty_ids + a "pid -> pty_id" lookup.
/// Extracted so it can be tested -- the real implementation needs a live, non-Apple-signed process carrying MAKIT_PTY_ID,
/// which a unit test cannot create.
pub fn resolve_bindings_in(
    dir: &Path,
    pty_ids: &[String],
    pty_of_pid: &dyn Fn(u32) -> Option<String>,
) -> Vec<PtyBinding> {
    if pty_ids.is_empty() {
        return vec![];
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut result: Vec<PtyBinding> = vec![];
    for entry in entries.flatten() {
        // Each tab can bind only one session; once the quota is full there is no need to keep scanning (the common case is a single new tab)
        if result.len() == pty_ids.len() {
            break;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if pid == 0 {
            continue;
        }
        let pty_id = match pty_of_pid(pid) {
            Some(p) if pty_ids.iter().any(|want| *want == p) => p,
            _ => continue,
        };
        result.push(PtyBinding {
            pty_id,
            short_id: session_id.split('-').next().unwrap_or("").to_string(),
            session_id,
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    result
}

#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // signal 0: only an existence / permission check, no signal is really sent. But it also succeeds for a **zombie** (dead, parent has not waited yet),
    // so zombies must be excluded as well, otherwise a killed session keeps showing as running until the parent reaps it
    let exists = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    exists && !is_zombie(pid)
}

/// Whether the process is a zombie. On macOS this uses `proc_pidinfo` (much cheaper than forking a `ps`; the patrol asks every 3 seconds).
/// Measured (macOS 26): for a zombie `kill(pid, 0)` succeeds and `ps` shows state Z, while `proc_pidinfo` returns 0 with errno = ESRCH.
/// Other failures (e.g. EPERM: another user's process) are treated as alive: better to show too much than to kill by mistake
#[cfg(target_os = "macos")]
fn is_zombie(pid: u32) -> bool {
    const SZOMB: u32 = 5; // <sys/proc.h>
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    if n == size {
        info.pbi_status == SZOMB
    } else {
        errno == Some(libc::ESRCH)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit(')').next().and_then(|rest| rest.trim_start().chars().next()))
        == Some('Z')
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod pty_binding_tests {
    use std::fs;

    /// Regression, the second half of #169: the title of a tab where "claude was typed by hand in a new shell" does not sync.
    ///
    /// The original binding hung off the `sessions` array (`list_sessions` -> scanning
    /// `~/.claude/projects/**/*.jsonl`), and that jsonl is **not created until the user sends the first message**.
    /// So a newly started claude is not in sessions at all before a message is sent,
    /// `s.pty_id === t.id` can never match, tab.sessionId stays null, and the title stays at "New session".
    /// The manual refresh still goes through `list_sessions`, hence "refreshing did not fix it either"; a resumed tab already has a
    /// jsonl, hence "resumed ones are fine" -- the three statements in the report are three sides of one root cause.
    ///
    /// The only fact really needed is `(pty_id, session_id)`, which claude writes into
    /// `~/.claude/sessions/<pid>.json` the moment it starts. This tests exactly that "reading only that directory is enough".
    ///
    /// Evidence measured before the fix: pid 86431 had MAKIT_PTY_ID=t_msngclks3eo5, session 32318d5f
    /// had been alive for nearly 2 hours, while the tab with id t_msngclks3eo5 in localStorage was still
    /// `kind:"new" / label:"New session" / sessionId:null` -- because 32318d5f.jsonl did not exist.
    fn write(dir: &std::path::Path, file: &str, json: &str) {
        fs::write(dir.join(file), json).unwrap();
    }

    #[test]
    fn binds_from_sessions_dir_without_any_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-bind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // the one we want to bind: pid 4321 -> t_wanted
        write(
            &dir,
            "4321.json",
            r#"{"pid":4321,"sessionId":"32318d5f-1111-2222-3333-444455556666","name":"ai-claw-studio-6d"}"#,
        );
        // another's pty must not get mixed up
        write(
            &dir,
            "4322.json",
            r#"{"pid":4322,"sessionId":"deadbeef-0000-0000-0000-000000000000","name":"别人"}"#,
        );
        // pid=0 (claude wrote no pid): must be skipped, and the environment variables must not be queried
        write(
            &dir,
            "4323.json",
            r#"{"pid":0,"sessionId":"aaaaaaaa-0000-0000-0000-000000000000"}"#,
        );
        // not a json suffix / bad json: neither may crash the whole scan
        write(&dir, "notes.txt", "t_wanted");
        write(&dir, "broken.json", "{ this is not json");

        let asked = std::cell::RefCell::new(Vec::<u32>::new());
        let pty_of = |pid: u32| -> Option<String> {
            asked.borrow_mut().push(pid);
            match pid {
                4321 => Some("t_wanted".into()),
                4322 => Some("t_someone_else".into()),
                _ => None,
            }
        };

        let out = super::resolve_bindings_in(&dir, &["t_wanted".to_string()], &pty_of);

        assert_eq!(out.len(), 1, "只该绑上 t_wanted 那一个");
        assert_eq!(out[0].pty_id, "t_wanted");
        assert_eq!(out[0].session_id, "32318d5f-1111-2222-3333-444455556666");
        assert_eq!(out[0].short_id, "32318d5f", "short_id 取第一段 uuid");
        assert_eq!(
            out[0].name, "ai-claw-studio-6d",
            "要把 claude 写的会话名带回去，否则 tab 只能显示 [shortId]"
        );
        assert!(
            !asked.borrow().contains(&0),
            "pid=0 不该触发环境变量查询（每次查询是一个 ps 进程）"
        );

        // pty_ids empty: not a single directory read may happen -- the zero-cost path when the frontend has no tab awaiting binding
        let never = |_pid: u32| -> Option<String> {
            panic!("pty_ids 为空时不该查任何 pid");
        };
        assert!(super::resolve_bindings_in(&dir, &[], &never).is_empty());

        // no match means empty, not "pick one at random and return it"
        let out = super::resolve_bindings_in(&dir, &["t_nobody".to_string()], &pty_of);
        assert!(out.is_empty(), "没有命中的 pty_id 时必须返回空");

        let _ = fs::remove_dir_all(&dir);
    }
}

// --------------- codex tab binding (#231) ---------------
//
// While running, claude writes `~/.claude/sessions/<pid>.json` (pid + session id), and tab binding relies on it; codex does not write this file,
// so typing `codex` by hand in a shell / a new tab can never bind a session (the status dot, reorderable view, and killing the process on tab close all fail).
// The two facts available on the codex side: the process environment has `MAKIT_PTY_ID` (= the tab id); the process has its own rollout file open,
// and `session_meta.payload.id` on the first line of that file is the session id.

/// In the output of `lsof -Fn`, the session files codex has open (under `sessions_dir` and ending in .jsonl)
pub fn rollout_paths_from_lsof(output: &str, sessions_dir: &Path) -> Vec<std::path::PathBuf> {
    output
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(std::path::PathBuf::from)
        .filter(|p| p.starts_with(sessions_dir) && p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

/// The session id in the first-line `session_meta` of the rollout file
pub fn codex_session_id_of(path: &Path) -> Option<String> {
    use std::io::BufRead;
    let f = fs::File::open(path).ok()?;
    let first = std::io::BufReader::new(f).lines().map_while(Result::ok).find(|l| !l.trim().is_empty())?;
    let v: serde_json::Value = serde_json::from_str(&first).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    v.pointer("/payload/id").and_then(|x| x.as_str()).map(String::from)
}

/// Running codex processes whose `MAKIT_PTY_ID` is in `pty_ids` -> (tab id, session id).
/// Called only when a tab awaits binding (one pgrep per pass + one ps / lsof per matching process)
pub fn codex_pty_bindings(pty_ids: &[String]) -> Vec<PtyBinding> {
    let Some(sessions_dir) = dirs::home_dir().map(|h| h.join(".codex").join("sessions")) else { return vec![] };
    let Ok(out) = std::process::Command::new("pgrep").args(["-x", "codex"]).output() else { return vec![] };
    let mut result = vec![];
    for pid in String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
        let Some(pty) = get_env_var_of_pid(pid, "MAKIT_PTY_ID").filter(|p| pty_ids.contains(p)) else { continue };
        let Ok(lsof) = std::process::Command::new("lsof").args(["-p", &pid.to_string(), "-Fn"]).output() else { continue };
        let paths = rollout_paths_from_lsof(&String::from_utf8_lossy(&lsof.stdout), &sessions_dir);
        if let Some(sid) = paths.iter().find_map(|p| codex_session_id_of(p)) {
            let short_id = sid.split('-').next().unwrap_or("").to_string();
            result.push(PtyBinding { pty_id: pty, session_id: sid, short_id, name: String::new() });
        }
    }
    result
}

#[cfg(test)]
mod dead_pid_tests {
    use super::*;

    /// Why test this: what goes wrong on screen is "after a process is killed the sidebar keeps showing running" and "clicking it says already running and cannot be opened twice, so it cannot be opened"
    fn dir_with(name: &str, entries: &[(&str, &str, u32)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("makit-running-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for (i, (sid, status, pid)) in entries.iter().enumerate() {
            let body = format!(r#"{{"sessionId":"{sid}","status":"{status}","pid":{pid},"name":"n{i}"}}"#);
            fs::write(d.join(format!("{pid}-{i}.json")), body).unwrap();
        }
        d
    }

    #[test]
    fn a_session_whose_process_is_gone_is_not_running() {
        let d = dir_with("incr", &[("alive-s", "busy", 111), ("dead-s", "busy", 222)]);
        let got = list_running_sessions_in(&d, &|pid| pid == 111);
        let ids: Vec<_> = got.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, ["alive-s"], "kill 掉的进程留下的文件不算在跑");
    }

    #[test]
    fn full_scan_path_skips_dead_processes_too() {
        let d = dir_with("full", &[("alive-s", "idle", 111), ("dead-s", "waiting", 222)]);
        let got = load_running_info_in(&d, &|pid| pid == 111);
        assert!(got.contains_key("alive-s"));
        assert!(!got.contains_key("dead-s"), "全量扫描和增量路径判断要一致");
    }

    #[test]
    fn an_entry_without_a_pid_is_kept_because_we_cannot_tell() {
        let d = dir_with("nopid", &[("s", "idle", 0)]);
        assert_eq!(list_running_sessions_in(&d, &|_| false).len(), 1, "没有 pid 的旧格式判断不了，保留");
    }

    /// A killed process is a zombie until the parent waits: `kill(pid, 0)` still succeeds for a zombie, misreporting "already dead" as "still alive".
    /// The end-to-end measurement tripped on exactly this: after the kill the sidebar still showed running 8 seconds later
    #[test]
    fn a_zombie_is_not_alive() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        // no wait: after it exits it becomes a zombie (its parent is this test process, which did not reap it)
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!pid_alive(pid), "僵尸进程不算活着");
        child.wait().unwrap();
    }

    #[test]
    fn the_real_pid_check_knows_this_process_is_alive_and_a_reaped_child_is_not() {
        assert!(pid_alive(std::process::id()));
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(pid), "已经退出并被回收的进程");
    }
}

#[cfg(test)]
mod codex_binding_tests {
    use super::*;

    /// Why test this: what goes wrong on screen is "running codex in a shell, the tab keeps showing shell, no status dot, and the reorderable view does not appear"
    #[test]
    fn lsof_output_picks_only_rollout_files_under_the_sessions_dir() {
        let out = "p15369\nfcwd\nn/Users/me/proj\nf3\nn/Users/me/.codex/sessions/2026/10/01/rollout-2026-10-01T13-14-24-01a0f5e2.jsonl\nf4\nn/Users/me/.codex/log/codex-tui.log\nf5\nn/Users/me/other.jsonl\n";
        let got = rollout_paths_from_lsof(out, Path::new("/Users/me/.codex/sessions"));
        assert_eq!(got, vec![std::path::PathBuf::from("/Users/me/.codex/sessions/2026/10/01/rollout-2026-10-01T13-14-24-01a0f5e2.jsonl")]);
        assert!(rollout_paths_from_lsof("", Path::new("/x")).is_empty());
    }

    #[test]
    fn session_id_comes_from_the_session_meta_line() {
        let d = std::env::temp_dir().join(format!("makit-codex-bind-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let f = d.join("rollout.jsonl");
        fs::write(&f, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"01a0f5e2-9cd9\",\"cwd\":\"/w\"}}\n{\"type\":\"event_msg\"}\n").unwrap();
        assert_eq!(codex_session_id_of(&f).as_deref(), Some("01a0f5e2-9cd9"));
        fs::write(&f, "{\"type\":\"event_msg\"}\n").unwrap();
        assert_eq!(codex_session_id_of(&f), None, "第一行不是 session_meta：不认");
        assert_eq!(codex_session_id_of(&d.join("nope.jsonl")), None);
    }
}
