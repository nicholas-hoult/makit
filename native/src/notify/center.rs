//! `Notifier`：通知的运行时（GPUI 实体，挂成全局 `GlobalNotifier`）。把纯逻辑串起来：
//!
//! ```text
//! hook 行 ──classify_hook_event──┐
//!                                ├─ Arbiter（按类主从）─ deliver（在屏？开关？）─ Book（记录 reducer）─ Effects
//! 会话列表 status ─classify_status_change─┘                                                     │
//!   横幅 / 撤回 → system（UNUserNotificationCenter） · Dock 角标 · 存盘（prefs.notifications） · 闪窗口 ◄─┘
//! ```
//!
//! 已读时机（#215 第 4 节）：窗口聚焦时 active tab 就是它（切标签、窗口回到前台）；点横幅；点通知中心那条；
//! 在终端里打字（A 包调 `mark_session_read`）。

use crate::tr;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Global, Pixels, ScrollHandle, Window};

use crate::state::{AppEvent, AppState};
use crate::workspace::model::collect_containers;

use super::book::{Banner, Book, Effects};
use super::classify::{classify_hook_event, classify_status_change, Arbiter, RunStatus};
use super::deliver::{self, deliver, is_on_screen, on_screen_session};
use super::model::{Kind, Signal};
use super::system::{set_dock_badge, Permission, SysEvent, System};

/// 窗口闪一下的时长（App.css `window-notify-flash 0.55s`）
pub const FLASH_MS: u64 = 550;

/// Notifier 发给别的包的事件（`cx.subscribe(&notifier, …)`）
#[derive(Clone, Debug, PartialEq)]
pub enum NotifyEvent {
    /// 跳转时会话已在某个标签里：已经切过去了，请 C 包在这个 pane 上闪落点牌（Tauri 版 `flashContainer`）
    FlashPane { container_id: String },
    /// 跳转时会话不在任何标签里：请 B 包在侧栏里定位它（Tauri 版 `revealSidebarSession`）。只定位，不打开
    RevealSession { session_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Hook,
    Status,
}

pub struct Notifier {
    state: Entity<AppState>,
    pub book: Book,
    arbiter: Arbiter,
    /// 上一次看到的运行状态（状态文件那路做差分）
    status_prev: HashMap<String, RunStatus>,
    /// 还没做过第一次状态观察：第一次看到的等待只进中心、不弹（清单 G「启动后第一次轮询」）
    status_baseline: bool,
    pub window_active: bool,
    pub permission: Permission,
    system: System,
    /// 授权框还开着时来的横幅：授权后补发（只留最新一条，同会话本来就是替换）
    pending_banner: Option<Banner>,
    last_badge: Option<usize>,

    // ---- 抽屉 ----
    pub open: bool,
    /// 键盘高亮的行（book.records 的下标）
    pub selected: Option<usize>,
    /// 铃铛在窗口里的位置（C 包的标题栏里画铃铛时记下；抽屉的 left 对齐它，清单 A「按铃铛的 left 定位抽屉」）
    pub bell_bounds: Option<Bounds<Pixels>>,
    pub focus: FocusHandle,
    restore_focus: Option<FocusHandle>,
    /// 列表滚动（键盘移动时把高亮行滚进视野）
    pub scroll: ScrollHandle,
    pub scrollbar: Entity<crate::scrollbar::Scrollbar>,

