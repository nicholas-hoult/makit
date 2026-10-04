//! PTY 启动：cwd 校正 / cwd 闸 / 目录降级、环境变量、shell 集成（ZDOTDIR → OSC 7），以及 OSC 7 的字节流扫描。
//! 对应 Tauri 版 `src-tauri/src/pty.rs` 的 `pty_spawn`；纯逻辑都在 core 的 `pty_prep`，这里只负责拼起来。
//!
//! 对齐清单不变量 17 / 20：
//! - 登录 shell（`$SHELL -l`，默认 /bin/zsh）；TERM=xterm-256color，LANG=LC_ALL=en_US.UTF-8；
//!   `MAKIT_PTY_ID=<标签 id>`（关标签按它扫逃逸进程，#3）；zsh 走 ZDOTDIR 集成，chpwd / precmd 两处发 OSC 7。
//! - `MAKIT_SESSION_ID` 用 `resume_session_id` 解析（Tauri 版按「以 claude -r 开头」去匹配，
//!   对 `clear && claude -r <id>` 永远匹配不上，#223 同类 bug —— 这里不重蹈）。
//! - resume 标签在会话起始目录启动（#190）；目录没了的 resume 标签必须拒绝启动（`cwd-missing:`，#173），
//!   其余标签逐级降级（祖先 → home → `/`）并写一行黄字。
//!
//! 为什么单独测：环境变量和 cwd 决策错了，表现是「resume 找不到会话」「关标签杀不干净」「cd 之后链接指错目录」，
//! 都要真起一个 shell 才看得出来。

use crate::ts;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};
use makit_core::pty_prep;
use polling::{Event, PollMode, Poller};

use super::osc::Osc7Scanner;

/// spawn 前的决定
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPlan {
    /// 实际在哪个目录起 shell（已展开 `~`、已降级）
    pub cwd: String,
    /// resume 标签被校正到了会话起始目录（#190）：调用方据此改写持久化的标签记录
    pub corrected: Option<String>,
    /// 目录不存在、降级了：原目录（写黄字用）
    pub fell_back_from: Option<String>,
}

/// cwd 校正 → cwd 闸 → 降级。`Err` 是 `cwd-missing:<目录>`（交给恢复对话框，不写红字）。
/// `allow_fallback`：`Some(false)` = 不许降级（resume 标签），`None` = 老行为（降级），`Some(true)` = 用户刚确认过（重试）
pub fn plan_spawn(init_command: Option<&str>, requested_cwd: &str, allow_fallback: Option<bool>) -> Result<SpawnPlan, String> {
    let mut cwd = if requested_cwd.trim().is_empty() { "~".to_string() } else { requested_cwd.to_string() };
    let corrected = pty_prep::resume_cwd_correction(init_command, &cwd);
    if let Some(c) = &corrected {
        cwd = c.clone();
    }
    if pty_prep::must_refuse_cwd(&cwd, allow_fallback) {
        return Err(format!("cwd-missing:{cwd}"));
    }
    let expanded = pty_prep::expand_tilde(&cwd);
    let resolved = pty_prep::resolve_existing_cwd(&cwd);
    let fell_back_from = (resolved != expanded).then_some(expanded);
    Ok(SpawnPlan { cwd: resolved, corrected, fell_back_from })
}

/// 子进程环境变量（在继承的环境之上覆盖）
pub fn build_env(pty_id: Option<&str>, init_command: Option<&str>, shell: &str, home: Option<&Path>) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    // GUI 启动的子进程可能没有 locale，中文会乱码
    env.insert("LANG".into(), "en_US.UTF-8".into());
    env.insert("LC_ALL".into(), "en_US.UTF-8".into());
    if let Some(id) = pty_id {
        env.insert("MAKIT_PTY_ID".into(), id.into());
    }
    if let Some(sid) = init_command.and_then(pty_prep::resume_session_id) {
        env.insert("MAKIT_SESSION_ID".into(), sid.into());
    }
    if pty_prep::is_zsh(shell) {
        if let Some(home) = home {
            let dir = pty_prep::prepare_zsh_integration(home);
            env.insert("ZDOTDIR".into(), dir.to_string_lossy().into_owned());
        }
    }
    env
}

pub fn user_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into())
}

/// 降级时写进终端的那行黄字（不经过 PTY，不会被 shell 当命令执行）
pub fn fallback_notice(from: &str, to: &str) -> String {
    format!("\x1b[33m{}\x1b[0m\r\n", ts!("terminal.cwd_missing", from = from, to = to))
}

/// 启动目录不存在、这个标签不许降级（resume 标签）时写进面板的说明（同 Tauri App.tsx openRecoverDialog）。
/// 恢复对话框可能被关掉，或者同一时间已经开着别的面板的对话框（一次只弹一个）—— 这一行必须写，
/// 不然那块面板就是一块没有任何线索的死屏。不写成红字：这不是故障，会话记录没丢，只是丢了「在哪启动」这把钥匙
pub fn cwd_missing_notice(cwd: &str) -> String {
    format!("\r\n\x1b[33m{}\x1b[0m\r\n\x1b[2m{}\x1b[0m", ts!("terminal.cwd_missing_title", cwd = cwd), ts!("terminal.cwd_missing_hint"))
}

/// 包在 alacritty 的 tty 外面：读出来的字节先过一遍 OSC 7 扫描器（alacritty 的解析器不处理 OSC 7），
/// 原样交给 alacritty。读写 / 注册都转给里面的 Pty
pub struct ScanningPty {
    inner: tty::Pty,
    reader: ScanReader,
}

