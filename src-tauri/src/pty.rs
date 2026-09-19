use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{Emitter, State, Window};

// cwd 不存在时（session 自己 mv 走目录、外部删目录），向上找最近存在的祖先
// 找不到则回退 home，再不行 "/"——保证 shell 能起来
fn expand_tilde(cwd: &str) -> String {
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

fn resolve_existing_cwd(cwd: &str) -> String {
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

/// 这个 pane 该不该拒绝启动。收成一个纯谓词只为了能测 —— `pty_spawn` 本体要
/// `Window` + `State`，测试里造不出来（和 `kill_tree` 从 `kill_process_group` 里
/// 抽出来是同一个理由）。
fn must_refuse_cwd(cwd: &str, allow_fallback: Option<bool>) -> bool {
    allow_fallback == Some(false) && resolve_existing_cwd(cwd) != expand_tilde(cwd)
}

/// `clear && claude -r <id>`（`workspace-types.ts` 的 resumeInitCommand）里的会话 id。
/// id 会拼进文件路径，所以只认 uuid 形状（36 位 hex 和 `-`）。
fn resume_session_id(init_command: &str) -> Option<&str> {
    let id = init_command.split("claude -r ").nth(1)?.split_whitespace().next()?;
    let uuid_shaped = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    uuid_shaped.then_some(id)
}

/// 会话的起始目录：第一条主线（非 sidechain）用户记录的 cwd。后面的记录会被「在别的目录
/// `claude -r`」改写，不能用。
fn first_user_cwd(reader: impl std::io::BufRead) -> Option<String> {
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

/// resume tab 要不要改到会话的起始目录启动：起始目录**还在**且和请求的不同才改。
/// 起始目录不在了 = 项目被移走过（用户可能已用「指到新位置」恢复），这时 tab 记的新位置才是对的，
/// 硬改回去会和 cwd-missing 恢复流程互相打架。
fn corrected_resume_cwd(requested: &str, home: Option<String>) -> Option<String> {
    home.filter(|h| Path::new(h).is_dir() && *h != expand_tilde(requested))
}

/// 在 `~/.claude/projects/*/<id>.jsonl` 里找这个会话，返回它的起始目录。
fn session_home_cwd(projects_dir: &Path, session_id: &str) -> Option<String> {
    let file = std::fs::read_dir(projects_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join(format!("{session_id}.jsonl")))
        .find(|p| p.is_file())?;
    first_user_cwd(std::io::BufReader::new(std::fs::File::open(file).ok()?))
}

pub struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[derive(Default)]
pub struct PtyState {
    pub inner: Mutex<HashMap<String, PtyHandle>>,
}

#[tauri::command]
pub async fn pty_spawn(
    window: Window,
    state: State<'_, PtyState>,
    id: String,
    cwd: String,
    cols: u16,
    rows: u16,
    init_command: Option<String>,
    allow_cwd_fallback: Option<bool>,
) -> Result<Option<String>, String> {
    {
        let map = state.inner.lock().unwrap();
        if map.contains_key(&id) {
            return Ok(None);
        }
    }

    // resume tab 在会话自己的起始目录启动（#190），不信 tab 记的 cwd：那个值可能来自「在别的
    // shell 里手打 claude -r」再升级成的 tab，照它启动会让 claude 在错的仓库里继续这个会话。
    // 和下面那道 cwd 闸同一个理由放在这里：五条打开会话的路（含重启后恢复的 tab）都从这儿过。
    // 校正了就把新目录返回给前端，让它改掉持久化的 tab 记录（否则从这个 tab 新开的终端还会继承旧目录）。
    let home = init_command
        .as_deref()
        .and_then(resume_session_id)
        .and_then(|sid| dirs::home_dir().and_then(|h| session_home_cwd(&h.join(".claude").join("projects"), sid)));
    let corrected = corrected_resume_cwd(&cwd, home);
    let cwd = corrected.clone().unwrap_or(cwd);

    // 目录不存在时到底该降级还是该拒绝，取决于**这个 pane 是干什么的** —— 而那只有调用方知道。
    //
    // shell / new tab：降级是对的，目的就是弄起一个 shell，在哪儿都行。
    // resume tab：降级是**有害**的。cwd 决定了 claude 去哪儿找会话
    // （`~/.claude/projects/<encode(realpath(cwd))>/<id>.jsonl`），换 cwd 就是换钥匙 ——
    // 于是一个「目录没了」会变成一个看起来没救的「会话没了」，而黄字只说了句「已切换到 X」。
    //
    // 这道闸放在这里而不是放在前端的某个入口，是因为**五条路都从这儿过**：侧栏点击、
    // 侧栏拖拽、SessionTree 拖拽、⌘K+修饰键、以及重启后从持久化 workspace 恢复的 tab。
    // 最后那条最常见（目录通常是在 app 关着的时候被删/改名的）也最容易漏 —— 它手里
    // 只有一条存下来的 tab 记录，前端任何「打开会话」的入口都拦不到它。
    if must_refuse_cwd(&cwd, allow_cwd_fallback) {
        // 机器可读前缀：前端靠它区分「目录没了、可恢复」和真正的 PTY 故障
        return Err(format!("cwd-missing:{}", cwd));
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l"); // login shell：读 .zprofile/.zshenv → 拿到完整 PATH（含 brew/cargo/nvm）
    let resolved_cwd = resolve_existing_cwd(&cwd);
    let expanded_cwd = expand_tilde(&cwd);
    if resolved_cwd != expanded_cwd {
        let _ = window.emit(
            "pty:cwd-fallback",
            serde_json::json!({ "id": &id, "requested": &cwd, "actual": &resolved_cwd }),
        );
        // 直接往 xterm 写黄字提示（不进入 PTY，不会被 shell 当命令执行）
        let msg = format!(
            "\x1b[33m原目录不存在: {} → 已切换到: {}\x1b[0m\r\n",
            cwd, resolved_cwd
        );
        let _ = window.emit(&format!("pty:data:{}", id), msg);
    }
    cmd.cwd(&resolved_cwd);
    cmd.env("TERM", "xterm-256color");
    // GUI 启动的子进程可能缺失 locale，强制 UTF-8 防止中文乱码
    cmd.env("LANG", "en_US.UTF-8");
    cmd.env("LC_ALL", "en_US.UTF-8");
    // 注入标识环境变量：所有从此 PTY fork 的子进程（含 nohup &）都继承
    cmd.env("MAKIT_PTY_ID", &id);

    // ===== Shell 集成：自动发 OSC 7 通知 cwd 变化 =====
    // 用 ZDOTDIR 接管 zsh 的 .zshrc 加载点：先 source 用户原 .zshrc，再加 OSC 7 hook
    // bash: 用 --rcfile（这里只为 zsh 做，macOS 默认 zsh）
    if shell.ends_with("zsh") || shell.ends_with("/zsh") {
        if let Some(home) = dirs::home_dir() {
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

            cmd.env("ZDOTDIR", integ_dir.to_string_lossy().to_string());
        }
    }
    // =============================================
    // 从 initCommand 提取 session_id（格式：`claude -r <session_id>`）
    if let Some(ref ic) = init_command {
        if let Some(sid) = ic.strip_prefix("claude -r ").map(|s| s.trim()) {
            if !sid.is_empty() {
                cmd.env("MAKIT_SESSION_ID", sid);
            }
        }
    }
    // -l: login shell。GUI 启动时 PATH 不含 brew/cargo 等（不走 shell profile）
    // login shell 会读 .zprofile → 拿到完整 PATH

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| e.to_string())?;

    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    // 如果有初始命令（例如 `claude -r <id>`），立即写入 PTY 的 stdin
    // OS 层会缓冲，zsh 加载完 .zshrc 后会读到并执行
    if let Some(cmd_line) = init_command.as_ref() {
        let trimmed = cmd_line.trim();
        if !trimmed.is_empty() {
            let to_write = format!("{}\r", trimmed);
            let _ = writer.write_all(to_write.as_bytes());
            let _ = writer.flush();
        }
    }

    let id_for_thread = id.clone();
    let win = window.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 32768]; // 32KB：减少高输出时 emit 事件频率
        // 跨 read 缓冲不完整的 UTF-8 字节序列（例如中文 3 字节字符可能被切断）
        let mut leftover: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    leftover.extend_from_slice(&buf[..n]);
                    // 找到最长有效 UTF-8 前缀，剩余字节留到下次
                    let valid_up_to = match std::str::from_utf8(&leftover) {
                        Ok(_) => leftover.len(),
                        Err(e) => e.valid_up_to(),
                    };
                    if valid_up_to == 0 {
                        // 全部都是不完整序列（罕见），等下一轮
                        continue;
                    }
                    let valid_part = &leftover[..valid_up_to];
                    let chunk = unsafe {
                        // 上面已经验证过 valid_up_to 是有效 UTF-8 边界
                        std::str::from_utf8_unchecked(valid_part).to_string()
                    };
                    let rest = leftover[valid_up_to..].to_vec();
                    leftover = rest;
                    if win
                        .emit(&format!("pty:data:{}", id_for_thread), chunk)
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = win.emit(&format!("pty:exit:{}", id_for_thread), 0_i32);
    });

    let handle = PtyHandle {
        master: pair.master,
        writer,
        child,
    };

    state.inner.lock().unwrap().insert(id, handle);
    Ok(corrected)
}

