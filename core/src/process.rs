//! 进程表、进程环境变量、后代进程收集，以及关 tab 时的杀进程（进程树 + 环境变量标记）。

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
    // flag 只能是 `-Eww`：macOS 的 ps 里 `-e` 是 `-A` 的同义词（显示所有进程），
    // 显示环境变量的是大写 `-E`；`-x`（把无控制终端的进程并进来）更不能加 ——
    // BSD ps 的选择条件是 OR，加一个就把 `-p` 淹掉，返回整张进程表。
    // 两者叠在一起的老写法 `-xeww` 是"756 行、零个环境变量"，恒定返回 None。
    // `-ww` 保留：不加会按终端宽度截断，环境变量正好在末尾，最先被切掉。
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
    // 也检查用空格分隔的情况
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

/// pid → (ppid, 进程名)。macOS 走 libproc（#216）：ps 要读全部进程的完整参数，0.3s；
/// 这里只取 pid / ppid / 进程名，几毫秒。完整命令行由 `full_command` 只给要输出的进程读。
#[cfg(target_os = "macos")]
pub fn collect_process_table() -> HashMap<u32, (u32, String)> {
    use std::ffi::{c_void, CStr};
    let mut table = HashMap::new();
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return table;
    }
    // 两次调用之间可能有新进程，多留一些余量
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
            continue; // 进程刚退出，或没权限
        }
        let name = unsafe { CStr::from_ptr(info.pbi_comm.as_ptr()) }.to_string_lossy().into_owned();
        table.insert(pid as u32, (info.pbi_ppid, name));
    }
    table
}

/// 完整命令行（和 `ps -o command=` 一样：参数用空格连起来）。读不到（进程已退出、
/// 别的用户的进程）就退回进程名，ps 在这种情况下也是只显示进程名。
#[cfg(target_os = "macos")]
pub fn full_command(pid: u32, fallback: &str) -> String {
    process_args(pid).unwrap_or_else(|| fallback.to_string())
}

/// KERN_PROCARGS2 的布局：argc(i32) | 可执行文件路径 \0 | 若干 \0 填充 | argv[0..argc] 各以 \0 结尾 | 环境变量…
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
    // 跳过可执行文件路径和它后面的 \0 填充
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
    cmd.to_string() // ps 版的进程表里本来就是完整命令行
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

