//! 运行状态：`~/.claude/sessions/*.json`（claude 运行时写的 pid / 状态 / 名字），
//! 以及 pty_id → session 的绑定。

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

/// `load_running_info` 的可测版本：目录 + 「这个 pid 还活着吗」。
/// 进程被 kill / 崩溃 / 终端被关时 claude 不会删 `sessions/<pid>.json`，文件还在但进程没了 ——
/// 这种条目不能算「在跑」，否则侧栏一直显示运行中，还会拦着不让重新打开（「正在运行中，不能重复启动」）
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
            continue; // 死进程留下的文件
        }
        // 检查 claude 进程的环境变量获取 MAKIT_PTY_ID
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

// 轻量扫描：只读 running 状态（~/.claude/sessions/*.json），不扫 projects
pub fn list_running_sessions() -> Vec<RunningMeta> {
    match dirs::home_dir() {
        Some(h) => list_running_sessions_in(&h.join(".claude").join("sessions"), &pid_alive),
        None => vec![],
    }
}

/// `list_running_sessions` 的可测版本，同样跳过死进程留下的文件
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
                    // name 必须带上：改名写的就是这个文件，而写它只会触发 `running-changed`。
                    // 少了这个字段，那条路就**结构上**搬不了标题，改名要 ⌘R 才生效（#4 / #8）。
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
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    /// 和 `RunningInfo.name` 同源；`parse_session` 里它是 display_name 的**最高优先级**。
    pub name: String,
}

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct PtyBinding {
    pub pty_id: String,
    pub session_id: String,
    pub short_id: String,
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    pub name: String,
}

/// 把「还没绑上 session 的 tab」和「正在跑的 claude」对上：pty_id → session_id。
///
/// 为什么不复用 `list_sessions`：绑定需要的事实只有 `(pty_id, session_id)`，它在 claude
/// 启动的那一瞬间就写进了 `~/.claude/sessions/<pid>.json`。而 `list_sessions` 扫的是
/// `~/.claude/projects/**/*.jsonl` —— **那个文件要等用户发出第一条消息之后才存在**。
/// 于是「新建 shell、手打 claude」的 tab 在发消息之前根本不在 sessions 数组里，
/// `s.pty_id === t.id` 无从匹配，标题一直停在「新会话」；⌘R 走的还是 `list_sessions`，
/// 所以刷新同样无效；而 resume 出来的 tab 早就有 jsonl，看着"是好的"。
/// 三种表现是同一个根因：绑定挂在了一个比它自己晚出现的数据源上。
///
/// 只对前端传进来的 pty_ids 干活：没有待绑定 tab 时前端不会调这个命令，一次 ps 都不跑。
/// 匹配满了就早退，正常情况（一个新 tab）只查到第一个命中就结束。
pub fn resolve_pty_bindings(pty_ids: Vec<String>) -> Vec<PtyBinding> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    resolve_bindings_in(
        &home.join(".claude").join("sessions"),
        &pty_ids,
        // 「这个 pid 现在归哪个 pty」合成一问：进程已经没了就当没有 marker。
        // sessions/*.json 不保证在 claude 退出时被清掉，先 kill(0) 挡掉死 pid，
        // 省一次 ps。
        &|pid| {
            if !pid_alive(pid) {
                return None;
            }
            get_env_var_of_pid(pid, "MAKIT_PTY_ID")
        },
    )
}

/// `resolve_pty_bindings` 的纯逻辑部分：目录 + 想要的 pty_ids + 「pid → pty_id」查询。
/// 抽出来是为了能测 —— 真实实现要一个活着的、带 MAKIT_PTY_ID 的非 Apple 签名进程，
/// 在单测里造不出来。
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
        // 每个 tab 只能绑一个 session，配额满了就不必再扫（常见情况是只有一个新 tab）
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
    // signal 0：只做存在性/权限检查，不真的发信号。但它对**僵尸**（已死、父进程还没 wait）也返回成功，
    // 所以要再排除僵尸，否则被 kill 的会话在父进程回收前一直显示运行中
    let exists = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    exists && !is_zombie(pid)
}