#[tauri::command]
pub async fn pty_write(
    state: State<'_, PtyState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let mut map = state.inner.lock().unwrap();
    let handle = map.get_mut(&id).ok_or_else(|| "pty not found".to_string())?;
    handle
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())?;
    handle.writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn pty_resize(
    state: State<'_, PtyState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = state.inner.lock().unwrap();
    let handle = map.get(&id).ok_or_else(|| "pty not found".to_string())?;
    handle
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// 杀进程组 + 递归杀所有后代进程
// portable-pty 给子 shell setsid()，但 Claude 启动的工具进程可能又 setsid 了新组
// 所以除了 killpg，还要用 pgrep 找后代进程逐个杀
#[cfg(unix)]
fn kill_process_group(child: &dyn portable_pty::Child) {
    if let Some(pid) = child.process_id() {
        kill_tree(pid);
    }
}

/// 关一个 pty 时真正干活的那段。按 pid 收口出来是为了能测（#142）——
/// `kill_process_group` 拿的是 portable-pty 的 `Child`，测试里造不出来。
#[cfg(unix)]
fn kill_tree(pid: u32) {
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
fn find_descendants(root_pid: u32) -> Vec<u32> {
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

#[cfg(not(unix))]
fn kill_process_group(_child: &dyn portable_pty::Child) {}

#[tauri::command]
pub fn kill_pids(pids: Vec<u32>) -> Result<(), String> {
    #[cfg(unix)]
    {
        for pid in &pids {
            unsafe {
                libc::kill(*pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn pty_kill(state: State<'_, PtyState>, id: String) -> Result<(), String> {
    let mut map = state.inner.lock().unwrap();
    if let Some(mut handle) = map.remove(&id) {
        kill_process_group(handle.child.as_ref());
        let _ = handle.child.kill();
    }
    Ok(())
}

// 给 app 退出时调用：杀光所有还活着的 PTY 进程组
pub fn kill_all_ptys(state: &PtyState) {
    let mut map = state.inner.lock().unwrap();
    let ids: Vec<String> = map.keys().cloned().collect();
    for id in &ids {
        if let Some(mut handle) = map.remove(id) {
            kill_process_group(handle.child.as_ref());
            let _ = handle.child.kill();
        }
    }
    // 兜底：用 MAKIT_PTY_ID 环境变量找逃逸进程（setsid 后 PPID=1 的）
    #[cfg(unix)]
    kill_by_env_marker(&ids);
}

/// 兜底清理：靠 MAKIT_PTY_ID 环境变量找逃逸进程（setsid 之后 PPID=1、进程组已经
/// 跟我们脱钩的那些）。
///
/// **目前是 dry-run：只打印匹配结果，不发信号。** 原因是这个函数在修掉下面那个
/// flag 之前从来没有生效过（见注释），一旦生效就是一条"真杀"的路径，先观察一轮
/// 匹配集合是否就是预期的逃逸进程、有没有误伤，再放开。放开方式不需要重新编译：
/// 设 `MAKIT_KILL_ESCAPED=1`（和 `MAKIT_TIMING` 同一套做法）。
///
/// 日志看法：dev 下直接打在 `pnpm tauri dev` 的终端里；release 的 .app 要从终端
/// 起（`open` 出来的看不到 stderr）。没有匹配也会打一行 —— "跑了但没找到" 和
/// "根本没跑" 是两件必须分得开的事，这个 bug 本身就是被后者掩盖了三个月。
#[cfg(unix)]
fn kill_by_env_marker(pty_ids: &[String]) {
    use std::process::Command;
    if pty_ids.is_empty() { return; }
    let my_pid = std::process::id();
    let armed = std::env::var_os("MAKIT_KILL_ESCAPED").is_some();
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
        // 即将开始真杀的路径，不留这种"靠格式凑巧"的前提。
        for tok in trimmed.split_whitespace() {
            if let Some(val) = tok.strip_prefix("MAKIT_PTY_ID=") {
                if let Some(hit) = pty_ids.iter().find(|p| p.as_str() == val) {
                    hits.push((pid, hit.as_str()));
                }
                break;
            }
        }
    }
    if hits.is_empty() {
        eprintln!("[escaped-pty] 扫了 {} 个 pty id，没有逃逸进程", pty_ids.len());
        return;
    }
    for (pid, pty_id) in &hits {
        if armed {
            eprintln!("[escaped-pty] SIGKILL pid={pid} pty_id={pty_id}");
            unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL); }
        } else {
            eprintln!("[escaped-pty] dry-run 命中 pid={pid} pty_id={pty_id}（未发信号；设 MAKIT_KILL_ESCAPED=1 放开）");
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
#[cfg(all(test, unix))]
mod kill_matrix_tests {
    use std::collections::HashSet;
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const MARKER: &str = "MAKIT_KILL_MATRIX";

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
    fn live_pids(pids: &[u32]) -> HashSet<u32> {
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
    fn sweep_by_nonce(nonce: &str) -> Vec<u32> {
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

    #[test]
    fn kill_tree_covers_all_child_shapes() {
        if !have_python3() {
            eprintln!("[#142] 没有 python3，跳过（setsid / double-fork 形态造不出来）");
            return;
        }

        let nonce = format!("n{}", std::process::id());
        let dir = std::env::temp_dir().join(format!("makit-kill-matrix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_scripts(&dir);

        let mut cmd = Command::new("sh");
        cmd.arg(dir.join("root.sh"))
            .env("MK_D", &dir)
            .env(MARKER, &nonce)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // 安全边界 1：根必须自成进程组，否则下面的 killpg 打的是 cargo test 自己的组。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut root = cmd.spawn().expect("合成树的根起不来");
        let root_pid = root.id();

        let leaves = wait_for_pids(&dir);
        let pids: Vec<u32> = leaves.iter().map(|(_, p)| *p).collect();

        // 前置：六个都得先活着，否则后面的"死了"没有意义。
        let before = live_pids(&pids);
        let dead_early: Vec<&str> = leaves.iter().filter(|(_, p)| !before.contains(p)).map(|(n, _)| *n).collect();
        if !dead_early.is_empty() {
            sweep_by_nonce(&nonce);
            let _ = root.kill();
            let _ = root.wait();
            panic!("发信号之前就已经死了: {dead_early:?}");
        }

        // 安全边界 2：pgid 三连断言。不满足就先清理再 panic，绝不带着错的 pgid 往下走。
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
            sweep_by_nonce(&nonce);
            let _ = root.kill();
            let _ = root.wait();
            panic!("安全边界不满足，已中止发信号: {msg}");
        }

        // ===== 被测行为：关一个 pty 时走的就是这一句 =====
        super::kill_tree(root_pid);

        // 信号是异步的，给内核一点时间把进程收走
        std::thread::sleep(Duration::from_millis(600));
        let after = live_pids(&pids);

        let survivors: Vec<(&str, u32)> = leaves
            .iter()
            .filter(|(_, p)| after.contains(p))
            .map(|(n, p)| (*n, *p))
            .collect();

        // 逃逸进程还活着的这一刻，先量一件事：环境变量兜底（kill_by_env_marker 用的
        // `ps -axEww` + 标记匹配）能不能看见它们。看不见的话，#3「关 tab 后 claude 不退」
        // 就**没有任何**可用的清理机制，那是个比"少杀两种形态"严重得多的结论。
        let visible_by_env: Vec<u32> = {
            let out = Command::new("ps").args(["-axEww", "-o", "pid=,command="]).output();
            let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
            let needle = format!("{MARKER}={nonce}");
            text.lines()
                .filter(|l| l.contains(&needle))
                .filter_map(|l| l.split_whitespace().next().and_then(|s| s.parse::<u32>().ok()))
                .filter(|p| survivors.iter().any(|(_, sp)| sp == p))
                .collect()
        };

        // 收尾一定要跑在断言之前：断言 panic 了也不能给机器留下一堆 sleep。
        let swept = sweep_by_nonce(&nonce);
        let _ = root.kill();
        let _ = root.wait();
        let _ = fs::remove_dir_all(&dir);

        let dead = |name: &str| !survivors.iter().any(|(n, _)| *n == name);

        // ===== 断言 1：killpg + pgrep -P 递归**保证**覆盖的四种，必须死 =====
        // 这四种是 kill_tree 的真实契约。任何一种活下来都是回归。
        for name in ["plain", "nohup", "setsid_child", "nested"] {
            assert!(
                dead(name),
                "kill_tree 没杀掉 `{name}`（{}），这是它本该保证覆盖的形态。\n\
                 存活: {survivors:?}，已兜底清掉 {swept:?}",
                SHAPES.iter().find(|(s, _)| *s == name).map(|(_, d)| *d).unwrap_or("")
            );
        }

        // ===== 断言 2：逃掉的那些，必须至少能被环境变量兜底看见 =====
        // orphan_setsid / double_fork 会 reparent 到 pid 1：PPID 链断了（pgrep -P 找不到）、
        // 进程组也换了（killpg 打不到）。这是 kill_tree 的机制上限，不是 bug ——
        // 但"杀不掉"和"连看都看不见"是两件事。能看见，兜底就有救（kill_by_env_marker，
        // 目前还是 dry-run，见那个函数的注释）；看不见就彻底没抓手。
        let escaped: Vec<&str> = survivors.iter().map(|(n, _)| *n).collect();
        if !escaped.is_empty() {
            eprintln!(
                "[#142] kill_tree 机制上限：{:?} 逃掉了（reparent 到 1 → PPID 链断 + 新进程组）；\
                 环境变量兜底可见 {}/{} 个",
                escaped,
                visible_by_env.len(),
                survivors.len()
            );
            assert_eq!(
                visible_by_env.len(),
                survivors.len(),
                "逃逸进程 {escaped:?} 里有 {} 个连 `ps -axEww` 的环境变量都扫不到 —— \
                 那么 #3 没有任何可用的兜底路径。可见的: {visible_by_env:?}",
                survivors.len() - visible_by_env.len()
            );
        }
    }
}

/// resume tab 的 cwd 闸（#173）。
///
/// 这条不变量承担的是：**目录没了的 resume tab 必须启动失败，而不是换个目录悄悄起来**。
/// 换 cwd 就是换钥匙 —— claude 只会去 `projects/<encode(realpath(cwd))>/<id>.jsonl`
/// 找会话，降级到祖先目录之后它报的是「No conversation found」，把一个能修的问题
/// 伪装成一个没救的问题。
#[cfg(test)]
mod cwd_gate_tests {
    use super::{expand_tilde, must_refuse_cwd, resolve_existing_cwd};

    #[test]
    fn refuses_only_when_fallback_is_forbidden_and_dir_is_gone() {
        let gone = "/definitely/not/a/path/makit-cwd-gate";
        // 前提：这个路径确实会触发降级，否则下面几条断言什么都没测到
        assert_ne!(resolve_existing_cwd(gone), expand_tilde(gone));

        // resume tab：必须拒
        assert!(must_refuse_cwd(gone, Some(false)));

        // shell / new tab：降级是对的（目的就是弄起一个 shell，在哪儿都行）
        assert!(!must_refuse_cwd(gone, Some(true)));
        // 老调用方不传这个参数 → None → 保持原有降级行为，向后兼容
        assert!(!must_refuse_cwd(gone, None));
    }

    #[test]
    fn never_refuses_when_the_dir_is_actually_there() {
        let here = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(!must_refuse_cwd(&here, Some(false)), "目录在就不该拦");
    }
}

/// resume tab 必须在会话自己的起始目录启动（#190）。
///
/// 起因：在 shell 里手打 `claude -r` 后 tab 就地升级成 resume tab，但 tab 记的 cwd 还是那个 shell 的
/// 目录；新开终端又继承当前 tab 的 cwd —— 一个错目录会传给所有 tab。下次启动在错目录里
/// `claude -r`，claude 就在那个仓库里继续这个会话（记录里的 cwd、git 分支、Bash 默认目录全变）。
/// 会话的起始目录 = jsonl 第一条用户记录的 cwd = claude 存它的目录，是唯一可信的来源。
#[cfg(test)]
mod resume_cwd_tests {
    use super::{corrected_resume_cwd, first_user_cwd, resume_session_id, session_home_cwd};
    use std::io::Cursor;

    const SID: &str = "73ec5479-5b96-494c-919c-1f36a7e192fd";

    #[test]
    fn parses_session_id_from_resume_init_command() {
        assert_eq!(resume_session_id(&format!("clear && claude -r {SID}")), Some(SID));
        assert_eq!(resume_session_id(&format!("claude -r {SID}")), Some(SID));
        // codex 的会话不在 ~/.claude/projects 里，不管
        assert_eq!(resume_session_id(&format!("codex resume {SID}")), None);
        assert_eq!(resume_session_id("clear && claude"), None);
        assert_eq!(resume_session_id("zsh"), None);
        // id 会拼进文件路径：不是 uuid 形状的一律不认，杜绝 ../ 之类
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
        // 在别的目录被恢复 → 改回起始目录
        assert_eq!(corrected_resume_cwd("/Users/me/other-project", Some(home.clone())), Some(home.clone()));
        // 本来就在起始目录 → 不改（不通知前端）
        assert_eq!(corrected_resume_cwd(&home, Some(home.clone())), None);
        // 起始目录已经不在了（目录被移走、用户用「指到新位置」恢复过）→ 不改，
        // 否则会和 cwd-missing 恢复流程互相打架：改回去 → 被拒 → relink 到新目录 → 又被改回去
        assert_eq!(corrected_resume_cwd("/Users/me/new-place", Some("/definitely/not/a/path/makit-home".into())), None);
        // 找不到会话记录 → 不改
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
