use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{Emitter, State, Window};

use crate::pty_prep::{corrected_resume_cwd, expand_tilde, must_refuse_cwd, resolve_existing_cwd, resume_session_id, session_home_cwd};
#[cfg(unix)]
use crate::process::{kill_by_env_marker, kill_pty_by_pid, kill_tree};


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

    // 读和发拆成两个线程（#222）：macOS 的 PTY 在程序逐行写时一次只读到几十字节，
    // 以前每读一次就 emit 一个事件 —— `seq 1 300000` 发了 7.8 万个，界面冻住 9 秒。
    // 读线程只管把字节塞进通道；发送线程按 pty_batch 的节奏合并（空闲后第一块立刻发，
    // 连续输出每 8ms 最多一次），并负责只发完整的 UTF-8 字符。
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 32768];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        // tx 在这里被丢掉 → 发送线程收到断开，把剩下的发完再发 exit
    });

    let id_for_thread = id.clone();
    let win = window.clone();
    std::thread::spawn(move || {
        use crate::pty_batch::{Batcher, Next, EMIT_INTERVAL, MAX_BATCH_BYTES};
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::Instant;
        let event = format!("pty:data:{}", id_for_thread);
        let mut batch = Batcher::new(EMIT_INTERVAL, MAX_BATCH_BYTES);
        let mut open = true;
        while open {
            // 通道里已经到了的先全部收进来
            while let Ok(chunk) = rx.try_recv() {
                batch.push(&chunk);
            }
            match batch.next(Instant::now()) {
                Next::EmitNow => {
                    let text = batch.take(Instant::now());
                    if win.emit(&event, text).is_err() {
                        return;
                    }
                }
                Next::WaitUntil(t) => {
                    let wait = t.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(chunk) => batch.push(&chunk),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => open = false,
                    }
                }
                Next::Idle => match rx.recv() {
                    Ok(chunk) => batch.push(&chunk),
                    Err(_) => open = false,
                },
            }
        }
        let rest = batch.finish();
        if !rest.is_empty() {
            let _ = win.emit(&event, rest);
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

/// 关一个 pty 的唯一入口：进程树 + 逃逸进程，两步缺一不可（#3）。
///
/// `kill_tree` 只保证杀掉「还够得着」的四种形态；reparent 到 pid 1 的那两种
/// （orphan_setsid / double_fork —— claude 起的工具进程正是这个形状）PPID 链断了、
/// 进程组也换了，`killpg` 和 `pgrep -P` 都打不到，只能靠 `MAKIT_PTY_ID` 环境变量认出来。
/// 六种形态各是什么、谁能被谁覆盖，见下面的 `kill_matrix_tests`（#142）。
#[cfg(unix)]
fn kill_pty(child: &dyn portable_pty::Child, pty_id: &str) {
    match child.process_id() {
        Some(pid) => kill_pty_by_pid(pid, pty_id),
        // 子进程已经被收走了也照样扫一遍：逃逸进程活得比它爹久，这正是要兜的情况
        None => kill_by_env_marker(&[pty_id]),
    }
}


#[cfg(not(unix))]
fn kill_pty(_child: &dyn portable_pty::Child, _pty_id: &str) {}


#[tauri::command(async)]
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
        kill_pty(handle.child.as_ref(), &id);
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
            #[cfg(unix)]
            if let Some(pid) = handle.child.process_id() {
                kill_tree(pid);
            }
            let _ = handle.child.kill();
        }
    }
    // 逃逸进程一次 ps 扫完所有 id（不走 `kill_pty`：那是一个 id 一次 ps）
    #[cfg(unix)]
    kill_by_env_marker(&ids.iter().map(|s| s.as_str()).collect::<Vec<_>>());
}
