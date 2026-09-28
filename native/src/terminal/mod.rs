//! 终端：alacritty_terminal 做 PTY + VT 解析，GPUI 画网格（#221 原型 → #226 对齐 Tauri 版）。
//!
//! 结构照 Zed 的 `crates/terminal` + `crates/terminal_view`（gpui 0.2.2 对应的 69e2130）：
//! - PTY 和解析在 alacritty 的 EventLoop 线程里跑，解析结果写进 `Term`（FairMutex 保护）。
//!   它读多少解析多少：解析跟不上时不再从 PTY 读，内核缓冲满了 `yes` 这类程序就阻塞在 write 上 ——
//!   天然有背压（Tauri 版 #229 要专门补的那件事），内存只和回滚行数有关。
//! - 它通过 `Listener` 把事件（有新内容、标题、要回写 PTY 的应答……）发到 GPUI 这边；
//!   GPUI 这边收到 Wakeup 只 `notify`，下一帧 prepaint 时锁一下 Term、把可见区域拍成
//!   背景块 + 文字段，paint 时画出来。输出再快，一帧也只画一次。
//!
//! 子模块：`element`（绘制 + 鼠标挂钩）、`input`（键盘 / 输入法 / 鼠标 / 滚轮）、`search`（⌘F），
//! 纯逻辑（都带测试）：`size` `links` `wheel` `scrollbar` `contrast` `osc` `pty` `drop_paths` `zoom` `keys` `palette` `segment`。
//!
//! 对外接口（给工作区 / 浮层包用）：
//! - `TerminalView::new(spec, …)`；事件 `TerminalEvent`（标题 / 退出）、`TerminalSpawnEvent`（cwd 校正 / cwd-missing）、
//!   `SearchResults`（⌘F 计数）
//! - `shutdown()`（关标签，杀干净）、`retry_spawn(cwd)`（恢复对话框确认后重试）、`notice(text)`（往屏幕写提示）
//! - `fit_now()`（切标签 / 最大化后立即行列都到位）、`flush_resize()`（拖分割线松手）
//! - `insert_text(text)` / `insert_paths(paths)`（粘贴语义：按程序状态决定括号粘贴）
//! - `selection_text()`、`search_*`、`cwd()`

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, Rgb};
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::{
    actions, div, prelude::*, px, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, MouseButton,
    Pixels, SharedString, Window,
};

mod element;
mod input;
mod search;

pub mod contrast;
pub mod drop_paths;
pub mod grid;
pub mod keys;
pub mod links;
pub mod osc;
pub mod palette;
pub mod pty;
pub mod scrollbar;
pub mod segment;
pub mod size;
pub mod wheel;
pub mod zoom;

#[cfg(test)]
mod flood_test;

use element::TerminalElement;
use links::ExistCache;
use palette::TermColors;
use scrollbar::Activity;
use search::SearchState;
use size::{clamp_size, is_visible_area, plan_resize, propose_geometry, ColsFollower, Follow, PtyLedger, Size, COLS_FOLLOW_MS};

use crate::actions::terminal as act;
use crate::perf;
use crate::theme::ActiveTheme;

actions!(terminal, [Copy, Paste, ScrollPageUp, ScrollPageDown]);

/// 等宽字体栈，同 TS 版 `fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace, …"`：
/// ui-monospace 在 macOS 上就是 SF Mono；按顺序取第一个能加载的
pub const FONT_STACK: &[&str] = &["SF Mono", ".AppleSystemUIFontMonospaced", "SFMono-Regular", "Menlo"];
/// 中日韩回退，同 TS 版字体栈后半段（顺序照抄：Apple SD Gothic Neo 在前）
pub const CJK_FALLBACKS: &[&str] = &["Apple SD Gothic Neo", "Hiragino Sans GB", "PingFang SC"];
/// `.xterm-inner` 的 inset：top 6 / right 10 / bottom 16 / left 10（App.css；用 inset 不用 padding，
/// padding 会被算进尺寸导致行数溢出、选区坐标错位）
pub const INSET_TOP: f32 = 6.0;
pub const INSET_RIGHT: f32 = 10.0;
pub const INSET_BOTTOM: f32 = 16.0;
pub const INSET_LEFT: f32 = 10.0;
/// 光标闪烁间隔（xterm CursorBlinkStateManager 的 600ms）
pub const CURSOR_BLINK_MS: u64 = 600;
/// 大段输出时搜索结果最多隔多久重算一次
const SEARCH_REFRESH_MS: u64 = 250;