pub struct ScanReader {
    file: File,
    scanner: Osc7Scanner,
    cwd: Arc<Mutex<String>>,
}

impl Read for ScanReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        if let Some(last) = self.scanner.feed(&buf[..n]).pop() {
            if let Ok(mut c) = self.cwd.lock() {
                *c = last;
            }
        }
        Ok(n)
    }
}

impl ScanningPty {
    /// `cwd`：OSC 7 报上来的目录写到这里（UI 线程读，给链接解析用）
    pub fn new(inner: tty::Pty, cwd: Arc<Mutex<String>>) -> io::Result<Self> {
        let file = inner.file().try_clone()?;
        Ok(Self { inner, reader: ScanReader { file, scanner: Osc7Scanner::default(), cwd } })
    }
    pub fn child_pid(&self) -> u32 {
        self.inner.child().id()
    }
}

impl EventedReadWrite for ScanningPty {
    type Reader = ScanReader;
    type Writer = File;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        unsafe { self.inner.register(poll, interest, mode) }
    }
    fn reregister(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        self.inner.reregister(poll, interest, mode)
    }
    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }
    fn reader(&mut self) -> &mut ScanReader {
        &mut self.reader
    }
    fn writer(&mut self) -> &mut File {
        self.inner.writer()
    }
}

impl EventedPty for ScanningPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for ScanningPty {
    fn on_resize(&mut self, ws: WindowSize) {
        self.inner.on_resize(ws)
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "0b8a4b0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b";

    #[test]
    fn env_has_marker_locale_and_session_id() {
        // 恢复命令只许 workspace::model 拼（那边有测试盯着），这里直接用它
        let resume = crate::workspace::model::resume_init_command(UUID, None);
        let env = build_env(Some("t_abc"), Some(&resume), "/bin/bash", None);
        assert_eq!(env["TERM"], "xterm-256color");
        assert_eq!(env["LANG"], "en_US.UTF-8");
        assert_eq!(env["LC_ALL"], "en_US.UTF-8");
        assert_eq!(env["MAKIT_PTY_ID"], "t_abc");
        // #223 同类 bug：带 `clear && ` 前缀的 resume 命令也要注入（Tauri 版的 strip_prefix 永远匹配不上）
        assert_eq!(env["MAKIT_SESSION_ID"], UUID);
        assert!(!env.contains_key("ZDOTDIR"), "bash 不做集成");
    }

    #[test]
    fn session_id_only_for_uuid_shaped_ids() {
        let bad = crate::workspace::model::resume_init_command("../../etc", None);
        let env = build_env(None, Some(&bad), "/bin/bash", None);
        assert!(!env.contains_key("MAKIT_SESSION_ID"), "id 会拼进路径，只认 uuid 形状");
        assert!(!env.contains_key("MAKIT_PTY_ID"));
        let env = build_env(None, None, "/bin/bash", None);
        assert!(!env.contains_key("MAKIT_SESSION_ID"));
    }

    #[test]
    fn zsh_gets_zdotdir_integration_emitting_osc7() {
        let home = std::env::temp_dir().join(format!("makit-a-zdot-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let env = build_env(None, None, "/bin/zsh", Some(&home));
        let dir = PathBuf::from(&env["ZDOTDIR"]);
        let rc = std::fs::read_to_string(dir.join(".zshrc")).unwrap();
        assert!(rc.contains("chpwd_functions+=(_makit_emit_cwd)") && rc.contains("precmd_functions+=(_makit_emit_cwd)"));
        assert!(rc.contains("]7;file://"), "发的是 OSC 7");
        assert!(rc.contains(&home.join(".zshrc").display().to_string()), "source 用户自己的 .zshrc");
        assert!(dir.join(".zprofile").exists() && dir.join(".zshenv").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn cwd_gate_and_fallback() {
        let base = std::env::temp_dir().join(format!("makit-a-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let gone = base.join("gone").join("deeper");
        let gone_s = gone.to_string_lossy().into_owned();
        // resume 标签（不许降级）：拒绝，前缀 cwd-missing:
        assert_eq!(plan_spawn(None, &gone_s, Some(false)), Err(format!("cwd-missing:{gone_s}")));
        // 普通标签：逐级降级到存在的祖先，并记下原目录（写黄字）
        let p = plan_spawn(None, &gone_s, None).unwrap();
        assert_eq!(p.cwd, base.to_string_lossy());
        assert_eq!(p.fell_back_from.as_deref(), Some(gone_s.as_str()));
        // 用户在恢复对话框里确认过（重试）：允许降级
        assert!(plan_spawn(None, &gone_s, Some(true)).is_ok());
        // 目录在：原样，不写黄字
        let ok = plan_spawn(None, &base.to_string_lossy(), Some(false)).unwrap();
        assert_eq!((ok.cwd.as_str(), ok.fell_back_from.as_deref(), ok.corrected.as_deref()), (&*base.to_string_lossy(), None, None));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cwd_missing_notice_is_yellow_then_dim_no_red() {
        let n = cwd_missing_notice("/gone");
        assert!(n.starts_with("\r\n\x1b[33m原启动目录已不存在: /gone"), "{n}");
        assert!(n.contains("会话记录没丢"), "第二段要说清楚不是故障");
        assert!(!n.contains("\x1b[31m"), "不是红字：不能让用户以为坏了");
    }

    #[test]
    fn notice_is_yellow_and_bypasses_shell() {
        let n = fallback_notice("/a", "/b");
        assert!(n.starts_with("\x1b[33m") && n.contains("原目录不存在: /a → 已切换到: /b"));
    }
}
