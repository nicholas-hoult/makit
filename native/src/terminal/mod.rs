//! 终端：alacritty_terminal 做 PTY + VT 解析，GPUI 画网格（#221）。
//!
//! 结构照 Zed 的 `crates/terminal` + `crates/terminal_view`（gpui 0.2.2 对应的 69e2130）：
//! - PTY 和解析在 alacritty 的 EventLoop 线程里跑，解析结果写进 `Term`（FairMutex 保护）
//! - 它通过 `Listener` 把事件（有新内容、标题、要回写 PTY 的应答……）发到 GPUI 这边
//! - GPUI 这边收到 Wakeup 只 `notify`，下一帧 prepaint 时锁一下 Term、把可见区域拍成
//!   背景块 + 文字段，paint 时画出来。输出再快，一帧也只画一次。

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as AlacPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, div, fill, font, point, prelude::*, px,
    relative, rgb, size,
};

mod element;

pub mod contrast;
pub mod drop_paths;
pub mod grid;
pub mod keys;
pub mod links;
pub mod osc;
pub mod palette;
pub mod scrollbar;
pub mod segment;
pub mod size;
pub mod wheel;
pub mod zoom;

use element::TerminalElement;
use grid::{point_to_cell, wheel_lines};
use keys::{Mods, key_to_bytes, sgr_wheel};

use crate::perf;

actions!(terminal, [Copy, Paste, ScrollPageUp, ScrollPageDown]);

pub const FONT_FAMILY: &str = "Menlo";
pub const FONT_SIZE: f32 = 13.0;
pub const LINE_HEIGHT: f32 = 17.0;

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

impl EventEmitter<TerminalEvent> for TerminalView {}

/// 怎么启动
pub struct SpawnSpec {
    /// 标签 id，注入成子进程环境变量 `MAKIT_PTY_ID`（运行状态绑定、关标签杀逃逸进程都认它）
    pub pty_id: Option<String>,
    pub cwd: Option<String>,
    /// 启动后像打字一样写进 shell 的命令（同 WebView 版的 initCommand：`clear && claude -r <id>`）
    pub init_command: Option<String>,
    pub session_id: Option<String>,
    /// perf.log 里的 tab 类型：resume / shell
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

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    focus: FocusHandle,
    pub title: String,
    pub exited: bool,
    cols: u16,
    rows: u16,
    cell_w: Pixels,
    line_h: Pixels,
    wheel_accum: f32,
    selecting: bool,
    marked_text: Option<String>,
    // ── 性能埋点 ──
    kind: &'static str,
    clicked_at: Instant,
    created_at: Instant,
    first_output_at: Option<Instant>,
    first_paint_logged: bool,
    burst: Option<Burst>,
    last_dump: Option<Instant>,
    child_pid: u32,
    shut_down: bool,
}