#[derive(Clone)]
pub struct Listener(UnboundedSender<AlacEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

/// alacritty 要的尺寸接口
#[derive(Clone, Copy)]
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// 终端标签发给外面的事件
pub enum TerminalEvent {
    TitleChanged,
    Exited,
}

/// 启动相关的事件（另开一个事件类型，不动 `TerminalEvent`，免得改工作区的 match）
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalSpawnEvent {
    /// resume 标签被改到会话起始目录启动（#190）：工作区据此改写持久化的标签 cwd
    CwdCorrected(String),
    /// resume 标签的目录不在了，拒绝启动（#173）：交给恢复对话框（不写红字）；确认后调 `retry_spawn`
    CwdMissing(String),
}

/// ⌘F 结果变了（给搜索条的「3/17」，文本用 `zoom::format_search_count` 拼）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchResults {
    /// 从 0 起；超过高亮上限（1000）时为 -1
    pub index: i64,
    pub count: usize,
}

/// 用户在这个终端里打字 / 粘贴了（给 E 通知包：= 看到了这个会话，集成时转给 `Notifier::mark_session_read(session_id)`）。
/// 连续打字只在距上一次超过 1 秒时发一次
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInput {
    pub session_id: Option<String>,
}

impl EventEmitter<UserInput> for TerminalView {}
impl EventEmitter<TerminalEvent> for TerminalView {}
impl EventEmitter<TerminalSpawnEvent> for TerminalView {}
impl EventEmitter<SearchResults> for TerminalView {}

/// 怎么启动
#[derive(Clone)]
pub struct SpawnSpec {
    /// 标签 id，注入成子进程环境变量 `MAKIT_PTY_ID`（运行状态绑定、关标签杀逃逸进程都认它）
    pub pty_id: Option<String>,
    pub cwd: Option<String>,
    /// 启动后像打字一样写进 shell 的命令（同 WebView 版的 initCommand：`clear && claude -r <id>`）
    pub init_command: Option<String>,
    pub session_id: Option<String>,
    /// perf.log 里的 tab 类型：resume / new / shell。resume 标签目录不在时拒绝启动（cwd 闸）
    pub kind: &'static str,
    /// 这个终端是被哪次点击触发的（打开终端的时间线从这里起算）
    pub clicked_at: Instant,
}

/// 大量输出时的帧统计：一段输出（Wakeup 之间间隔不超过 500ms）结束后写一条 perf.log
struct Burst {
    started: Instant,
    last_wakeup: Instant,
    frames: u32,
    last_frame: Option<Instant>,
    max_frame_ms: f64,
    wakeups: u32,
}

struct PtyHandle {
    notifier: Notifier,
    pid: u32,
}

/// ⌘ 悬停中的链接（画下划线 + 手型光标，⌘+点击打开）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HoverLink {
    /// 视口里的 (行, 起始列, 结束列含)
    pub segments: Vec<(usize, usize, usize)>,
    pub target: LinkTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LinkTarget {
    Url(String),
    /// 已经按 cwd 解析过、去掉了 `:行:列`
    Path(String),
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    events_tx: UnboundedSender<AlacEvent>,
    pty: Option<PtyHandle>,
    spec: SpawnSpec,
    /// cwd 闸：resume 标签 `Some(false)`；恢复对话框确认后重试 `Some(true)`；其余 `None`（降级）
    allow_fallback: Option<bool>,
    /// 启动被拒 / 失败：等 `retry_spawn`，布局时不再自动 spawn
    spawn_blocked: bool,
    /// OSC 7 报上来的当前目录（PTY 读线程写，链接解析读）
    cwd: Arc<Mutex<String>>,
    focus: FocusHandle,
    pub title: String,
    pub exited: bool,
    shut_down: bool,
    // ── 几何（#156 / #203）──
    font_size: f32,
    cell_w: Pixels,
    line_h: Pixels,
    /// 终端网格当前的尺寸（和 PTY 记账本必须一致）
    size: Size,
    /// 布局量出来想要的尺寸（列数可能还在节流中没跟上）
    desired: Size,
    ledger: PtyLedger,
    follower: ColsFollower,
    follow_timer: bool,
    cols_due: bool,
    /// 下一次布局行列都立即到位（切标签、字号、最大化、松手）
    immediate: bool,
    last_layout: Option<Instant>,
    /// 上次 layout 量到的区域（调试导出用：核对「区域 ÷ 格宽」和网格 / PTY 列数对不对得上）
    laid_out: (f32, f32),
    // ── 输入 ──
    /// PTY 就绪前的键入，就绪后按顺序写进去（retry 复用同一份）
    pending_input: Vec<u8>,
    marked_text: Option<String>,
    // ── 滚动 ──
    precise_pending: f32,
    discrete_pending: f32,
    scroll_activity: Activity,
    gutter_hover: bool,
    /// 拖滑块中：按下时指针在滑块内的偏移
    slider_drag: Option<f32>,
    slider_hover: bool,
    last_display_offset: usize,
    // ── 鼠标 ──
    selecting: bool,
    mouse_btn_down: Option<keys::MouseBtn>,
    last_report_cell: Option<(usize, usize)>,
    last_mouse: Option<gpui::Point<Pixels>>,
    // ── 链接 ──
    hover: Option<HoverLink>,
    exist_cache: ExistCache,
    // ── 光标 ──
    blink_on: bool,
    blink_epoch: u64,
    // ── 搜索 ──
    search: SearchState,
    colors: TermColors,
    epoch: Instant,
    // ── 性能埋点 ──
    created_at: Instant,
    first_output_at: Option<Instant>,
    first_paint_logged: bool,
    burst: Option<Burst>,
    last_dump: Option<Instant>,
    last_user_input: Option<Instant>,
}

