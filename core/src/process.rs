//! Process table, process environment variables, descendant collection, and killing processes when a tab closes (process tree + environment variable marker).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub command: String,
}

#[cfg(unix)]
pub fn get_env_var_of_pid(pid: u32, var_name: &str) -> Option<String> {
    // The flags can only be `-Eww`: in macOS's ps, `-e` is a synonym of `-A` (show all processes),
    // and what shows environment variables is the uppercase `-E`; `-x` (which pulls in processes with no controlling terminal) must not be added either --
    // BSD ps selection conditions are OR'ed, so adding one drowns out `-p` and returns the whole process table.
    // The old spelling `-xeww`, with both stacked, gave "756 lines, zero environment variables" and always returned None.
    // `-ww` stays: without it output is truncated to the terminal width, and environment variables sit at the end, so they get cut first.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-Eww", "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let marker = format!("{}=", var_name);
    for part in text.split_whitespace() {
        if let Some(val) = part.strip_prefix(&marker) {return Some(val.to_string());
        }
    }
    // also handle the space-separated case
    if let Some(pos) = text.find(&marker) {
        let after = &text[pos + marker.len()..];
        let val = after.split_whitespace().next().unwrap_or("");
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

#[cfg(not(unix))]
pub fn get_env_var_of_pid(_pid: u32, _var_name: &str) -> Option<String> {
    None
}

/// pid -> (ppid, process name). On macOS this goes through libproc (#216): ps has to read the full arguments of every process, 0.3s;
/// here only pid / ppid / process name are taken, a few milliseconds. The full command line is read by `full_command` only for the processes to be output.
#[cfg(target_os = "macos")]
pub fn collect_process_table() -> HashMap<u32, (u32, String)> {
    use std::ffi::{c_void, CStr};
    let mut table = HashMap::new();
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return table;
    }
    // New processes may appear between the two calls, so leave some slack
    let mut pids = vec![0 as libc::pid_t; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes) };
    if n <= 0 {
        return table;
    }
    for &pid in &pids[..(n as usize).min(pids.len())] {
        if pid <= 0 {
            continue;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size)
        };
        if got != size {
            continue; // the process just exited, or no permission
        }
        let name = unsafe { CStr::from_ptr(info.pbi_comm.as_ptr()) }.to_string_lossy().into_owned();
        table.insert(pid as u32, (info.pbi_ppid, name));
    }
    table
}

/// The full command line (like `ps -o command=`: arguments joined with spaces). If it cannot be read (the process has exited,
/// another user's process), fall back to the process name; ps likewise shows only the process name in that case.
#[cfg(target_os = "macos")]
pub fn full_command(pid: u32, fallback: &str) -> String {
    process_args(pid).unwrap_or_else(|| fallback.to_string())
}

/// Layout of KERN_PROCARGS2: argc(i32) | executable path \0 | several \0 padding | argv[0..argc] each ending in \0 | environment variables...
#[cfg(target_os = "macos")]
pub fn process_args(pid: u32) -> Option<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let ok = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, &mut argmax as *mut _ as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0)
    };
    if ok != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let ok = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0)
    };
    if ok != 0 || size < 4 {
        return None;
    }
    let buf = &buf[..size];
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?);
    let mut rest = &buf[4..];
    // skip the executable path and the \0 padding after it
    let path_end = rest.iter().position(|&b| b == 0)?;
    rest = &rest[path_end..];
    let first_arg = rest.iter().position(|&b| b != 0)?;
    rest = &rest[first_arg..];
    let args: Vec<String> = rest
        .split(|&b| b == 0)
        .take(argc.max(0) as usize)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    if args.is_empty() {
        return None;
    }
    Some(args.join(" "))
}

#[cfg(not(target_os = "macos"))]
pub fn full_command(_pid: u32, cmd: &str) -> String {
    cmd.to_string() // the ps-based process table already holds the full command line
}

#[cfg(not(target_os = "macos"))]
pub fn collect_process_table() -> HashMap<u32, (u32, String)> {
    let mut table = HashMap::new();
    let out = match Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return table,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim_start();
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let cmd: String = parts.collect::<Vec<_>>().join(" ");
        table.insert(pid, (ppid, cmd));
    }
    table
}