impl TerminalView {
    pub fn new(spec: SpawnSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let created_at = Instant::now();
        let (tx, mut rx) = unbounded();
        let size = TermSize { cols: 80, rows: 24 };
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, Listener(tx.clone()))));

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        // GUI 启动的子进程可能缺 locale，强制 UTF-8 防中文乱码（同 pty.rs）
        env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        env.insert("LC_ALL".to_string(), "en_US.UTF-8".to_string());
        if let Some(id) = &spec.pty_id {
            env.insert("MAKIT_PTY_ID".to_string(), id.clone());
        }
        if let Some(sid) = &spec.session_id {
            env.insert("MAKIT_SESSION_ID".to_string(), sid.clone());
        }
        let cwd = spec
            .cwd
            .as_ref()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(dirs_home);
        let options = tty::Options {
            // login shell：读 .zprofile 拿到完整 PATH（同 pty.rs）
            shell: Some(tty::Shell::new(shell, vec!["-l".to_string()])),
            working_directory: cwd,
            drain_on_exit: true,
            env,
        };
        let window_size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 17 };
        let pty = tty::new(&options, window_size, 0).expect("PTY 创建失败");
        let child_pid = pty.child().id();
        let event_loop = EventLoop::new(term.clone(), Listener(tx), pty, true, false)
            .expect("终端事件循环创建失败");
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();

        if let Some(cmd) = &spec.init_command {
            notifier.notify(format!("{cmd}\r").into_bytes());
        }

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

        // 不在这里抢焦点：恢复布局时一次会建好几个终端，焦点由工作区视图给当前标签
        let focus = cx.focus_handle();
        let _ = window;
        Self {
            term,
            notifier,
            focus,
            title: String::new(),
            exited: false,
            cols: 80,
            rows: 24,
            cell_w: px(8.0),
            line_h: px(LINE_HEIGHT),
            wheel_accum: 0.0,
            selecting: false,
            marked_text: None,
            kind: spec.kind,
            clicked_at: spec.clicked_at,
            created_at,
            first_output_at: None,
            first_paint_logged: false,
            burst: None,
            last_dump: None,
            child_pid,
            shut_down: false,
        }
    }

    /// 关标签 / 退出应用时：挂断 shell（zsh 收到 SIGHUP 会把它的作业一起挂断），停掉 IO 线程。
    /// 不做的话 claude 会变成没有终端的孤儿进程一直挂着。
    pub fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        unsafe {
            libc::kill(self.child_pid as i32, libc::SIGHUP);
        }
        let _ = self.notifier.0.send(Msg::Shutdown);
    }

    fn handle_event(&mut self, ev: AlacEvent, cx: &mut Context<Self>) {
        match ev {
            AlacEvent::Wakeup => {
                let now = Instant::now();
                if self.first_output_at.is_none() {
                    self.first_output_at = Some(now);
                }
                match &mut self.burst {
                    Some(b) if now.duration_since(b.last_wakeup).as_millis() < 500 => {
                        b.last_wakeup = now;
                        b.wakeups += 1;
                    }
                    _ => {
                        self.flush_burst();
                        self.burst = Some(Burst {
                            started: now,
                            last_wakeup: now,
                            frames: 0,
                            last_frame: None,
                            max_frame_ms: 0.0,
                            wakeups: 1,
                        });
                        // 输出停下来 600ms 后把这一段写掉
                        cx.spawn(async move |this, cx| {
                            loop {
                                cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
                                let done = this
                                    .update(cx, |v, _| {
                                        let idle = v
                                            .burst
                                            .as_ref()
                                            .map(|b| b.last_wakeup.elapsed().as_millis() >= 500)
                                            .unwrap_or(true);
                                        if idle {
                                            v.flush_burst();
                                        }
                                        idle
                                    })
                                    .unwrap_or(true);
                                if done {
                                    break;
                                }
                            }
                        })
                        .detach();
                    }
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
            AlacEvent::PtyWrite(s) => self.notifier.notify(s.into_bytes()),
            AlacEvent::ColorRequest(index, format) => {
                let colors = self.term.lock().colors()[index];
                let c = colors.unwrap_or_else(|| {
                    let v = default_color(index);
                    Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 }
                });
                self.notifier.notify(format(c).into_bytes());
            }
            AlacEvent::TextAreaSizeRequest(format) => {
                let ws = self.window_size();
                self.notifier.notify(format(ws).into_bytes());
            }
            AlacEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            AlacEvent::ClipboardLoad(_, format) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.notifier.notify(format(&text).into_bytes());
            }
            AlacEvent::ChildExit(_) | AlacEvent::Exit => {
                if !self.exited {
                    self.exited = true;
                    cx.emit(TerminalEvent::Exited);
                    cx.notify();
                }
            }
            AlacEvent::Bell | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => {}
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

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: f32::from(self.cell_w) as u16,
            cell_height: f32::from(self.line_h) as u16,
        }
    }

    /// 布局量出新尺寸时调：改 Term 的网格、通知 PTY（SIGWINCH）
    fn resize(&mut self, cols: u16, rows: u16, cell_w: Pixels, line_h: Pixels) {
        self.cell_w = cell_w;
        self.line_h = line_h;
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(TermSize { cols: cols as usize, rows: rows as usize });
        let _ = self.notifier.0.send(Msg::Resize(self.window_size()));
    }

    fn write(&mut self, bytes: Vec<u8>) {
        // 打字时回到底部（同常见终端）
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        term.selection = None;
        drop(term);
        self.notifier.notify(bytes);
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        let mods = Mods { ctrl: m.control, alt: m.alt, shift: m.shift, cmd: m.platform };
        let mode = self.mode();
        if let Some(bytes) = key_to_bytes(&ev.keystroke.key, mods, mode.contains(TermMode::APP_CURSOR)) {
            self.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bytes = if self.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.write(bytes.into_bytes());
    }

    fn scroll_page(&mut self, up: bool, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
        cx.notify();
    }

    fn scroll_wheel(&mut self, ev: &ScrollWheelEvent, local: Point<Pixels>, cx: &mut Context<Self>) {
        let delta_px = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y * f32::from(self.line_h),
        };
        let (lines, acc) = wheel_lines(self.wheel_accum, delta_px, f32::from(self.line_h));
        self.wheel_accum = acc;
        if lines == 0 {
            return;
        }
        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE) {
            // 程序自己要鼠标事件：滚轮上报给它
            let (col, row) = self.cell_at(local);
            let mut all = Vec::new();
            for _ in 0..lines.abs() {
                all.extend(sgr_wheel(lines > 0, col, row));
            }
            self.notifier.notify(all);
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // 全屏程序（less、vim……）：滚轮翻成上下方向键
            let key = if lines > 0 { "up" } else { "down" };
            let bytes = key_to_bytes(key, Mods::default(), mode.contains(TermMode::APP_CURSOR)).unwrap_or_default();
            let mut all = Vec::new();
            for _ in 0..lines.abs() {
                all.extend_from_slice(&bytes);
            }
            self.notifier.notify(all);
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }

    /// 视口里的 (列, 行) → alacritty 网格坐标（带上回看偏移）
    fn grid_point(&self, col: usize, row: usize) -> AlacPoint {
        let offset = self.term.lock().grid().display_offset() as i32;
        AlacPoint::new(Line(row as i32 - offset), Column(col))
    }

    fn mouse_down(&mut self, local: Point<Pixels>, click_count: usize, cx: &mut Context<Self>) {
        let (col, row) = self.cell_at(local);
        let p = self.grid_point(col, row);
        let ty = match click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(ty, p, self.side_at(local)));
        self.selecting = true;
        cx.notify();
    }

    fn mouse_drag(&mut self, local: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let (col, row) = self.cell_at(local);
        let p = self.grid_point(col, row);
        let side = self.side_at(local);
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(p, side);
        }
        cx.notify();
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.term.lock();
        if term.selection.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            term.selection = None;
        }
        cx.notify();
    }

    fn cell_at(&self, local: Point<Pixels>) -> (usize, usize) {
        point_to_cell(
            f32::from(local.x),
            f32::from(local.y),
            f32::from(self.cell_w),
            f32::from(self.line_h),
            self.cols,
            self.rows,
        )
    }

    fn side_at(&self, local: Point<Pixels>) -> Side {
        let w = f32::from(self.cell_w);
        if w <= 0.0 {
            return Side::Left;
        }
        if (f32::from(local.x) % w) < w / 2.0 { Side::Left } else { Side::Right }
    }

    /// 可见区域的文字（调试用：`MAKIT_NATIVE_DUMP=<文件>` 时每 300ms 写一次，无人值守时核对网格内容）
    fn dump_text(&self) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let mut lines = vec![String::new(); self.rows as usize];
        for cell in content.display_iter {
            let row = (cell.point.line.0 + offset) as usize;
            if row < lines.len() && !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                lines[row].push(cell.c);
            }
        }
        let mode = *term.mode();
        format!(
            "cols={} rows={} alt_screen={} display_offset={}\n{}\n",
            self.cols,
            self.rows,
            mode.contains(TermMode::ALT_SCREEN),
            offset,
            lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
        )
    }

    /// 给 perf：第一次把输出画出来；大量输出时数帧
    fn on_painted(&mut self) {
        let now = Instant::now();
        if let Some(path) = std::env::var_os("MAKIT_NATIVE_DUMP") {
            if self.last_dump.map(|t| now.duration_since(t).as_millis() >= 300).unwrap_or(true) {
                self.last_dump = Some(now);
                let _ = std::fs::write(path, self.dump_text());
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
        let ms = |t: Instant| (t.duration_since(self.clicked_at).as_secs_f64() * 1000.0).round();
        perf::record(serde_json::json!({
            "kind": "terminal-open",
            "app": "gpui",
            "tab": self.kind,
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

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn default_color(index: usize) -> u32 {
    match index {
        0..=255 => palette::indexed(index as u8),
        256 => palette::FG,
        257 => palette::BG,
        258 => palette::CURSOR,
        _ => palette::FG,
    }
}

fn hsla(v: u32) -> Hsla {
    rgb(v).into()
}

/// 单元格颜色 → 实际颜色：程序用 OSC 改过的颜色优先，否则走调色板
fn resolve(color: AnsiColor, overrides: &alacritty_terminal::term::color::Colors) -> u32 {
    let from_rgb = |c: Rgb| ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32;
    match color {
        AnsiColor::Spec(c) => from_rgb(c),
        AnsiColor::Indexed(i) => overrides[i as usize].map(from_rgb).unwrap_or_else(|| palette::indexed(i)),
        AnsiColor::Named(n) => {
            let idx = n as usize;
            if let Some(c) = overrides[idx] {
                return from_rgb(c);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => palette::FG,
                NamedColor::Background => palette::BG,
                NamedColor::Cursor => palette::CURSOR,
                NamedColor::DimForeground => dim(palette::FG),
                _ if idx < 16 => palette::ANSI[idx],
                // Dim 系列：DimBlack = 259 起，对应 ANSI 0..7
                _ => dim(palette::ANSI[(idx - NamedColor::DimBlack as usize).min(7)]),
            }
        }
    }
}

fn dim(v: u32) -> u32 {
    let f = |c: u32| (c * 2 / 3) & 0xff;
    (f(v >> 16) << 16) | (f((v >> 8) & 0xff) << 8) | f(v & 0xff)
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|v, _: &ScrollPageUp, _, cx| v.scroll_page(true, cx)))
            .on_action(cx.listener(|v, _: &ScrollPageDown, _, cx| v.scroll_page(false, cx)))
            .size_full()
            .bg(hsla(palette::BG))
            .pl(px(6.0))
            .pt(px(4.0))
            .child(TerminalElement { view: cx.entity() })
    }
}

// ── 输入法：中文上屏走这里，拼音组字（marked text）画在光标处 ──
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = if new_text.is_empty() { None } else { Some(new_text.to_string()) };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 候选框贴着光标
        let term = self.term.lock();
        let c = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let row = (c.line.0 + offset).max(0) as f32;
        Some(Bounds::new(
            point(
                element_bounds.origin.x + self.cell_w * c.column.0 as f32,
                element_bounds.origin.y + self.line_h * row,
            ),
            size(self.cell_w, self.line_h),
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

