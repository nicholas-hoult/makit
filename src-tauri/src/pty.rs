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
) -> Result<(), String> {
    {
        let map = state.inner.lock().unwrap();
        if map.contains_key(&id) {
            return Ok(());
        }
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
    cmd.env("CCS_PTY_ID", &id);

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
                 [ -f {user_rc} ] && source {user_rc}\n_ccs_emit_cwd() {{ printf '\\033]7;file://%s%s\\033\\\\' \"$HOST\" \"$PWD\" }}\n\
                 typeset -ga chpwd_functions precmd_functions\n\
                 chpwd_functions+=(_ccs_emit_cwd)\n\
                 precmd_functions+=(_ccs_emit_cwd)\n\
                 _ccs_emit_cwd\n",
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
                cmd.env("CCS_SESSION_ID", sid);
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
    Ok(())
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
        // 1) 先递归找所有后代进程（在发 signal 前收集，避免进程退出后丢失）
        let descendants = find_descendants(pid);

        unsafe {
            // 2) killpg 杀主进程组
            libc::killpg(pid as libc::pid_t, libc::SIGHUP);
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);

            // 3) 逐个杀后代进程（覆盖 setsid 逃逸的）
            for dpid in &descendants {
                libc::kill(*dpid as libc::pid_t, libc::SIGKILL);
            }
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
    // 兜底：用 CCS_PTY_ID 环境变量找逃逸进程（setsid 后 PPID=1 的）
    #[cfg(unix)]
    kill_by_env_marker(&ids);
}

#[cfg(unix)]
fn kill_by_env_marker(pty_ids: &[String]) {
    use std::process::Command;
    // macOS: ps -xeww 输出包含环境变量（追加在 command 后面）
    // 一次性拿全部进程的完整 command+env，逐行 grep CCS_PTY_ID
    if pty_ids.is_empty() { return; }
    let my_pid = std::process::id();
    if let Ok(output) = Command::new("ps").args(["-xeww", "-o", "pid,command"]).output() {
        if !output.status.success() { return; }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let trimmed = line.trim();
            // 解析 PID（行首数字）
            let pid_str = trimmed.split_whitespace().next().unwrap_or("");
            let pid: u32 = match pid_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if pid == my_pid || pid <= 1 { continue; }
            // 检查这行是否包含我们的 CCS_PTY_ID
            for pty_id in pty_ids {
                if trimmed.contains(&format!("CCS_PTY_ID={}", pty_id)) {
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL); }
                    break;
                }
            }
        }
    }
}