// Scan orphan processes with PPID=1 and check whether the env contains MAKIT_SESSION_ID=<session_id>
// Returns a list of (session_id, ProcessInfo)
pub fn collect_orphan_by_env(table: &HashMap<u32, (u32, String)>) -> Vec<(String, ProcessInfo)> {
    let orphans: Vec<u32> = table
        .iter()
        .filter(|(_, (ppid, cmd))| {
            *ppid == 1 && {
                let exe = cmd.split_whitespace().next().unwrap_or("");
                let base = exe.rsplit('/').next().unwrap_or(exe);
                // Scan only service-type processes (avoid running ps eww for all 393 orphans)
                ["java", "python", "python3", "node", "go", "gradle", "mvn", "ruby", "cargo"]
                    .iter()
                    .any(|k| base == *k)
            }
        })
        .map(|(&pid, _)| pid)
        .collect();
    if orphans.is_empty() {
        return Vec::new();
    }
    // batch ps eww to get the environment variables
    let pid_args: Vec<String> = orphans.iter().map(|p| p.to_string()).collect();
    let out = match Command::new("ps")
        .arg("eww")
        .arg("-p")
        .arg(pid_args.join(","))
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    for line in text.lines().skip(1) {
        // extract MAKIT_SESSION_ID=xxx from the env part
        if let Some(pos) = line.find("MAKIT_SESSION_ID=") {
            let after = &line[pos + 15..];
            let session_id = after.split(|c: char| c.is_whitespace() || c == '\0')
                .next()
                .unwrap_or("")
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            // extract the PID from the start of the line
            let pid: u32 = match line.trim_start().split_whitespace().next().and_then(|s| s.parse().ok()) {
                Some(v) => v,
                None => continue,
            };
            if let Some((ppid, cmd)) = table.get(&pid) {
                result.push((session_id, ProcessInfo {
                    pid,
                    ppid: *ppid,
                    command: full_command(pid, cmd),
                }));
            }
        }
    }
    result
}

pub fn descendants_of(root: u32, table: &HashMap<u32, (u32, String)>) -> Vec<ProcessInfo> {
    if root == 0 {
        return Vec::new();
    }
    let mut by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &(ppid, _)) in table.iter() {
        by_parent.entry(ppid).or_default().push(pid);
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(p) = queue.pop() {
        if let Some(children) = by_parent.get(&p) {
            for &c in children {
                if let Some((ppid, cmd)) = table.get(&c) {
                    out.push(ProcessInfo {
                        pid: c,
                        ppid: *ppid,
                        command: full_command(c, cmd),
                    });
                    queue.push(c);
                }
            }
        }
    }
    out
}

/// #216: the process table used to rely on `ps -eo pid=,ppid=,command=`; macOS's ps has to read the full arguments of all ~700 processes,
/// 0.3s (up to 0.55s when competing for CPU at startup), the biggest piece of the startup path once the cache is persisted.
/// Changed to libproc taking only pid / ppid / process name, with the full command line read only for the few processes finally output.
/// What goes wrong on screen if this breaks: the "child processes" list of a running session has missing entries, command lines show only the process name, or orphan service processes cannot be found again.
#[cfg(all(test, target_os = "macos"))]
mod process_table_tests {
    use super::*;