/// 进程是不是僵尸。macOS 用 `proc_pidinfo`（比 fork 一次 `ps` 便宜得多，巡检每 3 秒要问一遍）。
/// 实测（macOS 26）：僵尸进程 `kill(pid, 0)` 成功、`ps` 显示状态 Z，而 `proc_pidinfo` 返回 0 且 errno = ESRCH。
/// 其它失败（比如 EPERM：别的用户的进程）当作活着，宁可多显示也不误杀
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

    /// 回归 #169 的第二半：「新建 shell 里手敲 claude」的 tab 标题不同步。
    ///
    /// 原来的绑定挂在 `sessions` 数组上（`list_sessions` → 扫
    /// `~/.claude/projects/**/*.jsonl`），而那个 jsonl **要等用户发出第一条消息之后
    /// 才被创建**。于是新起的 claude 在发消息前根本不在 sessions 里，
    /// `s.pty_id === t.id` 无从匹配，tab.sessionId 一直是 null，标题停在「新会话」。
    /// ⌘R 走的还是 `list_sessions`，所以"刷新也没修复"；resume 出来的 tab 早就有
    /// jsonl，所以"恢复的是可以的"——用户给的三句话是同一个根因的三个侧面。
    ///
    /// 真正需要的事实只有 `(pty_id, session_id)`，claude 启动那一刻就写进了
    /// `~/.claude/sessions/<pid>.json`。这里测的就是"只读那个目录也够"。
    ///
    /// 实测证据（修之前）：pid 86431 的 MAKIT_PTY_ID=t_msngclks3eo5、session 32318d5f
    /// 活了近 2 小时，而 localStorage 里 id 为 t_msngclks3eo5 的 tab 仍是
    /// `kind:"new" / label:"新会话" / sessionId:null` —— 因为 32318d5f.jsonl 不存在。
    fn write(dir: &std::path::Path, file: &str, json: &str) {
        fs::write(dir.join(file), json).unwrap();
    }

    #[test]
    fn binds_from_sessions_dir_without_any_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-bind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 想绑的那个：pid 4321 → t_wanted
        write(
            &dir,
            "4321.json",
            r#"{"pid":4321,"sessionId":"32318d5f-1111-2222-3333-444455556666","name":"ai-claw-studio-6d"}"#,
        );
        // 别人家的 pty，不能串台
        write(
            &dir,
            "4322.json",
            r#"{"pid":4322,"sessionId":"deadbeef-0000-0000-0000-000000000000","name":"别人"}"#,
        );
        // pid=0（claude 没写 pid）：必须跳过，且不该去查环境变量
        write(
            &dir,
            "4323.json",
            r#"{"pid":0,"sessionId":"aaaaaaaa-0000-0000-0000-000000000000"}"#,
        );
        // 不是 json 后缀 / 坏 json：都不能让整个扫描挂掉
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

        // pty_ids 为空：一次目录读都不该发生 —— 前端没有待绑定 tab 时的零成本路径
        let never = |_pid: u32| -> Option<String> {
            panic!("pty_ids 为空时不该查任何 pid");
        };
        assert!(super::resolve_bindings_in(&dir, &[], &never).is_empty());

        // 匹配不上就是空，而不是"随便挑一个回去"
        let out = super::resolve_bindings_in(&dir, &["t_nobody".to_string()], &pty_of);
        assert!(out.is_empty(), "没有命中的 pty_id 时必须返回空");

        let _ = fs::remove_dir_all(&dir);
    }
}

// ─────────────── codex 的标签绑定（#231）───────────────
//
// claude 运行时写 `~/.claude/sessions/<pid>.json`（pid + 会话 id），标签绑定靠它；codex 不写这个文件，
// 所以在 shell / 新建标签里手敲 `codex` 永远绑不上会话（状态点、可重排视图、关标签杀进程都拿不到）。
// codex 这边的两头事实：进程环境里有 `MAKIT_PTY_ID`（= 标签 id）；进程开着自己的 rollout 文件，
// 文件第一行 `session_meta.payload.id` 就是会话 id。

/// `lsof -Fn` 的输出里，codex 开着的会话文件（在 `sessions_dir` 下、以 .jsonl 结尾）
pub fn rollout_paths_from_lsof(output: &str, sessions_dir: &Path) -> Vec<std::path::PathBuf> {
    output
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .map(std::path::PathBuf::from)
        .filter(|p| p.starts_with(sessions_dir) && p.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

/// rollout 文件第一行 `session_meta` 里的会话 id
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

/// 正在运行、`MAKIT_PTY_ID` 在 `pty_ids` 里的 codex 进程 → (标签 id, 会话 id)。
/// 只在有待绑定标签时调（每次一趟 pgrep + 每个命中进程一次 ps / lsof）
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

    /// 为什么要测：错了在界面上就是「进程被 kill 之后侧栏一直显示运行中」「点它提示正在运行中不能重复启动，打不开」
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

    /// 被杀掉的进程在父进程 wait 之前是僵尸：`kill(pid, 0)` 对僵尸照样成功，把「已经死了」误报成「还活着」。
    /// 端到端实测就栽在这上面：kill 之后侧栏 8 秒都还显示运行中
    #[test]
    fn a_zombie_is_not_alive() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        // 不 wait：等它退出后成为僵尸（父进程是本测试进程，没回收）
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

    /// 为什么要测：错了在界面上就是「在 shell 里跑 codex，标签一直显示 shell、没有状态点、可重排视图不出现」
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