    // ---- 窗口闪一下 ----
    pub flash_seq: u64,
    pub flash_until: Option<Instant>,
}

impl EventEmitter<NotifyEvent> for Notifier {}

pub struct GlobalNotifier(pub Entity<Notifier>);
impl Global for GlobalNotifier {}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Notifier {
    /// 建实体、挂全局、起 hook 服务和系统通知层。在 `AppState::init` 之后、开窗口之前调
    pub fn init(state: Entity<AppState>, cx: &mut App) -> Entity<Self> {
        let (sys_tx, mut sys_rx) = unbounded::<SysEvent>();
        let (hook_tx, mut hook_rx) = unbounded::<String>();
        let book = Book::from_saved(&state.read(cx).prefs.notifications);
        let system = System::start(sys_tx);
        let this = cx.new(|cx| {
            cx.subscribe(&state, |this: &mut Self, _, ev: &AppEvent, cx| match ev {
                AppEvent::SessionsChanged => this.on_sessions_changed(cx),
                AppEvent::WorkspaceChanged => this.read_what_is_on_screen(cx),
                AppEvent::PrefsChanged => {}
            })
            .detach();
            cx.spawn(async move |this, cx| {
                while let Some(line) = hook_rx.next().await {
                    if this.update(cx, |n, cx| n.on_hook_line(&line, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            cx.spawn(async move |this, cx| {
                while let Some(ev) = sys_rx.next().await {
                    if this.update(cx, |n, cx| n.on_sys_event(ev, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            let scroll = ScrollHandle::new();
            let scrollbar = crate::scrollbar::Scrollbar::handle(&scroll, cx);
            Self {
                state: state.clone(),
                book,
                arbiter: Arbiter::default(),
                status_prev: HashMap::new(),
                status_baseline: false,
                window_active: true,
                permission: if system.available() { Permission::Unknown } else { Permission::Unavailable },
                pending_banner: None,
                system,
                last_badge: None,
                open: false,
                selected: None,
                bell_bounds: None,
                focus: cx.focus_handle(),
                restore_focus: None,
                scroll,
                scrollbar,
                flash_seq: 0,
                flash_until: None,
            }
        });
        start_hook_server(hook_tx);
        this.update(cx, |n, _| n.sync_badge());
        cx.set_global(GlobalNotifier(this.clone()));
        this
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalNotifier>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalNotifier>().map(|g| g.0.clone())
    }

    pub fn unread_count(&self) -> usize {
        self.book.unread_count()
    }

    // ---------- 来源 ----------

    fn on_hook_line(&mut self, line: &str, cx: &mut Context<Self>) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            log::warn!(target: "notify", "hook 行不是 JSON，跳过");
            return;
        };
        if let Some((sid, sig)) = classify_hook_event(&v) {
            self.handle(&sid, sig, Source::Hook, false, cx);
        }
    }

    /// 会话列表变了：对比运行状态，产生等待 / 离开等待 / 一轮跑完
    fn on_sessions_changed(&mut self, cx: &mut Context<Self>) {
        let (loaded, now) = {
            let s = self.state.read(cx);
            let now: HashMap<String, RunStatus> = s
                .sessions
                .iter()
                .filter(|m| m.running && !m.status.is_empty())
                .map(|m| (m.session_id.clone(), RunStatus::new(&m.status, &m.waiting_for)))
                .collect();
            (s.loaded, now)
        };
        if !loaded {
            return;
        }
        let quiet = !self.status_baseline;
        let ids: HashSet<String> = self.status_prev.keys().chain(now.keys()).cloned().collect();
        let prev = std::mem::take(&mut self.status_prev);
        for sid in ids {
            if let Some(sig) = classify_status_change(prev.get(&sid), now.get(&sid)) {
                self.handle(&sid, sig, Source::Status, quiet, cx);
            }
        }
        self.status_prev = now;
        self.status_baseline = true;
    }

    fn on_sys_event(&mut self, ev: SysEvent, cx: &mut Context<Self>) {
        match ev {
            SysEvent::Clicked(sid) => {
                // 点横幅：跳过去 + 已读（#215 问题 8：Tauri 版点了没反应）
                cx.activate(true);
                self.navigate(&sid, cx);
            }
            SysEvent::Permission(p) => {
                self.permission = p;
                // 授权框刚被点掉：授权了就补发等着的那条；拒绝了就丢掉
                if let Some(b) = self.pending_banner.take() {
                    if p == Permission::Granted {
                        self.post_banner(&b, cx);
                    }
                }
                cx.notify();
            }
        }
    }

    fn handle(&mut self, sid: &str, sig: Signal, source: Source, quiet: bool, cx: &mut Context<Self>) {
        match source {
            Source::Hook => self.arbiter.note_hook(sid, &sig),
            Source::Status if !self.arbiter.allow_status(sid, &sig) => return,
            Source::Status => {}
        }
        let e = match sig {
            Signal::Notify { kind, message } => {
                let on_screen = is_on_screen(&self.state.read(cx).workspace.state, self.window_active, sid);
                let has_body = kind != Kind::TurnComplete || !message.is_empty();
                let prefs = self.state.read(cx).prefs.notify.clone();
                let d = deliver(kind, on_screen, has_body, quiet, &prefs);
                self.book.notify(sid, kind, &message, now_ms(), d)
            }
            Signal::Resolved => self.book.resolve(sid),
            Signal::Clear => self.book.clear_session(sid),
        };
        self.apply(e, cx);
    }

    // ---------- 效果 ----------

    fn apply(&mut self, e: Effects, cx: &mut Context<Self>) {
        if e.flash {
            self.flash_window(cx);
        }
        if let Some(b) = &e.banner {
            self.post_banner(b, cx);
        }
        self.system.withdraw(&e.withdraw);
        if e.changed {
            self.clamp_selection();
            let saved = self.book.to_saved();
            self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.notifications = saved));
            self.sync_badge();
            cx.notify();
        }
    }

    fn sync_badge(&mut self) {
        let n = self.book.unread_count();
        if self.last_badge != Some(n) {
            self.last_badge = Some(n);
            set_dock_badge(n);
        }
    }

    /// 窗口边框亮一下（清单 A「窗口闪一下提醒」，App.css `.app-notify-flash`）
    pub fn flash_window(&mut self, cx: &mut Context<Self>) {
        self.flash_seq += 1;
        self.flash_until = Some(Instant::now() + Duration::from_millis(FLASH_MS));
        cx.notify();
        let seq = self.flash_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(FLASH_MS + 50)).await;
            let _ = this.update(cx, |n, cx| {
                if n.flash_seq == seq {
                    n.flash_until = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 横幅文案：标题「Claude 等待审批」（清单 G）；没有原话时正文「会话名 — 项目名」（清单 G），
    /// 有原话时它进副标题、原话当正文（#215，照 对标产品）。名字发的时候现取（#8）
    pub fn banner_text(&self, b: &Banner, cx: &App) -> (String, String, String) {
        let title = format!("Claude {}", b.kind.label());
        let (name, project) = self.names(&b.session_id, cx);
        let who = if project.is_empty() { name } else { format!("{name} — {project}") };
        if b.message.is_empty() {
            (title, String::new(), who)
        } else {
            (title, who, b.message.clone())
        }
    }

    fn post_banner(&mut self, b: &Banner, cx: &App) {
        let (title, subtitle, body) = self.banner_text(b, cx);
        match deliver::plan_post(self.permission) {
            deliver::PostPlan::Now => self.system.post(&b.session_id, &title, &subtitle, &body, b.sound, b.kind == Kind::TurnComplete),
            deliver::PostPlan::AskThenPost => {
                // 已经有一条在等授权框：只换成最新的，不重复弹框
                if self.pending_banner.replace(b.clone()).is_none() {
                    self.system.request_permission();
                }
            }
            deliver::PostPlan::Skip => log::info!(target: "notify", "无 app 身份或已被拒绝，未发横幅（标题 / 正文可能含对话内容，不记）"),
        }
    }

    /// 会话名 + 项目名，现取（#8）。会话不在列表里：名字用 id 前 8 位，项目名留空（同 Tauri 版兜底）
    pub fn names(&self, sid: &str, cx: &App) -> (String, String) {
        match self.state.read(cx).session(sid) {
            Some(m) => (
                crate::sidebar::groups::session_title(&m.display_name, &m.first_user_msg, &m.short_id),
                crate::sidebar::groups::project_name(&m.git_root, &m.cwd),
            ),
            None => (sid.chars().take(8).collect(), String::new()),
        }
    }

    // ---------- 已读 / 跳转 ----------

    /// 窗口聚焦时正摆在眼前的会话标已读（切标签 / 窗口回到前台，#215 问题 6）
    fn read_what_is_on_screen(&mut self, cx: &mut Context<Self>) {
        let sid = on_screen_session(&self.state.read(cx).workspace.state, self.window_active).map(String::from);
        if let Some(sid) = sid {
            let e = self.book.mark_read(&sid);
            self.apply(e, cx);
        }
    }

    /// 窗口焦点变了（`attach_window` 挂的观察者调）
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active == active {
            return;
        }
        self.window_active = active;
        if active {
            self.read_what_is_on_screen(cx);
        }
    }

    /// 给 A 包：用户在这个终端里打字了 = 看到了（#215）
    pub fn mark_session_read(&mut self, sid: &str, cx: &mut Context<Self>) {
        let e = self.book.mark_read(sid);
        self.apply(e, cx);
    }

    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let e = self.book.mark_all_read();
        self.apply(e, cx);
    }

    pub fn remove(&mut self, sid: &str, cx: &mut Context<Self>) {
        let e = self.book.remove(sid);
        self.apply(e, cx);
    }

    pub fn clear_all(&mut self, cx: &mut Context<Self>) {
        let e = self.book.clear_all();
        self.apply(e, cx);
    }

    /// 跳到这个会话（清单 G「跳转」）：标已读；已经在某个标签里 → 切过去并闪牌，不动侧栏；
    /// 不在任何标签里 → 侧栏定位。只定位，不打开会话
    pub fn navigate(&mut self, sid: &str, cx: &mut Context<Self>) {
        let e = self.book.mark_read(sid);
        self.apply(e, cx);
        let found = {
            let ws = &self.state.read(cx).workspace.state;
            collect_containers(&ws.root)
                .into_iter()
                .find_map(|c| c.tabs.iter().find(|t| t.session_id.as_deref() == Some(sid)).map(|t| (c.id.clone(), t.id.clone())))
        };
        match found {
            Some((cid, tid)) => {
                self.state.update(cx, |s, cx| {
                    s.workspace.tab_click(&cid, &tid);
                    s.workspace_changed(cx);
                });
                cx.emit(NotifyEvent::FlashPane { container_id: cid });
            }
            None => {
                if self.state.read(cx).session(sid).is_none() {
                    // #215 问题 7：Tauri 版这里静默失败。会话可能已归档 / 列表还没加载
                    log::info!(target: "notify", "跳转：会话 {sid} 不在列表里（已归档或还没加载）");
                }
                cx.emit(NotifyEvent::RevealSession { session_id: sid.to_string() });
            }
        }
    }

    // ---------- 授权（给 D 包设置面板） ----------

    pub fn request_permission(&self) {
        self.system.request_permission();
    }

    pub fn refresh_permission(&self) {
        self.system.refresh_permission();
    }

    /// 设置面板「发送测试通知」：受总开关控制（#215 问题 13）。返回是否真的交给了系统
    pub fn send_test(&self, cx: &App) -> bool {
        if !self.state.read(cx).prefs.notify.system || !self.system.available() {
            return false;
        }
        self.system.post("makit-selftest", &tr!("notify.selftest.title"), "", &tr!("notify.selftest.body"), false, false);
        true
    }

    // ---------- 抽屉 ----------

    /// 打开时默认高亮第一条未读，全部已读时高亮第一条（清单 G）
    fn initial_selection(&self) -> Option<usize> {
        if self.book.records.is_empty() {
            return None;
        }
        Some(self.book.records.iter().position(|r| !r.read).unwrap_or(0))
    }

    fn clamp_selection(&mut self) {
        let n = self.book.records.len();
        self.selected = if n == 0 { None } else { Some(self.selected.unwrap_or(0).min(n - 1)) };
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.open(window, cx);
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.selected = self.initial_selection();
        // 记下原来的焦点（通常是终端），关的时候还回去（Tauri 版 useRestoreFocusOnClose）
        self.restore_focus = window.focused(cx).filter(|f| *f != self.focus);
        self.focus.focus(window);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(f) = self.restore_focus.take() {
            f.focus(window);
        }
        cx.notify();
    }

    /// ↑↓ 循环移动
    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.book.records.len() as isize;
        if n == 0 {
            return;
        }
        let cur = self.selected.map(|i| i as isize).unwrap_or(if delta > 0 { -1 } else { 0 });
        let i = (cur + delta).rem_euclid(n) as usize;
        self.selected = Some(i);
        self.scroll.scroll_to_item(i);
        cx.notify();
    }

    /// Enter / 点一行：标已读、跳转、关抽屉
    pub fn activate_row(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sid) = self.book.records.get(idx).map(|r| r.session_id.clone()) else { return };
        self.close(window, cx);
        self.navigate(&sid, cx);
    }
}

/// hook 服务。**两个版本同时开时，谁后启动 hook 就给谁**（用户 2026-09-27 拍板，和现在行为一致）：
/// `makit_core::hook::start` 绑定前先删掉旧的 `~/.claude/makit/hook.sock`，于是后启动的一方拿到新 socket，
/// 先启动的一方还挂在已经被 unlink 的旧 inode 上，再也收不到连接（不报错）—— 它那边只剩状态文件兜底
/// （等待照样能通知，只是分不清审批 / 回答的原话、收不到 Stop）。
///
/// `MAKIT_NATIVE_NO_HOOK=1` 时不起（跑测试实例又不想抢 hook 时用；假 HOME 下 socket 本来就在假目录里，不会抢）。
fn start_hook_server(tx: futures::channel::mpsc::UnboundedSender<String>) {
    if std::env::var_os("MAKIT_NATIVE_NO_HOOK").is_some() {
        log::info!(target: "notify", "MAKIT_NATIVE_NO_HOOK：不起 hook 服务，只靠状态文件");
        return;
    }
    if let Some(p) = makit_core::hook::socket_path() {
        log::info!(target: "notify", "hook 服务：{}（后启动的一方拿到 hook）", p.display());
    }
    makit_core::hook::start(move |line| {
        let _ = tx.unbounded_send(line);
    });
}