    #[test]
    fn native_table_agrees_with_ps_on_parents() {
        let table = collect_process_table();
        let out = Command::new("ps").args(["-axo", "pid=,ppid="]).output().unwrap();
        let (mut both, mut same) = (0, 0);
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            let (Some(pid), Some(ppid)) = (it.next().and_then(|s| s.parse::<u32>().ok()), it.next().and_then(|s| s.parse::<u32>().ok())) else { continue };
            if let Some((p, _)) = table.get(&pid) {
                both += 1;
                if *p == ppid { same += 1; }
            }
        }
        // processes appear and die between the two samples, so judge by proportion; pid / ppid themselves do not change
        assert!(both > 50, "原生进程表几乎是空的：只对上 {both} 个");
        assert_eq!(same, both, "同一个 pid 的父进程应该完全一致");
    }

    #[test]
    fn descendants_carry_full_command_line() {
        let mut child = Command::new("sleep").arg("7.216").spawn().unwrap();
        let me = std::process::id();
        let table = collect_process_table();
        let found = descendants_of(me, &table);
        let _ = child.kill();
        let _ = child.wait();
        let hit = found.iter().find(|p| p.pid == child.id()).expect("子进程要在后代里");
        assert_eq!(hit.ppid, me);
        assert_eq!(hit.command, "sleep 7.216", "输出的是完整命令行，不只是进程名");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod env_of_pid_tests {
    /// Regression: `get_env_var_of_pid` must really read the environment variables of **the specified pid**.
    ///
    /// The original implementation was `ps -p PID -xeww -o command=`, and both flags were wrong:
    /// in macOS's `ps`, `-e` is a synonym of `-A` ("show all processes"), and what shows environment variables is
    /// **uppercase `-E`**; and `-x` pulls in processes with no controlling terminal (all daemons) --
    /// BSD ps selection conditions are OR rather than AND, so `-p` was completely drowned out. Measured: this command returned
    /// the whole 756-line process table with not a single environment variable, so the function always returned None.
    ///
    /// The consequence is not "occasionally unavailable" but "never available": `pty_id` is always an empty string -> in App.tsx
    /// `sessions.filter(s => s.running && s.pty_id)` filters everything out because an empty string is falsy ->
    /// the tab where claude was typed by hand in a new shell can never bind a session, and the title stays at
    /// "New session" (a manual refresh cannot save it either; the full load goes through the same command).
    ///
    /// It uses its own pid as the subject: the test binary is a build artifact of this project, not an Apple platform
    /// binary, so its environment variables can be read (SIP only blocks signed platform binaries like /bin/*).
    /// PATH is compared for **equality** rather than non-emptiness -- with only a non-empty check, a mix-up such as "picking up
    /// someone else's PATH from the whole process table" would also pass.
    /// The sampled variable is deliberately one whose value has no whitespace: in the ps output environment variables are separated by spaces,
    /// and a value that itself contains spaces (e.g. a PATH with `/Library/Application Support/...`)
    /// cannot be restored unambiguously -- an inherent limit of the ps output format, not a fixable bug.
    /// The only caller reads just MAKIT_PTY_ID, whose value looks like `t_ms5nflsm3a0g` and is unaffected.
    #[test]
    fn reads_env_of_the_requested_pid() {
        let me = std::process::id();
        let (key, expected) = std::env::vars()
            .find(|(k, v)| {
                !v.is_empty() && !v.contains(char::is_whitespace) && !k.contains('=')
            })
            .expect("测试环境里至少得有一个值不含空白的环境变量");
        assert_eq!(
            super::get_env_var_of_pid(me, &key),
            Some(expected),
            "没读到本进程的 {key}（要么 ps 没显示环境变量，要么串到了别的进程）"
        );
        // a variable that was never set must be None, not one picked up from elsewhere
        assert_eq!(
            super::get_env_var_of_pid(me, "MAKIT_DEFINITELY_UNSET_VAR_9f3a"),
            None
        );
    }
}

/// These are funneled out by pid so they can be tested (#142) -- `kill_pty` holds a portable-pty `Child`,
/// which tests cannot construct. Closing a tab goes through exactly these two statements.
#[cfg(unix)]
pub fn kill_pty_by_pid(pid: u32, pty_id: &str) {
    kill_tree(pid);
    kill_by_env_marker(&[pty_id]);
}

/// Kill the process tree: killpg hits the main process group + `pgrep -P` recursively kills descendants one by one.
/// It covers the four shapes plain / nohup / setsid_child / nested; the two that reparent to 1 are out of reach and
/// are left to `kill_by_env_marker` (the matrix test of #142 pins down this dividing line).
#[cfg(unix)]
pub fn kill_tree(pid: u32) {
    // 1) first recursively find all descendant processes (collected before sending signals, so they are not lost after processes exit)
    let descendants = find_descendants(pid);

    unsafe {
        // 2) killpg kills the main process group.
        //    Note that `killpg` takes a **pgid**, not a pid; passing the pid directly works here because
        //    portable-pty setsid's the child shell, so pid == pgid. This precondition is stated explicitly in the tests.
        libc::killpg(pid as libc::pid_t, libc::SIGHUP);
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);

        // 3) kill descendants one by one (covers those that escaped via setsid)
        for dpid in &descendants {
            libc::kill(*dpid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
pub fn find_descendants(root_pid: u32) -> Vec<u32> {
    use std::process::Command;
    // pgrep -P <pid> finds direct children, recursively
    let mut all = Vec::new();
    let mut stack = vec![root_pid];
    while let Some(pid) = stack.pop() {
        if let Ok(output) = Command::new("pgrep").arg("-P").arg(pid.to_string()).output() {
            if output.status.success() {
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Ok(child_pid) = line.trim().parse::<u32>() {
                        all.push(child_pid);
                        stack.push(child_pid);
                    }
                }
            }
        }
    }
    all
}

/// Kill escaped processes by the `MAKIT_PTY_ID` environment variable -- those that after setsid / double-fork have PPID=1
/// and a process group detached from ours (#3). It is the only mark by which they can still be recognized: environment variables are
/// copied at fork and carried everywhere, while the PPID chain and the process group are already broken.
///
/// **It really kills by default**. When gathering evidence, set `MAKIT_KILL_ESCAPED=0` to fall back to dry-run (print only, no signals),
/// the same approach as `MAKIT_TIMING`, with no recompilation.
///
/// Why it is safe to kill by default: what is matched is the whole token `MAKIT_PTY_ID=<this tab's id>`; the id is generated by this process
/// and can only appear on processes this tab launched; other tabs, other apps and the user's own processes
/// never carry it. The only collateral case is a process the user deliberately nohup/setsid'ed out of this tab, expecting it to outlive closing the
/// tab; this trade-off is recorded in #3 (a tab's semantics is one claude session, not a general-purpose terminal).
///
/// Where to read the log: under dev it goes straight to the terminal running `pnpm tauri dev`; a release .app has to be
/// launched from a terminal (stderr is not visible when started with `open`).
#[cfg(unix)]
pub fn kill_by_env_marker(pty_ids: &[&str]) {
    use std::process::Command;
    if pty_ids.is_empty() { return; }
    let my_pid = std::process::id();
    // Only an explicit 0 falls back to dry-run; unset = really kill
    let dry_run = std::env::var("MAKIT_KILL_ESCAPED").map(|v| v == "0").unwrap_or(false);
    // The flags must be `-xEww`: what shows environment variables is **uppercase `-E`**, and lowercase `-e` in macOS's ps
    // is a synonym of `-A` ("show all processes"). The old spelling `-xeww` thus got a whole process table containing no
    // environment variables, and the `MAKIT_PTY_ID=` match below was always false -- this fallback had been idling all along.
    // `-x` is kept here **on purpose**: what we want to find is exactly the escaped processes detached from the controlling terminal.
    let output = match Command::new("ps").args(["-xEww", "-o", "pid,command"]).output() {
        Ok(o) if o.status.success() => o,
        _ => {
            log::warn!(target: "escaped-pty", "ps 执行失败，兜底清理跳过");
            return;
        }
    };
    let mut hits: Vec<(u32, &str)> = Vec::new();
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        // parse the PID (the digits at the start of the line)
        let pid: u32 = match trimmed.split_whitespace().next().unwrap_or("").parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pid == my_pid || pid <= 1 { continue; }
        // whole-token equality rather than contains: `contains` would judge `MAKIT_PTY_ID=t_abc` as
        // a hit on `MAKIT_PTY_ID=t_abcdef`. All current ids have equal length so they cannot collide, but this is
        // a really-kills path, so no premise of the "works only because of the format" kind is left in.
        for tok in trimmed.split_whitespace() {
            if let Some(val) = tok.strip_prefix("MAKIT_PTY_ID=") {
                if let Some(hit) = pty_ids.iter().find(|p| **p == val) {
                    hits.push((pid, hit));
                }
                break;
            }
        }
    }
    if hits.is_empty() {
        // A miss prints one line only under dev -- closing a tab is a path hit dozens of times a day, and release should not flood the log;
        // but "ran but found nothing" must be distinguishable from "did not run at all"; this very bug was masked by the latter for three months.
        log::debug!(target: "escaped-pty", "扫了 {} 个 pty id，没有逃逸进程", pty_ids.len());
        return;
    }
    for (pid, pty_id) in &hits {
        if dry_run {
            log::info!(target: "escaped-pty", "dry-run 命中 pid={pid} pty_id={pty_id}（MAKIT_KILL_ESCAPED=0，未发信号）");
        } else {
            log::info!(target: "escaped-pty", "SIGKILL pid={pid} pty_id={pty_id}");
            unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL); }
        }
    }
}