impl TerminalView {
    pub fn new(spec: SpawnSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let created_at = Instant::now();
        let (tx, mut rx) = unbounded();
        let size = Size { cols: 80, rows: 24 };
        let config = Config {
            scrolling_history: size::SCROLLBACK,
            // 双击选词的分隔符照 对标终端（#214）
            semantic_escape_chars: links::WORD_SEPARATORS.to_string(),
            // xterm `cursorBlink: true`
            default_cursor_style: CursorStyle { shape: CursorShape::Block, blinking: true },
            ..Config::default()
        };
        let term_size = TermSize { cols: size.cols as usize, rows: size.rows as usize };
        let term = Arc::new(FairMutex::new(Term::new(config, &term_size, Listener(tx.clone()))));

        // 事件泵：把一批事件合起来处理，一批只 notify 一次
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(ev) = rx.try_recv() {
                    batch.push(ev);
                }
                let alive = this.update(cx, |view, cx| {
                    for ev in batch {
                        view.handle_event(ev, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        // 退出应用时同步杀干净（关窗 / ⌘Q 都走这里）；后台线程在进程退出时会被腰斩，所以这里阻塞着做
        cx.on_app_quit(|this, _| {
            this.kill_blocking();
            async {}
        })
        .detach();

        let _ = window;
        let allow_fallback = if spec.kind == "resume" { Some(false) } else { None };
        let initial_cwd = spec.cwd.clone().unwrap_or_default();
        Self {
            term,
            events_tx: tx,
            pty: None,
            spec,
            allow_fallback,
            spawn_blocked: false,
            cwd: Arc::new(Mutex::new(initial_cwd)),
            // 不在这里抢焦点：恢复布局时一次会建好几个终端，焦点由工作区视图给当前标签
            focus: cx.focus_handle(),
            title: String::new(),
            exited: false,
            shut_down: false,
            font_size: zoom::DEFAULT_FONT_SIZE,
            cell_w: px(7.5),
            line_h: px(16.0),
            size,
            desired: size,
            ledger: PtyLedger::default(),
            follower: ColsFollower::new(COLS_FOLLOW_MS),
            follow_timer: false,
            cols_due: false,
            immediate: true,
            last_layout: None,
            laid_out: (0.0, 0.0),
            pending_input: Vec::new(),
            marked_text: None,
            precise_pending: 0.0,
            discrete_pending: 0.0,
            scroll_activity: Activity::default(),
            gutter_hover: false,
            slider_drag: None,
            slider_hover: false,
            last_display_offset: 0,
            selecting: false,
            mouse_btn_down: None,
            last_report_cell: None,
            last_mouse: None,
            hover: None,
            exist_cache: ExistCache::default(),
            blink_on: true,
            blink_epoch: 0,
            search: SearchState::default(),
            colors: TermColors::default(),
            epoch: Instant::now(),
            created_at,
            first_output_at: None,
            first_paint_logged: false,
            burst: None,
            last_dump: None,
            last_user_input: None,
        }
    }

    fn now_ms(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() * 1000.0
    }

    // ───────────────────────── 启动 / 关闭 ─────────────────────────

    /// 第一次布局量出真实尺寸后才 spawn（容器没尺寸前不 spawn，同 Tauri 版）：
    /// cwd 校正 → cwd 闸 → 降级 → 起 shell（带 OSC 7 扫描）→ 按 spawn 尺寸记账 → 键入 initCommand → 补写 pending 输入
    fn spawn(&mut self, size: Size, cx: &mut Context<Self>) {
        if self.pty.is_some() || self.spawn_blocked || self.shut_down {
            return;
        }
        let init = self.spec.init_command.clone();
        let requested = self.spec.cwd.clone().unwrap_or_default();
        let plan = match pty::plan_spawn(init.as_deref(), &requested, self.allow_fallback) {
            Ok(p) => p,
            Err(e) => {
                self.spawn_blocked = true;
                if let Some(dir) = e.strip_prefix("cwd-missing:") {
                    cx.emit(TerminalSpawnEvent::CwdMissing(dir.to_string()));
                } else {
                    self.write_local(&format!("\x1b[31mPTY 启动失败: {e}\x1b[0m\r\n"));
                }
                return;
            }
        };
        if let Some(c) = &plan.corrected {
            self.spec.cwd = Some(c.clone());
            cx.emit(TerminalSpawnEvent::CwdCorrected(c.clone()));
        }
        if let Some(from) = &plan.fell_back_from {
            self.write_local(&pty::fallback_notice(from, &plan.cwd));
        }
        if let Ok(mut c) = self.cwd.lock() {
            *c = plan.cwd.clone();
        }
        let shell = pty::user_shell();
        let env = pty::build_env(self.spec.pty_id.as_deref(), init.as_deref(), &shell, pty::home_dir().as_deref());
        let options = tty::Options {
            // login shell：读 .zprofile 拿到完整 PATH（brew / cargo / nvm）
            shell: Some(tty::Shell::new(shell, vec!["-l".to_string()])),
            working_directory: Some(PathBuf::from(&plan.cwd)),
            drain_on_exit: true,
            env,
        };
        let ws = self.window_size_for(size);
        let result = tty::new(&options, ws, 0).and_then(|p| pty::ScanningPty::new(p, self.cwd.clone())).and_then(|p| {
            let pid = p.child_pid();
            EventLoop::new(self.term.clone(), Listener(self.events_tx.clone()), p, true, false).map(|el| (el, pid))
        });
        match result {
            Ok((event_loop, pid)) => {
                let notifier = Notifier(event_loop.channel());
                let _io_thread = event_loop.spawn();
                self.ledger.spawned(size);
                if let Some(cmd) = init.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                    notifier.notify(format!("{cmd}\r").into_bytes());
                }
                let pending = std::mem::take(&mut self.pending_input);
                if !pending.is_empty() {
                    notifier.notify(pending);
                }
                self.pty = Some(PtyHandle { notifier, pid });
                self.start_blink(cx);
                if std::env::var_os("MAKIT_TERM_FLOOD").is_some() {
                    self.start_flood_probe(cx);
                }
            }
            Err(e) => {
                self.spawn_blocked = true;
                self.write_local(&format!("\x1b[31mPTY 启动失败: {e}\x1b[0m\r\n"));
            }
        }
    }

    /// 恢复对话框确认之后重试（这一刻的目录用户刚确认过，允许降级）。复用同一份 pending 输入和终端
    pub fn retry_spawn(&mut self, cwd: Option<String>, cx: &mut Context<Self>) {
        if self.pty.is_some() || self.shut_down {
            return;
        }
        if let Some(c) = cwd {
            self.spec.cwd = Some(c);
        }
        self.allow_fallback = Some(true);
        self.spawn_blocked = false;
        self.immediate = true;
        cx.notify();
    }

    /// 往屏幕写一行提示（不经过 PTY），比如「启动被拒、对话框被关」之后，免得留下一块没有线索的死面板
    pub fn notice(&mut self, text: &str, cx: &mut Context<Self>) {
        self.write_local(&format!("\r\n\x1b[2m{text}\x1b[0m\r\n"));
        cx.notify();
    }

    /// 直接喂给解析器（黄字 / 红字 / 提示），不进 PTY，不会被 shell 当命令执行
    fn write_local(&mut self, s: &str) {
        let mut term = self.term.lock();
        let mut p: Processor = Processor::new();
        p.advance(&mut *term, s.as_bytes());
    }

    /// 关标签：杀干净（对齐清单不变量 21）。进程组 SIGHUP + SIGKILL、后代逐个 SIGKILL，再按 `MAKIT_PTY_ID`
    /// 用 `ps -xEww` 扫逃逸进程（core 的 `kill_pty_by_pid`）。ps / pgrep 要几十毫秒，放后台线程，不卡界面；
    /// 退出应用时走 `kill_blocking`。
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        if let Some(p) = self.pty.take() {
            let _ = p.notifier.0.send(Msg::Shutdown);
            let id = self.spec.pty_id.clone().unwrap_or_default();
            let pid = p.pid;
            std::thread::spawn(move || makit_core::process::kill_pty_by_pid(pid, &id));
        }
    }

    fn kill_blocking(&mut self) {
        self.shut_down = true;
        if let Some(p) = self.pty.take() {
            let _ = p.notifier.0.send(Msg::Shutdown);
            makit_core::process::kill_pty_by_pid(p.pid, &self.spec.pty_id.clone().unwrap_or_default());
        }
    }

    /// 子进程 pid（测试 / 取证用）
    pub fn child_pid(&self) -> Option<u32> {
        self.pty.as_ref().map(|p| p.pid)
    }

    /// OSC 7 跟踪的当前目录
    pub fn cwd(&self) -> String {
        self.cwd.lock().map(|c| c.clone()).unwrap_or_default()
    }

    // ───────────────────────── 事件 ─────────────────────────

    fn handle_event(&mut self, ev: AlacEvent, cx: &mut Context<Self>) {
        match ev {
            AlacEvent::Wakeup => {
                let now = Instant::now();
                if self.first_output_at.is_none() {
                    self.first_output_at = Some(now);
                }
                self.track_burst(now, cx);
                if self.search.active() {
                    self.search_schedule_refresh(cx);
                }
                cx.notify();
            }
            AlacEvent::Title(t) => {
                self.title = t;
                cx.emit(TerminalEvent::TitleChanged);
                cx.notify();
            }
            AlacEvent::ResetTitle => {
                self.title.clear();
                cx.emit(TerminalEvent::TitleChanged);
                cx.notify();
            }
            AlacEvent::PtyWrite(s) => self.write_pty(s.into_bytes()),
            AlacEvent::ColorRequest(index, format) => {
                let c = self.colors_for_request(index);
                let rgb = Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 };
                let reply = format(rgb);
                self.write_pty(reply.into_bytes());
            }
            AlacEvent::TextAreaSizeRequest(format) => {
                let ws = self.window_size_for(self.size);
                self.write_pty(format(ws).into_bytes());
            }
            AlacEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            AlacEvent::ClipboardLoad(_, format) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.write_pty(format(&text).into_bytes());
            }
            AlacEvent::ChildExit(_) | AlacEvent::Exit => {
                if !self.exited {
                    self.exited = true;
                    // 同 Tauri 版：写一行 dim 的「[进程已退出]」
                    self.write_local("\r\n\x1b[2m[进程已退出]\x1b[0m\r\n");
                    cx.emit(TerminalEvent::Exited);
                    cx.notify();
                }
            }
            AlacEvent::Bell | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => {}
        }
    }