// 扫 PPID=1 的孤儿进程，检查 env 里是否有 MAKIT_SESSION_ID=<session_id>
// 返回 (session_id, ProcessInfo) 列表
pub fn collect_orphan_by_env(table: &HashMap<u32, (u32, String)>) -> Vec<(String, ProcessInfo)> {
    let orphans: Vec<u32> = table
        .iter()
        .filter(|(_, (ppid, cmd))| {
            *ppid == 1 && {
                let exe = cmd.split_whitespace().next().unwrap_or("");
                let base = exe.rsplit('/').next().unwrap_or(exe);
                // 只扫服务类进程（避免对所有 393 个孤儿跑 ps eww）
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
    // 批量 ps eww 拿环境变量
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
        // 从 env 部分提取 MAKIT_SESSION_ID=xxx
        if let Some(pos) = line.find("MAKIT_SESSION_ID=") {
            let after = &line[pos + 15..];
            let session_id = after.split(|c: char| c.is_whitespace() || c == '\0')
                .next()
                .unwrap_or("")
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            // 从行首提取 PID
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

/// #216：进程表以前靠 `ps -eo pid=,ppid=,command=`，macOS 的 ps 要读全部 ~700 个进程的
/// 完整参数，0.3s（启动时抢 CPU 能到 0.55s），是缓存落盘后启动路径上最大的一块。
/// 改成 libproc 只取 pid / ppid / 进程名，完整命令行只给最后要输出的那几个进程读。
/// 错了在 UI 上：运行中会话的「子进程」列表缺项、命令行只剩进程名，或孤儿服务进程找不回来。
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
        // 两次取样之间会有进程生灭，所以按比例判；pid / ppid 本身不会变
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
    /// 回归：`get_env_var_of_pid` 必须真的读到**指定那个 pid** 的环境变量。
    ///
    /// 原来的实现是 `ps -p PID -xeww -o command=`，两个 flag 都错了：
    /// macOS 的 `ps` 里 `-e` 是 `-A` 的同义词（"显示所有进程"），显示环境变量的是
    /// **大写 `-E`**；而 `-x` 又会把没有控制终端的进程（全部 daemon）并进来 ——
    /// BSD ps 的选择条件是 OR 而不是 AND，所以 `-p` 被彻底淹没。实测这条命令返回
    /// 756 行整张进程表、且一个环境变量都没有，函数于是恒定返回 None。
    ///
    /// 后果不是"偶尔取不到"而是"永远取不到"：`pty_id` 恒为空串 → App.tsx 里
    /// `sessions.filter(s => s.running && s.pty_id)` 全被空串的 falsy 过滤掉 →
    /// 在新建 shell 里手敲 claude 的那个 tab 永远绑不上 session，标题一直停在
    /// "新会话"（⌘R 也救不回来，全量 load 走的是同一条命令）。
    ///
    /// 用自己的 pid 当被测对象：测试二进制是本项目的编译产物，不是 Apple 平台
    /// 二进制，环境变量读得到（SIP 只挡 /bin/* 那类签名平台二进制）。
    /// 拿 PATH 做**全等**比较而不是判非空 —— 只判非空的话，"从整张进程表里捞到
    /// 别人的 PATH" 这种串台也能过。
    /// 取样变量刻意挑"值里没有空白"的那一个：ps 输出里环境变量之间就是用空格
    /// 分隔的，值本身含空格（比如 PATH 里有 `/Library/Application Support/...`）
    /// 根本无法无歧义还原 —— 这是 ps 输出格式的固有限制，不是可修的 bug。
    /// 唯一的调用方只读 MAKIT_PTY_ID，值形如 `t_ms5nflsm3a0g`，不受影响。
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
        // 没设过的变量必须是 None，而不是从别处捞一个回来
        assert_eq!(
            super::get_env_var_of_pid(me, "MAKIT_DEFINITELY_UNSET_VAR_9f3a"),
            None
        );
    }
}

/// 按 pid 收口出来是为了能测（#142）——`kill_pty` 拿的是 portable-pty 的 `Child`，
/// 测试里造不出来。关 tab 走的就是这两句。
#[cfg(unix)]
pub fn kill_pty_by_pid(pid: u32, pty_id: &str) {
    kill_tree(pid);
    kill_by_env_marker(&[pty_id]);
}

/// 杀进程树：killpg 打主进程组 + `pgrep -P` 递归逐个杀后代。
/// 覆盖 plain / nohup / setsid_child / nested 四种形态；reparent 到 1 的那两种够不着，
/// 交给 `kill_by_env_marker`（#142 的矩阵测试把这条分界线钉住了）。
#[cfg(unix)]
pub fn kill_tree(pid: u32) {
    // 1) 先递归找所有后代进程（在发 signal 前收集，避免进程退出后丢失）
    let descendants = find_descendants(pid);

    unsafe {
        // 2) killpg 杀主进程组。
        //    注意 `killpg` 的参数是 **pgid** 而不是 pid，这里能直接传 pid 是因为
        //    portable-pty 给子 shell setsid 过，pid == pgid。这个前提在测试里显式立住。
        libc::killpg(pid as libc::pid_t, libc::SIGHUP);
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);

        // 3) 逐个杀后代进程（覆盖 setsid 逃逸的）
        for dpid in &descendants {
            libc::kill(*dpid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
pub fn find_descendants(root_pid: u32) -> Vec<u32> {
    use std::process::Command;
    // pgrep -P <pid> 找直接子进程，递归
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

/// 按 `MAKIT_PTY_ID` 环境变量杀掉逃逸进程 —— setsid / double-fork 之后 PPID=1、
/// 进程组也跟我们脱钩的那些（#3）。这是它们唯一还认得出来的印记：环境变量是
/// fork 时复制的，逃到哪都带着，而 PPID 链和进程组都已经断了。
///
/// **默认真杀**。取证时设 `MAKIT_KILL_ESCAPED=0` 退回 dry-run（只打印不发信号），
/// 和 `MAKIT_TIMING` 同一套做法，不用重新编译。
///
/// 为什么敢默认杀：匹配的是整 token `MAKIT_PTY_ID=<这个 tab 的 id>`，id 是本进程
/// 生成的、只可能出现在这个 tab 拉起来的进程上；别的 tab、别的 app、用户自己的进程
/// 都不带。误伤面只有一种 —— 用户在这个 tab 里主动 nohup/setsid 出去、指望它活过关
/// tab 的进程；这条取舍写在 #3 里（tab 的语义是一个 claude 会话，不是通用终端）。
///
/// 日志看法：dev 下直接打在 `pnpm tauri dev` 的终端里；release 的 .app 要从终端
/// 起（`open` 出来的看不到 stderr）。
#[cfg(unix)]
pub fn kill_by_env_marker(pty_ids: &[&str]) {
    use std::process::Command;
    if pty_ids.is_empty() { return; }
    let my_pid = std::process::id();
    // 只有显式写 0 才退回 dry-run；没设 = 真杀
    let dry_run = std::env::var("MAKIT_KILL_ESCAPED").map(|v| v == "0").unwrap_or(false);
    // flag 必须是 `-xEww`：显示环境变量的是**大写 `-E`**，小写 `-e` 在 macOS 的 ps
    // 里是 `-A` 的同义词（"显示所有进程"）。老写法 `-xeww` 于是拿到了一张不含任何
    // 环境变量的全进程表，下面 `MAKIT_PTY_ID=` 的匹配永远为假 —— 这个兜底一直在空转。
    // 这里 `-x` 是**故意**留的：要找的就是脱离了控制终端的逃逸进程。
    let output = match Command::new("ps").args(["-xEww", "-o", "pid,command"]).output() {
        Ok(o) if o.status.success() => o,
        _ => {
            eprintln!("[escaped-pty] ps 执行失败，兜底清理跳过");
            return;
        }
    };
    let mut hits: Vec<(u32, &str)> = Vec::new();
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        // 解析 PID（行首数字）
        let pid: u32 = match trimmed.split_whitespace().next().unwrap_or("").parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pid == my_pid || pid <= 1 { continue; }
        // 整 token 相等而不是 contains：`MAKIT_PTY_ID=t_abc` 会被 `contains` 判成
        // 命中 `MAKIT_PTY_ID=t_abcdef`。当前 id 都是等长的所以撞不上，但这是一条
        // 真杀的路径，不留这种"靠格式凑巧"的前提。
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
        // 没命中只在 dev 里打一行 —— 关 tab 是每天几十次的路径，release 不该刷屏；
        // 但"跑了但没找到"和"根本没跑"必须分得开，这个 bug 本身就是被后者掩盖了三个月。
        #[cfg(debug_assertions)]
        eprintln!("[escaped-pty] 扫了 {} 个 pty id，没有逃逸进程", pty_ids.len());
        return;
    }
    for (pid, pty_id) in &hits {
        if dry_run {
            eprintln!("[escaped-pty] dry-run 命中 pid={pid} pty_id={pty_id}（MAKIT_KILL_ESCAPED=0，未发信号）");
        } else {
            eprintln!("[escaped-pty] SIGKILL pid={pid} pty_id={pty_id}");
            unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL); }
        }
    }
}

/// #142 子进程 kill 全场景验证。
///
/// 目的不是"测一个函数的返回值"，而是**量出 `kill_tree` 到底覆盖哪几种子进程形态**：
/// 六种形态各造一个真进程，发真信号，再看谁还活着。关 tab 杀不干净这件事之前一直
/// 靠猜（#3、#142），这里把它变成可复现的测量。
///
/// 安全边界（这里在动真信号，一个写错的 pgid 能把用户正在跑的 claude 全带走）：
///   1. 合成树的根用 `setsid()` 起 —— 它必须有**自己的进程组**，否则 `killpg` 打的
///      就是 cargo test 自己所在的组，等于自杀。
///   2. 发信号前显式断言 `pgid(root) == root` 且 `pgid(root) != pgid(self)` 且 `root > 1`；
///      断言在 `kill_tree` 之前，不满足就先清理再 panic，绝不带着错的 pgid 往下走。
///   3. 所有合成进程都带 `MAKIT_KILL_MATRIX=<nonce>` 环境变量，收尾按 nonce 兜底清扫，
///      中途 panic 也不会在机器上留下一堆 sleep。
/// 逐个 SIGKILL（前端「杀掉逃出进程组的子进程」按钮）。非 unix 上什么都不做。
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

    /// 六种形态：名字 + 它相对「killpg + pgrep -P 递归」这套机制处在什么位置。
    const SHAPES: [(&str, &str); 6] = [
        ("plain", "普通子进程：同进程组，killpg 直接覆盖"),
        ("nohup", "nohup：忽略 SIGHUP，同进程组，靠后面那发 SIGKILL"),
        ("setsid_child", "setsid 子进程：新进程组，但 PPID 仍是 root → pgrep -P 能找到"),
        ("nested", "嵌套孙进程：root → sh → sleep，全在同进程组"),
        ("orphan_setsid", "逃逸孙进程：中间 sh 立刻退出 → 被 reparent 到 1，PPID 链断 + 新进程组"),
        ("double_fork", "经典 double-fork daemon：PPID=1 + 新 session"),
    ];

    /// 活着的 pid 集合。用 `ps` 的 state 而不是 `kill(pid, 0)`：被杀掉的子进程会先变成
    /// 僵尸，`kill(pid, 0)` 对僵尸照样返回 0，会把"已经杀掉"误报成"还活着"。
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

    /// 兜底清扫：按 nonce 环境变量找所有还活着的合成进程并 SIGKILL。返回清掉的 pid。
    /// 这条路径不能依赖前面收集到的 pid —— 逃逸进程的 pid 有可能就是没记上的那个。
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
        // 叶子进程刻意用 python3，不用 `sleep`。
        //
        // 因为 /bin/sleep 和 /bin/sh 都是 SIP 保护的平台二进制，macOS 不允许读它们的
        // 环境变量：`ps -axEww` 对它们只输出命令行，环境变量整段是空的。实测
        // （/bin/sleep ✗、/bin/sh ✗、/usr/bin/python3 ✓、node ✓）。而下面要验的正是
        // 「环境变量兜底能不能看见逃逸进程」，用 sleep 当叶子会量出一个和真实场景
        // 无关的假阴性 —— 真实场景里跑的是 node（claude），环境变量是可见的。
        //
        // 顺带记一笔：把 /bin/sleep 拷一份出来跑也不行，平台二进制的签名换了位置就
        // 失效，exec 直接被内核拒掉。
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

        // 根脚本：六种形态各起一个，然后 wait 住 —— root 必须活着，否则 killpg 的目标组就没了。
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

    /// 等六个 pid 文件都写全。不等就发信号的话，⑤⑥ 可能还没 setsid 完，
    /// 会把"没来得及逃"读成"没逃掉"—— 那是个假的绿灯。
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

    /// 一棵活着的六形态合成树。两个测试各造一棵，靠 `tag` 区分 —— nonce / 临时目录 /
    /// `MAKIT_PTY_ID` 都带上它，否则 cargo 并发跑两个测试时，一个的兜底清扫会把另一个的
    /// 进程扫掉，症状是随机绿/随机红。
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

        /// 哪些叶子还活着（名字 + pid）
        fn survivors(&self) -> Vec<(&'static str, u32)> {
            let live = live_pids(&self.pids());
            self.leaves.iter().filter(|(_, p)| live.contains(p)).cloned().collect()
        }

        /// 收尾：按 nonce 兜底清扫 + 杀根 + 删临时目录。返回兜底清掉的 pid。
        /// **必须跑在断言之前** —— 断言 panic 了也不能给机器留下一堆 python。
        fn finish(mut self) -> Vec<u32> {
            let swept = sweep_by_nonce(&self.nonce);
            let _ = self.root.kill();
            let _ = self.root.wait();
            let _ = fs::remove_dir_all(&self.dir);
            swept
        }
    }

    /// 造一棵合成树并立住安全边界（这里在动真信号，一个写错的 pgid 能把用户正在跑的 claude 全带走）：
    ///   1. 根用 `setsid()` 起 —— 必须有**自己的进程组**，否则 `killpg` 打的就是 cargo test 自己所在的组；
    ///   2. 返回前断言 `pgid(root) == root` 且 `!= pgid(self)` 且 `root > 1`，不满足就先清理再 panic；
    ///   3. 六个叶子都得先活着，否则后面的"死了"没有意义。
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
            // 真实 pty 起的进程都带这个（pty.rs 里 spawn 时设的），逃逸兜底就认它
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

    /// 哪些还活着的合成进程能被 `ps` 的环境变量看见（兜底认得出来的那批）
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

    /// 机制测量（#142）：单靠 `kill_tree`（killpg + `pgrep -P` 递归）能覆盖到哪几种形态。
    ///
    /// 这不是"该有的行为"，是**机制上限**：reparent 到 1 的两种必然逃掉。它存在的意义是
    /// 钉住「哪四种是白纸黑字保证死的」，以及「逃掉的那两种至少还看得见」——
    /// 看得见，兜底就有救（`kill_by_env_marker`）；看不见，#3 就彻底没抓手了。
    #[test]
    fn kill_tree_covers_all_child_shapes() {
        if !have_python3() {
            eprintln!("[#142] 没有 python3，跳过（setsid / double-fork 形态造不出来）");
            return;
        }
        let m = spawn_matrix("mech");
        let nonce = m.nonce.clone();

        // ===== 被测行为 =====
        super::kill_tree(m.root.id());

        // 信号是异步的，给内核一点时间把进程收走
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

    /// 关 tab 真正走的那条路（#3）：`kill_tree` + 按 `MAKIT_PTY_ID` 扫逃逸进程。
    /// 契约比上面那条强一档 —— **六种全死**，一个都不许剩。
    ///
    /// 这条是用户能看见的那件事：关掉 tab 之后，这个会话不能还在后台烧内存和 token。
    #[test]
    fn close_path_kills_every_child_shape() {
        if !have_python3() {
            eprintln!("[#3] 没有 python3，跳过（setsid / double-fork 形态造不出来）");
            return;
        }
        let m = spawn_matrix("close");
        let pty_id = m.pty_id.clone();

        // ===== 被测行为：`pty_kill` 拿到 Child 之后干的就是这一句 =====
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


/// 真 pty 端到端（#3）：这条和 `kill_matrix_tests` 的区别是**没有任何模拟**——
/// 用生产代码同一个 `portable_pty::native_pty_system()` 开一个真 pty，起真 shell，
/// 在里面用 node（claude 就是 node）起一个 detached 孙进程后父进程立刻退出，
/// 于是孙进程 reparent 到 1 —— 这就是用户关不掉的那个东西的真实形状。
///
/// 它验的是三件光靠合成树验不到的事：
///   1. `killpg(pid)` 的前提成立：portable-pty 起的 shell 真的 `pid == pgid`；
///   2. `MAKIT_PTY_ID` 真的一路继承到逃逸的孙进程（pty → shell → node → detached node）；
///   3. node 进程的环境变量在 `ps -xEww` 里真的看得见 —— SIP 保护的平台二进制
///      （/bin/sh、/bin/sleep）是看不见的，兜底认不出它们。claude 是 node，所以这条成立。
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

    /// 读一个进程的 (ppid, pgid)，进程没了返回 None
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

        // 父 node：起一个 detached（= setsid）孙进程，然后自己立刻退出 → 孙被 reparent 到 1
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

        // ===== 和 pty_spawn 走同一套：openpty + CommandBuilder + MAKIT_PTY_ID =====
        let pair = native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .expect("openpty 失败");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("node \"$MK_D/escape.js\"; sleep 300");
        cmd.cwd(&dir);
        cmd.env("MK_D", dir.to_string_lossy().to_string());
        cmd.env(MARKER, &nonce);
        cmd.env("MAKIT_PTY_ID", &pty_id); // 生产代码 pty_spawn 里设的就是这一行
        let mut child = pair.slave.spawn_command(cmd).expect("pty 里起 shell 失败");
        drop(pair.slave);
        let shell_pid = child.process_id().expect("拿不到 pty 子进程 pid");

        // 等孙进程写下自己的 pid
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

        // 前置测量：它是不是真的逃了，以及 killpg 的前提成不成立
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

        // ===== 被测行为：关这个 tab =====
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