// ---- Windows (platform matrix P2): a first, coarse version. `taskkill /T` walks the process tree; escaped
// processes cannot be found by an environment marker yet (needs a Job Object or reading the process environment block).
#[cfg(not(unix))]
pub fn kill_pty_by_pid(pid: u32, _pty_id: &str) {
    kill_tree(pid);
}

#[cfg(not(unix))]
pub fn kill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
}

#[cfg(not(unix))]
pub fn find_descendants(_root_pid: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(unix))]
pub fn kill_by_env_marker(_pty_ids: &[&str]) {}

/// #142 verification of killing child processes in every scenario.
///
/// The purpose is not to "test a function's return value" but to **measure which child-process shapes `kill_tree` actually covers**:
/// build one real process for each of six shapes, send real signals, and see who is still alive. Whether closing a tab kills everything cleanly was
/// previously a guess (#3, #142); here it becomes a reproducible measurement.
///
/// Safety boundaries (real signals are being sent here, and one wrong pgid could take down every claude the user is running):
///   1. The root of the synthetic tree is started with `setsid()` -- it must have **its own process group**, otherwise `killpg` hits
///      the group cargo test itself is in, which is suicide.
///   2. Before sending signals, explicitly assert `pgid(root) == root`, `pgid(root) != pgid(self)` and `root > 1`;
///      the assertions come before `kill_tree`; if unmet, clean up first and then panic, never proceeding with a wrong pgid.
///   3. All synthetic processes carry the `MAKIT_KILL_MATRIX=<nonce>` environment variable, and cleanup sweeps by nonce as a safety net,
///      so a panic midway does not leave a pile of sleeps on the machine.
/// SIGKILL one by one (the frontend's "kill child processes that escaped the process group" button). Does nothing on non-unix.
pub fn kill_pids(pids: &[u32]) {
    #[cfg(unix)]
    {
        for pid in pids {
            unsafe {
                libc::kill(*pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = pids;
}

#[cfg(all(test, unix))]
mod kill_matrix_tests {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    pub(super) const MARKER: &str = "MAKIT_KILL_MATRIX";

    /// Six shapes: the name + where each stands relative to the "killpg + `pgrep -P` recursion" mechanism.
    const SHAPES: [(&str, &str); 6] = [
        ("plain", "普通子进程：同进程组，killpg 直接覆盖"),
        ("nohup", "nohup：忽略 SIGHUP，同进程组，靠后面那发 SIGKILL"),
        ("setsid_child", "setsid 子进程：新进程组，但 PPID 仍是 root → pgrep -P 能找到"),
        ("nested", "嵌套孙进程：root → sh → sleep，全在同进程组"),
        ("orphan_setsid", "逃逸孙进程：中间 sh 立刻退出 → 被 reparent 到 1，PPID 链断 + 新进程组"),
        ("double_fork", "经典 double-fork daemon：PPID=1 + 新 session"),
    ];

    /// The set of live pids. Uses the `ps` state rather than `kill(pid, 0)`: a killed child first becomes a
    /// zombie, and `kill(pid, 0)` still returns 0 for a zombie, misreporting "already killed" as "still alive".
    pub(super) fn live_pids(pids: &[u32]) -> HashSet<u32> {
        let mut live = HashSet::new();
        if pids.is_empty() {
            return live;
        }
        let list = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
        let out = match Command::new("ps").args(["-o", "pid=,state=", "-p", &list]).output() {
            Ok(o) => o,
            Err(e) => panic!("ps 执行失败，无法判定存活: {e}"),
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            let pid: u32 = match it.next().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            let state = it.next().unwrap_or("");
            if !state.starts_with('Z') {
                live.insert(pid);
            }
        }
        live
    }

    /// Safety-net sweep: find all still-alive synthetic processes by the nonce environment variable and SIGKILL them. Returns the pids cleared.
    /// This path cannot depend on the pids collected earlier -- an escaped process's pid may be exactly the one not recorded.
    pub(super) fn sweep_by_nonce(nonce: &str) -> Vec<u32> {
        let my_pid = std::process::id();
        let out = match Command::new("ps").args(["-axEww", "-o", "pid=,command="]).output() {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let needle = format!("{MARKER}={nonce}");
        let mut killed = Vec::new();
        for line in text.lines() {
            if !line.contains(&needle) {
                continue;
            }
            let pid: u32 = match line.split_whitespace().next().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            if pid <= 1 || pid == my_pid {
                continue;
            }
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            killed.push(pid);
        }
        killed
    }

    fn have_python3() -> bool {
        Command::new("python3")
            .args(["-c", "pass"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn write_scripts(dir: &PathBuf) {
        // The leaf processes deliberately use python3, not `sleep`.
        //
        // Because /bin/sleep and /bin/sh are both SIP-protected platform binaries, macOS does not allow reading their
        // environment variables: `ps -axEww` prints only the command line for them, and the environment variables part is entirely empty. Measured
        // (/bin/sleep no, /bin/sh no, /usr/bin/python3 yes, node yes). And what is verified below is exactly
        // "whether the environment variable safety net can see escaped processes"; using sleep as the leaf would measure a false negative
        // unrelated to the real scenario -- in the real scenario what runs is node (claude), whose environment variables are visible.
        //
        // A side note: copying /bin/sleep out and running the copy does not work either; a platform binary's signature
        // is invalidated once it is moved, and exec is rejected by the kernel outright.
        fs::write(
            dir.join("leaf.py"),
            "import os, sys, time\n\
             name = sys.argv[1]\n\
             mode = sys.argv[2] if len(sys.argv) > 2 else \"plain\"\n\
             if mode == \"dfork\":\n\
             \x20   # 经典 daemonize：fork → 父退出（子 reparent 到 1）→ setsid → 再 fork\n\
             \x20   if os.fork() > 0: os._exit(0)\n\
             \x20   os.setsid()\n\
             \x20   if os.fork() > 0: os._exit(0)\n\
             elif mode == \"setsid\":\n\
             \x20   os.setsid()\n\
             with open(os.path.join(os.environ[\"MK_D\"], name + \".pid\"), \"w\") as f:\n\
             \x20   f.write(str(os.getpid()))\n\
             time.sleep(300)\n",
        )
        .unwrap();

        // Root script: start one of each of the six shapes, then wait -- root must stay alive, otherwise the killpg target group is gone.
        fs::write(
            dir.join("root.sh"),
            "L=\"$MK_D/leaf.py\"\n\
             python3 \"$L\" plain &\n\
             nohup python3 \"$L\" nohup >/dev/null 2>&1 &\n\
             python3 \"$L\" setsid_child setsid &\n\
             sh -c 'python3 \"$0\" nested; true' \"$L\" &\n\
             sh -c 'python3 \"$0\" orphan_setsid setsid & exit 0' \"$L\" &\n\
             python3 \"$L\" double_fork dfork &\n\
             wait\n",
        )
        .unwrap();
    }

    /// Wait until all six pid files are fully written. If signals were sent without waiting, 5 and 6 might not have finished setsid yet,
    /// and "did not get to escape" would be read as "failed to escape" -- a false green light.
    fn wait_for_pids(dir: &PathBuf) -> Vec<(&'static str, u32)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut got = Vec::new();
            for (name, _) in SHAPES {
                if let Ok(s) = fs::read_to_string(dir.join(format!("{name}.pid"))) {
                    if let Ok(pid) = s.trim().parse::<u32>() {
                        got.push((name, pid));
                    }
                }
            }
            if got.len() == SHAPES.len() {
                return got;
            }
            if Instant::now() >= deadline {
                let missing: Vec<&str> = SHAPES
                    .iter()
                    .map(|(n, _)| *n)
                    .filter(|n| !got.iter().any(|(g, _)| g == n))
                    .collect();
                panic!("10s 内只起来了 {}/6 个合成进程，缺: {:?}", got.len(), missing);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A live six-shape synthetic tree. Each of the two tests builds one, distinguished by `tag` -- the nonce / temp directory /
    /// `MAKIT_PTY_ID` all carry it, otherwise when cargo runs the two tests concurrently, one's safety-net sweep would sweep away the other's
    /// processes, with the symptom of random green / random red.
    struct Matrix {
        root: std::process::Child,
        dir: PathBuf,
        nonce: String,
        pty_id: String,
        leaves: Vec<(&'static str, u32)>,
    }

    impl Matrix {
        fn pids(&self) -> Vec<u32> {
            self.leaves.iter().map(|(_, p)| *p).collect()
        }

        /// Which leaves are still alive (name + pid)
        fn survivors(&self) -> Vec<(&'static str, u32)> {
            let live = live_pids(&self.pids());
            self.leaves.iter().filter(|(_, p)| live.contains(p)).cloned().collect()
        }

        /// Cleanup: safety-net sweep by nonce + kill the root + delete the temp directory. Returns the pids the safety net cleared.
        /// **Must run before the assertions** -- even if an assertion panics, no pile of python may be left on the machine.
        fn finish(mut self) -> Vec<u32> {
            let swept = sweep_by_nonce(&self.nonce);
            let _ = self.root.kill();
            let _ = self.root.wait();
            let _ = fs::remove_dir_all(&self.dir);
            swept
        }
    }

    /// Build a synthetic tree and establish the safety boundaries (real signals are being sent here, and one wrong pgid could take down every claude the user is running):
    ///   1. The root is started with `setsid()` -- it must have **its own process group**, otherwise `killpg` hits the group cargo test itself is in;
    ///   2. Before returning, assert `pgid(root) == root` and `!= pgid(self)` and `root > 1`; if unmet, clean up first and then panic;
    ///   3. All six leaves must be alive first, otherwise the later "dead" means nothing.
    fn spawn_matrix(tag: &str) -> Matrix {
        let nonce = format!("n{}x{}", std::process::id(), tag);
        let pty_id = format!("t_killmatrix_{}_{}", std::process::id(), tag);
        let dir = std::env::temp_dir().join(format!("makit-kill-matrix-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_scripts(&dir);

        let mut cmd = Command::new("sh");
        cmd.arg(dir.join("root.sh"))
            .env("MK_D", &dir)
            .env(MARKER, &nonce)
            // processes started by a real pty all carry this (set at spawn time in pty.rs), and the escape safety net recognizes it
            .env("MAKIT_PTY_ID", &pty_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let root = cmd.spawn().expect("合成树的根起不来");
        let root_pid = root.id();
        let leaves = wait_for_pids(&dir);
        let m = Matrix { root, dir, nonce, pty_id, leaves };

        let alive = m.survivors();
        if alive.len() != SHAPES.len() {
            let dead_early: Vec<&str> = m
                .leaves
                .iter()
                .filter(|(n, _)| !alive.iter().any(|(a, _)| a == n))
                .map(|(n, _)| *n)
                .collect();
            m.finish();
            panic!("发信号之前就已经死了: {dead_early:?}");
        }

        let my_pgid = unsafe { libc::getpgid(0) };
        let root_pgid = unsafe { libc::getpgid(root_pid as libc::pid_t) };
        let guard_err = if root_pid <= 1 {
            Some(format!("root pid 不合法: {root_pid}"))
        } else if root_pgid != root_pid as libc::pid_t {
            Some(format!("root 没有自己的进程组: pid={root_pid} pgid={root_pgid}，killpg 会打到别人身上"))
        } else if root_pgid == my_pgid {
            Some(format!("root 的进程组和测试进程同组（pgid={my_pgid}），killpg 会杀掉 cargo test 自己"))
        } else {
            None
        };
        if let Some(msg) = guard_err {
            m.finish();
            panic!("安全边界不满足，已中止发信号: {msg}");
        }
        m
    }

    /// Which live synthetic processes can be seen via `ps` environment variables (the batch the safety net can recognize)
    fn visible_by_env(nonce: &str, among: &[(&'static str, u32)]) -> Vec<u32> {
        let out = Command::new("ps").args(["-axEww", "-o", "pid=,command="]).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let needle = format!("{MARKER}={nonce}");
        text.lines()
            .filter(|l| l.contains(&needle))
            .filter_map(|l| l.split_whitespace().next().and_then(|s| s.parse::<u32>().ok()))
            .filter(|p| among.iter().any(|(_, sp)| sp == p))
            .collect()
    }

    /// Mechanism measurement (#142): which shapes `kill_tree` alone (killpg + `pgrep -P` recursion) can cover.
    ///
    /// This is not "desired behaviour" but the **mechanism's upper bound**: the two that reparent to 1 inevitably escape. It exists to
    /// pin down "which four are guaranteed dead in black and white", and "the two that escape are at least still visible" --
    /// if visible, the safety net can save it (`kill_by_env_marker`); if not, #3 would have nothing to hold on to at all.
    #[test]
    fn kill_tree_covers_all_child_shapes() {
        if !have_python3() {
            eprintln!("[#142] 没有 python3，跳过（setsid / double-fork 形态造不出来）");
            return;
        }
        let m = spawn_matrix("mech");
        let nonce = m.nonce.clone();

        // ===== behaviour under test =====
        super::kill_tree(m.root.id());

        // signals are asynchronous; give the kernel a moment to reap the processes
        std::thread::sleep(Duration::from_millis(600));
        let survivors = m.survivors();
        let visible = visible_by_env(&nonce, &survivors);
        let swept = m.finish();

        let dead = |name: &str| !survivors.iter().any(|(n, _)| *n == name);
        for name in ["plain", "nohup", "setsid_child", "nested"] {
            assert!(
                dead(name),
                "kill_tree 没杀掉 `{name}`（{}），这是它本该保证覆盖的形态。\n\
                 存活: {survivors:?}，已兜底清掉 {swept:?}",
                SHAPES.iter().find(|(s, _)| *s == name).map(|(_, d)| *d).unwrap_or("")
            );
        }

        if !survivors.is_empty() {
            let escaped: Vec<&str> = survivors.iter().map(|(n, _)| *n).collect();
            eprintln!(
                "[#142] kill_tree 机制上限：{escaped:?} 逃掉了（reparent 到 1 → PPID 链断 + 新进程组）；\
                 环境变量兜底可见 {}/{} 个",
                visible.len(),
                survivors.len()
            );
            assert_eq!(
                visible.len(),
                survivors.len(),
                "逃逸进程 {escaped:?} 里有 {} 个连 `ps -axEww` 的环境变量都扫不到 —— \
                 那么 #3 没有任何可用的兜底路径。可见的: {visible:?}",
                survivors.len() - visible.len()
            );
        }
    }

    /// The path closing a tab really takes (#3): `kill_tree` + sweeping escaped processes by `MAKIT_PTY_ID`.
    /// The contract is one notch stronger than the one above -- **all six die**, none may remain.
    ///
    /// This is what the user can see: after closing a tab, that session must not keep burning memory and tokens in the background.
    #[test]
    fn close_path_kills_every_child_shape() {
        if !have_python3() {
            eprintln!("[#3] 没有 python3，跳过（setsid / double-fork 形态造不出来）");
            return;
        }
        let m = spawn_matrix("close");
        let pty_id = m.pty_id.clone();

        // ===== behaviour under test: what `pty_kill` does once it has the Child is exactly this one statement =====
        super::kill_pty_by_pid(m.root.id(), &pty_id);

        std::thread::sleep(Duration::from_millis(600));
        let survivors = m.survivors();
        let swept = m.finish();

        assert!(
            survivors.is_empty(),
            "关 tab 之后还活着: {survivors:?}（已兜底清掉 {swept:?}）。\n\
             逃逸形态靠 MAKIT_PTY_ID 兜底，活下来说明兜底没生效 —— \
             先确认 ps 用的是大写 -E（小写 -e 在 macOS 上是「全部进程」，拿不到环境变量）。"
        );
    }
}


/// Real pty end to end (#3): the difference from `kill_matrix_tests` is that there is **no simulation at all** --
/// it opens a real pty with the very same `portable_pty::native_pty_system()` as production code, starts a real shell,
/// and inside it uses node (claude is node) to start a detached grandchild process, after which the parent exits immediately,
/// so the grandchild reparents to 1 -- this is the real shape of the thing the user cannot close.
///
/// It verifies three things a synthetic tree alone cannot:
///   1. The precondition of `killpg(pid)` holds: the shell started by portable-pty really has `pid == pgid`;
///   2. `MAKIT_PTY_ID` really is inherited all the way down to the escaped grandchild (pty -> shell -> node -> detached node);
///   3. The node process's environment variables are really visible in `ps -xEww` -- SIP-protected platform binaries
///      (/bin/sh, /bin/sleep) are not visible, and the safety net cannot recognize them. claude is node, so this holds.
#[cfg(all(test, unix))]
mod real_pty_close_tests {
    use super::kill_matrix_tests::{live_pids, sweep_by_nonce, MARKER};
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::fs;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn have_node() -> bool {
        Command::new("node").arg("-v").output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// Read a process's (ppid, pgid); returns None if the process is gone
    fn ppid_pgid(pid: u32) -> Option<(u32, u32)> {
        let out = Command::new("ps").args(["-o", "ppid=,pgid=", "-p", &pid.to_string()]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut it = text.split_whitespace();
        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
    }

    #[test]
    fn closing_real_pty_kills_detached_node_grandchild() {
        if !have_node() {
            eprintln!("[#3] 没有 node，跳过（逃逸孙进程造不出来）");
            return;
        }

        let nonce = format!("n{}xrealpty", std::process::id());
        let pty_id = format!("t_realpty_{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("makit-real-pty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // parent node: start a detached (= setsid) grandchild process, then exit immediately -> the grandchild is reparented to 1
        fs::write(
            dir.join("escape.js"),
            "const { spawn } = require('child_process');\n\
             const d = process.env.MK_D;\n\
             const code = `require('fs').writeFileSync(process.env.MK_D + '/escaped.pid', String(process.pid)); setTimeout(() => {}, 300000)`;\n\
             const c = spawn(process.execPath, ['-e', code], { detached: true, stdio: 'ignore' });\n\
             c.unref();\n\
             process.exit(0);\n",
        )
        .unwrap();

        // ===== same setup as pty_spawn: openpty + CommandBuilder + MAKIT_PTY_ID =====
        let pair = native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .expect("openpty 失败");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("node \"$MK_D/escape.js\"; sleep 300");
        cmd.cwd(&dir);
        cmd.env("MK_D", dir.to_string_lossy().to_string());
        cmd.env(MARKER, &nonce);
        cmd.env("MAKIT_PTY_ID", &pty_id); // this is exactly the line production pty_spawn sets
        let mut child = pair.slave.spawn_command(cmd).expect("pty 里起 shell 失败");
        drop(pair.slave);
        let shell_pid = child.process_id().expect("拿不到 pty 子进程 pid");

        // wait for the grandchild to write its own pid
        let deadline = Instant::now() + Duration::from_secs(20);
        let escaped_pid = loop {
            if let Ok(t) = fs::read_to_string(dir.join("escaped.pid")) {
                if let Ok(p) = t.trim().parse::<u32>() {
                    break p;
                }
            }
            if Instant::now() >= deadline {
                sweep_by_nonce(&nonce);
                let _ = child.kill();
                let _ = child.wait();
                panic!("20s 内 detached 孙进程没起来");
            }
            std::thread::sleep(Duration::from_millis(100));
        };

        // precondition measurement: did it really escape, and does killpg's precondition hold
        let shell_pgid = unsafe { libc::getpgid(shell_pid as libc::pid_t) };
        let escaped = ppid_pgid(escaped_pid);
        let visible = {
            let out = Command::new("ps").args(["-xEww", "-o", "pid=,command="]).output().unwrap();
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| {
                    l.split_whitespace().next().and_then(|s| s.parse::<u32>().ok()) == Some(escaped_pid)
                        && l.split_whitespace().any(|t| t == format!("MAKIT_PTY_ID={pty_id}"))
                })
        };
        let pre_err = match escaped {
            None => Some("孙进程在发信号前就没了".to_string()),
            Some((ppid, pgid)) if shell_pgid != shell_pid as libc::pid_t => Some(format!(
                "portable-pty 起的 shell 没有自己的进程组: pid={shell_pid} pgid={shell_pgid}（killpg 会打到别人身上）；孙 ppid={ppid} pgid={pgid}"
            )),
            Some((ppid, pgid)) if ppid != 1 || pgid == shell_pgid as u32 => Some(format!(
                "孙进程没真的逃出去: ppid={ppid}（期望 1）pgid={pgid}（shell 组 {shell_pgid}）—— 那这条测试证明不了兜底有用"
            )),
            Some(_) if !visible => Some(format!(
                "逃逸的 node 孙进程 pid={escaped_pid} 在 `ps -xEww` 里看不到 MAKIT_PTY_ID=<id> —— 兜底认不出它"
            )),
            Some(_) => None,
        };
        if let Some(msg) = pre_err {
            sweep_by_nonce(&nonce);
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_dir_all(&dir);
            panic!("前置条件不成立，已中止: {msg}");
        }
        let (ppid, pgid) = escaped.unwrap();
        eprintln!(
            "[#3] 真 pty：shell pid={shell_pid} pgid={shell_pgid}；逃逸孙进程 pid={escaped_pid} ppid={ppid} pgid={pgid}，MAKIT_PTY_ID 可见"
        );

        // ===== behaviour under test: close this tab =====
        super::kill_pty_by_pid(shell_pid, &pty_id);
        let _ = child.kill();
        let _ = child.wait();

        std::thread::sleep(Duration::from_millis(600));
        let still_alive = live_pids(&[shell_pid, escaped_pid]);

        let swept = sweep_by_nonce(&nonce);
        let _ = fs::remove_dir_all(&dir);

        assert!(
            !still_alive.contains(&escaped_pid),
            "关掉 pty 之后逃逸的 node 孙进程 pid={escaped_pid} 还活着（已兜底清掉 {swept:?}）—— \
             这就是用户看到的 #3：tab 没了，claude 还在烧内存和 token"
        );
        assert!(!still_alive.contains(&shell_pid), "pty 里的 shell pid={shell_pid} 还活着");
    }
}