    fn colors_for_request(&self, index: usize) -> u32 {
        let overrides = self.term.lock().colors()[index];
        if let Some(c) = overrides {
            return ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32;
        }
        match index {
            0..=255 => self.colors.indexed(index as u8),
            256 => self.colors.fg,
            257 => self.colors.bg,
            258 => self.colors.cursor,
            _ => self.colors.fg,
        }
    }

    fn track_burst(&mut self, now: Instant, cx: &mut Context<Self>) {
        match &mut self.burst {
            Some(b) if now.duration_since(b.last_wakeup).as_millis() < 500 => {
                b.last_wakeup = now;
                b.wakeups += 1;
            }
            _ => {
                self.flush_burst();
                self.burst =
                    Some(Burst { started: now, last_wakeup: now, frames: 0, last_frame: None, max_frame_ms: 0.0, wakeups: 1 });
                // 输出停下来 600ms 后把这一段写掉
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_millis(600)).await;
                    let done = this
                        .update(cx, |v, _| {
                            let idle = v.burst.as_ref().map(|b| b.last_wakeup.elapsed().as_millis() >= 500).unwrap_or(true);
                            if idle {
                                v.flush_burst();
                            }
                            idle
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                })
                .detach();
            }
        }
    }

    fn flush_burst(&mut self) {
        let Some(b) = self.burst.take() else { return };
        let ms = b.last_wakeup.duration_since(b.started).as_secs_f64() * 1000.0;
        // 只记像样的大段输出，敲一个字母那种不记
        if ms < 200.0 {
            return;
        }
        perf::record(serde_json::json!({
            "kind": "terminal-burst",
            "app": "gpui",
            "ms": ms.round(),
            "frames": b.frames,
            "fps": ((b.frames as f64) / (ms / 1000.0) * 10.0).round() / 10.0,
            "max_frame_ms": (b.max_frame_ms * 10.0).round() / 10.0,
            "wakeups": b.wakeups,
        }));
    }

    // ───────────────────────── 尺寸（#156 / #203）─────────────────────────

    fn window_size_for(&self, s: Size) -> WindowSize {
        WindowSize {
            num_lines: s.rows,
            num_cols: s.cols,
            cell_width: f32::from(self.cell_w) as u16,
            cell_height: f32::from(self.line_h) as u16,
        }
    }

    /// 每帧 prepaint 时调一次（= 每帧最多一次 resize）。量出来的区域太小（不可见 / 祖先隐藏）就什么都不做
    fn layout(&mut self, width: f32, height: f32, cell_w: Pixels, line_h: Pixels, cx: &mut Context<Self>) {
        if !is_visible_area(width, height) {
            return;
        }
        self.laid_out = (width, height);
        let now = Instant::now();
        // 隔了一阵才又被画（切标签回来、刚显示出来）：这次行列都立即到位
        let long_gap = self.last_layout.map(|t| now.duration_since(t).as_millis() > 100).unwrap_or(true);
        self.last_layout = Some(now);
        let metrics_changed = cell_w != self.cell_w || line_h != self.line_h;
        self.cell_w = cell_w;
        self.line_h = line_h;
        // 下限两边同夹：网格和 PTY 用同一个夹过的数（#156）
        let proposed = clamp_size(propose_geometry(width, height, f32::from(cell_w), f32::from(line_h)));
        self.desired = proposed;
        if self.pty.is_none() {
            if proposed != self.size {
                self.apply_size(proposed);
            }
            self.spawn(proposed, cx);
            return;
        }
        let immediate = std::mem::take(&mut self.immediate) || long_gap || metrics_changed;
        if std::mem::take(&mut self.cols_due) {
            self.apply_size(proposed);
            return;
        }
        let buffer_lines = {
            let t = self.term.lock();
            t.grid().history_size() + t.screen_lines()
        };
        let plan = plan_resize(self.size, proposed, buffer_lines, immediate);
        if plan.cols_now {
            self.apply_size(proposed);
            return;
        }
        if plan.rows_now {
            self.apply_size(Size { cols: self.size.cols, rows: proposed.rows });
        }
        if plan.cols_later {
            match self.follower.request(self.now_ms()) {
                Follow::RunNow => self.apply_size(proposed),
                Follow::Scheduled { at } => self.schedule_cols(at, cx),
            }
        }
    }

    fn schedule_cols(&mut self, at: f64, cx: &mut Context<Self>) {
        if self.follow_timer {
            return;
        }
        self.follow_timer = true;
        let delay = (at - self.now_ms()).max(0.0) as u64;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            let _ = this.update(cx, |v, cx| {
                v.follow_timer = false;
                let now = v.now_ms();
                if v.follower.fire(now) {
                    v.cols_due = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 唯一改网格尺寸 + 通知 PTY 的地方。判定和记账是同一个动作（`ledger.claim`），发送失败退账
    fn apply_size(&mut self, s: Size) {
        if s != self.size {
            self.term.lock().resize(TermSize { cols: s.cols as usize, rows: s.rows as usize });
            self.size = s;
        }
        let Some(p) = &self.pty else { return };
        let claim = self.ledger.claim(s);
        if claim.send && p.notifier.0.send(Msg::Resize(self.window_size_for(s))).is_err() {
            self.ledger.revert(s, claim.prev);
        }
    }

    /// 切标签 / 最大化 / 还原之后：下一帧行列都立即到位
    pub fn fit_now(&mut self, cx: &mut Context<Self>) {
        self.immediate = true;
        cx.notify();
    }

    /// 拖分割线松手：欠着的列数立即补上（#203）
    pub fn flush_resize(&mut self, cx: &mut Context<Self>) {
        if self.follower.flush(self.now_ms()) {
            self.cols_due = true;
        }
        self.immediate = true;
        cx.notify();
    }

    pub fn grid_size(&self) -> Size {
        self.size
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// 字号缩放：只作用于当前 pane，不持久化；撞到边界跳过；改完走同一条 fit 路径（立即）同步 PTY 尺寸
    fn zoom(&mut self, z: zoom::Zoom, cx: &mut Context<Self>) {
        let next = zoom::next_font_size(self.font_size, z);
        if next == self.font_size {
            return;
        }
        self.font_size = next;
        self.fit_now(cx);
    }

    // ───────────────────────── 写入 ─────────────────────────

    /// 写 PTY；还没就绪就先存着（spawn 成功后一次性写入）
    fn write_pty(&mut self, bytes: Vec<u8>) {
        match &self.pty {
            Some(p) => p.notifier.notify(bytes),
            None => self.pending_input.extend(bytes),
        }
    }

    /// 用户输入（键盘 / 输入法 / 粘贴 / 拖入）：发 `UserInput`（节流 1 秒）后照常写
    fn write_user_cx(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.last_user_input.map(|t| now.duration_since(t).as_millis() >= 1000).unwrap_or(true) {
            self.last_user_input = Some(now);
            cx.emit(UserInput { session_id: self.spec.session_id.clone() });
        }
        self.write_user(bytes);
    }

    /// 用户输入：回到底部、清掉选区、光标重新亮起
    fn write_user(&mut self, bytes: Vec<u8>) {
        {
            let mut term = self.term.lock();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            term.selection = None;
        }
        self.blink_on = true;
        self.blink_epoch += 1;
        self.write_pty(bytes);
    }

    /// 插入一段文本，语义等同一次粘贴（拖文件进来插路径用）：只有程序开了 DECSET 2004 才包括号粘贴标记
    /// （macOS 文件名里可以有换行，裸写就等于回车执行；`cat` 之类没开的程序会把标记原样回显）。
    /// PTY 还没起来返回 false，不排队
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.pty.is_none() || text.is_empty() {
            return false;
        }
        let bytes = self.paste_bytes(text);
        self.write_user_cx(bytes, cx);
        cx.notify();
        true
    }

    /// 拖进来的文件路径：按 dropPaths 规则加引号、空格连接、末尾一个空格
    pub fn insert_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) -> bool {
        let ps: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let text = drop_paths::format_paths_for_terminal(&ps);
        self.insert_text(&text, cx)
    }

    fn paste_bytes(&self, text: &str) -> Vec<u8> {
        if self.term.lock().mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }

    /// 当前选中的文字（右键菜单「复制」「搜索选中内容」用）
    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bytes = self.paste_bytes(&text);
        self.write_user_cx(bytes, cx);
        cx.notify();
    }

    fn scroll_page(&mut self, up: bool, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        cx.notify();
    }

    // ───────────────────────── 光标闪烁 ─────────────────────────

    fn start_blink(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut last_epoch = u64::MAX;
            loop {
                cx.background_executor().timer(Duration::from_millis(CURSOR_BLINK_MS)).await;
                let alive = this.update(cx, |v, cx| {
                    if v.shut_down {
                        return false;
                    }
                    // 刚打过字：这一拍保持亮着（重新计时）
                    if v.blink_epoch != last_epoch {
                        last_epoch = v.blink_epoch;
                        if !v.blink_on {
                            v.blink_on = true;
                            cx.notify();
                        }
                        return true;
                    }
                    let blinking = v.term.lock().cursor_style().blinking;
                    if blinking {
                        v.blink_on = !v.blink_on;
                        cx.notify();
                    } else if !v.blink_on {
                        v.blink_on = true;
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    // ───────────────────────── 调试 / perf ─────────────────────────

    /// 背压实测（无人值守，`MAKIT_TERM_FLOOD=1`）：终端起来 1.5 秒后敲 `yes`，刷 5 秒，量应用内存和画了多少帧，
    /// 再发 Ctrl-C，量输出多久停下来（最后一次 Wakeup 距 Ctrl-C 的时间）。结果打在 stderr，行首 `[term-flood]`
    fn start_flood_probe(&mut self, cx: &mut Context<Self>) {
        fn rss_mb() -> f64 {
            let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output();
            out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
        }
        cx.spawn(async move |this, cx| {
            let ex = cx.background_executor().clone();
            ex.timer(Duration::from_millis(1500)).await;
            let rss0 = rss_mb();
            let _ = this.update(cx, |v, _| v.write_user(b"yes\r".to_vec()));
            ex.timer(Duration::from_secs(5)).await;
            let rss1 = rss_mb();
            let frames = this.update(cx, |v, _| v.burst.as_ref().map(|b| b.frames).unwrap_or(0)).unwrap_or(0);
            let sent = Instant::now();
            let _ = this.update(cx, |v, _| v.write_user(vec![0x03]));
            loop {
                ex.timer(Duration::from_millis(20)).await;
                let last = this.update(cx, |v, _| v.burst.as_ref().map(|b| b.last_wakeup)).ok().flatten();
                match last {
                    Some(t) if t.elapsed().as_millis() < 300 && sent.elapsed().as_secs() < 30 => continue,
                    _ => {
                        let stop = last.map(|t| t.saturating_duration_since(sent).as_millis()).unwrap_or(0);
                        eprintln!(
                            "[term-flood] yes 刷 5 秒：应用 RSS {rss0:.1}MB → {rss1:.1}MB（涨 {:.1}MB），这段画了 {frames} 帧；Ctrl-C 后 {stop}ms 输出停下",
                            rss1 - rss0
                        );
                        break;
                    }
                }
            }
        })
        .detach();
    }

    /// 可见区域的文字（调试用：`MAKIT_NATIVE_DUMP=<文件>` 时每 300ms 写一次，无人值守时核对网格内容）
    fn dump_text(&self) -> String {
        use alacritty_terminal::term::cell::Flags;
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let mut lines = vec![String::new(); self.size.rows as usize];
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            if row < lines.len() && !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                lines[row].push(cell.c);
            }
        }
        let mode = *term.mode();
        format!(
            "area={:.1}x{:.1} desired={}x{} cols={} rows={} pty={:?} font={}@{} cell={:.3}x{:.3} alt_screen={} display_offset={} history={} cwd={}\n{}\n",
            self.laid_out.0,
            self.laid_out.1,
            self.desired.cols,
            self.desired.rows,
            self.size.cols,
            self.size.rows,
            self.ledger.known().map(|s| (s.cols, s.rows)),
            element::picked_family().unwrap_or_default(),
            self.font_size,
            f32::from(self.cell_w),
            f32::from(self.line_h),
            mode.contains(TermMode::ALT_SCREEN),
            offset,
            term.grid().history_size(),
            self.cwd(),
            lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
        )
    }

    /// 给 perf：第一次把输出画出来；大量输出时数帧
    fn on_painted(&mut self) {
        let now = Instant::now();
        if let Some(path) = std::env::var_os("MAKIT_NATIVE_DUMP") {
            if self.last_dump.map(|t| now.duration_since(t).as_millis() >= 300).unwrap_or(true) {
                self.last_dump = Some(now);
                // 每个终端一份（`<文件>.<pty id>`），多 pane 时逐个核对；原文件名仍写最后画的那个
                let text = self.dump_text();
                if let Some(id) = &self.spec.pty_id {
                    let mut per = path.clone();
                    per.push(format!(".{id}"));
                    let _ = std::fs::write(per, &text);
                }
                let _ = std::fs::write(path, text);
            }
        }
        if let Some(b) = &mut self.burst {
            if let Some(last) = b.last_frame {
                let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                if dt > b.max_frame_ms {
                    b.max_frame_ms = dt;
                }
            }
            b.last_frame = Some(now);
            b.frames += 1;
        }
        if self.first_paint_logged {
            return;
        }
        let Some(out) = self.first_output_at else { return };
        self.first_paint_logged = true;
        let ms = |t: Instant| (t.duration_since(self.spec.clicked_at).as_secs_f64() * 1000.0).round();
        perf::record(serde_json::json!({
            "kind": "terminal-open",
            "app": "gpui",
            "tab": self.spec.kind,
            "ms": ms(out),
            "stages": [
                ["终端开始创建", ms(self.created_at)],
                ["首次输出", ms(out)],
                ["首次画出", ms(now)],
            ],
        }));
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        self.colors = TermColors::from_theme(&theme);
        let show_jump = {
            let t = self.term.lock();
            scrollbar::should_show_jump_latest(t.grid().history_size() - t.grid().display_offset(), t.grid().history_size())
        };
        // 「↓ 回到最新」（#205）：right 18 / bottom 12（相对 .xterm-inner），padding 4/10，1px border-strong 描边，
        // 全圆角，bg-soft 底，11px 字，行高 1.6，不透明度 0.92（悬停 1 + bg-hover）；按下不抢终端焦点
        let jump = div()
            .id("jump-latest")
            .absolute()
            .right(px(18.0))
            .bottom(px(12.0))
            .px(px(10.0))
            .py(px(4.0))
            .border_1()
            .border_color(theme.border_strong)
            .rounded_full()
            .bg(theme.bg_soft)
            .text_color(theme.fg)
            .text_size(px(11.0))
            .line_height(px(11.0 * 1.6))
            .opacity(0.92)
            .cursor_pointer()
            .occlude()
            .hover(|s| s.opacity(1.0).bg(theme.bg_hover))
            .child(SharedString::from("↓ 回到最新"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|v, _, _, cx| v.scroll_to_bottom(cx)));
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|v, _: &ScrollPageUp, _, cx| v.scroll_page(true, cx)))
            .on_action(cx.listener(|v, _: &ScrollPageDown, _, cx| v.scroll_page(false, cx)))
            .on_action(cx.listener(|v, _: &act::FontIncrease, _, cx| v.zoom(zoom::Zoom::In, cx)))
            .on_action(cx.listener(|v, _: &act::FontDecrease, _, cx| v.zoom(zoom::Zoom::Out, cx)))
            .on_action(cx.listener(|v, _: &act::FontReset, _, cx| v.zoom(zoom::Zoom::Reset, cx)))
            .relative()
            .size_full()
            .bg(theme.bg)
            .child(
                div()
                    .absolute()
                    .top(px(INSET_TOP))
                    .right(px(INSET_RIGHT))
                    .bottom(px(INSET_BOTTOM))
                    .left(px(INSET_LEFT))
                    .child(TerminalElement { view: cx.entity() })
                    .when(show_jump, |d| d.child(jump)),
            )
    }
}
