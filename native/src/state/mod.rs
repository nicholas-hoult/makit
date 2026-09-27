//! `AppState`：全应用共享的数据（GPUI 实体，挂成全局 `GlobalAppState`）。
//!
//! 装什么：会话列表（core watcher 回调 + 增量解析，替掉原型的 2s / 30s 轮询）、工作区模型、
//! 持久化的偏好（置顶 / 侧栏选项 / 主题 / 通知记录 / 各开关，`NativeState`）。
//! 不装什么：视图自己的临时状态（hover、滚动位置、终端实体……）留在各自的视图里。
//!
//! ## 怎么读
//! ```ignore
//! let state = AppState::global(cx);            // Entity<AppState>
//! let s = state.read(cx);
//! s.sessions / s.workspace.state / s.prefs.pinned_sessions …
//! ```
//! 视图在 `new` 里 `cx.observe(&state, |_, _, cx| cx.notify()).detach()`，数据一变就重画；
//! 要区分「什么变了」就 `cx.subscribe(&state, |this, _, ev: &AppEvent, cx| …)`。
//!
//! ## 怎么写
//! ```ignore
//! state.update(cx, |s, cx| {
//!     s.workspace.split(&cid, Dir::V);   // 改工作区
//!     s.workspace_changed(cx);           // → 通知 + 发 AppEvent::WorkspaceChanged + 防抖存盘
//! });
//! state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed ^= true)); // 改偏好 + 存盘
//! ```
//! 改完**一定**调对应的 `*_changed` / `update_prefs`，否则不存盘、别的视图也不知道。

pub mod sessions;

use std::collections::HashSet;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};
use makit_core::watcher::WatchEvent;
use makit_core::SessionMeta;

use crate::persist::{NativeState, Saver};
use crate::workspace::model::{default_workspace, Workspace};

/// AppState 发出的事件（`cx.subscribe` 收）。简单视图用 `cx.observe` 就够了。
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// 会话列表变了（全量 / 增量 / 运行状态）
    SessionsChanged,
    /// 工作区树变了（开关标签、分屏、切焦点……）
    WorkspaceChanged,
    /// 持久化的偏好变了
    PrefsChanged,
}

pub struct AppState {
    /// 全部会话，按 mtime 降序（最近在前）。`loaded` 之前是空的
    pub sessions: Vec<SessionMeta>,
    pub loaded: bool,
    /// 工作区模型（纯逻辑，见 workspace/model.rs）。持久化时写进 `prefs.workspace`
    pub workspace: Workspace,
    /// 其余持久化的偏好。**`prefs.workspace` 只在存盘时由 `workspace` 填**，平时别读它
    pub prefs: NativeState,
    /// 正在归档写盘的会话（这期间增量结果不覆盖它，同 TS 版的 archivingRef）
    pub archiving: HashSet<String>,
    saver: Option<Saver>,
}

impl EventEmitter<AppEvent> for AppState {}

pub struct GlobalAppState(pub Entity<AppState>);
impl Global for GlobalAppState {}

impl AppState {
    /// 建实体并挂成全局；启动全量扫描和目录监听。`saver` 为 None 时不写盘（测试用）
    pub fn init(mut prefs: NativeState, saver: Option<Saver>, cx: &mut App) -> Entity<Self> {
        let ws = prefs.workspace.take().unwrap_or_else(default_workspace);
        let state = cx.new(|cx| {
            let mut s = Self {
                sessions: Vec::new(),
                loaded: false,
                workspace: Workspace::new(ws),
                prefs,
                archiving: HashSet::new(),
                saver,
            };
            s.start_session_feed(cx);
            s
        });
        cx.set_global(GlobalAppState(state.clone()));
        state
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAppState>().0.clone()
    }

    /// 启动时全量扫一次，之后靠 watcher 增量：RunningChanged → 运行状态合并；
    /// SessionsChanged(路径) → 只解析这几个文件（再做 800ms 尾部合并，同 TS 版）。
    fn start_session_feed(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
        let (tx, mut rx) = unbounded::<WatchEvent>();
        makit_core::watcher::start(move |ev| {
            let _ = tx.unbounded_send(ev);
        });
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut running = false;
                let mut paths: Vec<String> = Vec::new();
                let mut absorb = |ev: WatchEvent| match ev {
                    WatchEvent::RunningChanged => running = true,
                    WatchEvent::SessionsChanged(p) => paths.extend(p),
                };
                absorb(first);
                // 尾部合并：会话活跃时事件每 500ms 一发，每次都要解析 jsonl + 跑 ps，标题同步不在乎这点延迟
                cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                while let Ok(ev) = rx.try_recv() {
                    absorb(ev);
                }
                if running {
                    let list = cx.background_executor().spawn(async { makit_core::running::list_running_sessions() }).await;
                    if this.update(cx, |s, cx| s.apply_running(list, cx)).is_err() {
                        break;
                    }
                }
                if !paths.is_empty() {
                    paths.sort();
                    paths.dedup();
                    let updated = cx
                        .background_executor()
                        .spawn(async move { makit_core::sessions::list_sessions_by_paths(paths, Some("smart".into())).unwrap_or_default() })
                        .await;
                    if this.update(cx, |s, cx| s.apply_updated(updated, cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    /// 全量重扫（启动、⌘R）
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let t = std::time::Instant::now();
            let list = cx
                .background_executor()
                .spawn(async { makit_core::sessions::list_sessions(Some("smart".into())).unwrap_or_default() })
                .await;
            crate::perf::mark("会话列表返回");
            eprintln!("[state] list_sessions: {} 条，{:?}", list.len(), t.elapsed());
            let _ = this.update(cx, |s, cx| {
                s.sessions = list;
                s.loaded = true;
                s.sessions_changed(cx);
            });
        })
        .detach();
    }

    fn apply_running(&mut self, list: Vec<makit_core::RunningMeta>, cx: &mut Context<Self>) {
        let archiving = self.archiving.clone();
        if sessions::merge_running(&mut self.sessions, &list, |s| archiving.contains(&s.session_id)) {
            self.sessions_changed(cx);
        }
    }

    fn apply_updated(&mut self, updated: Vec<SessionMeta>, cx: &mut Context<Self>) {
        let archiving = self.archiving.clone();
        if sessions::merge_updated(&mut self.sessions, updated, |id| archiving.contains(id)) {
            self.sessions_changed(cx);
        }
    }

    pub fn session(&self, id: &str) -> Option<&SessionMeta> {
        self.sessions.iter().find(|s| s.session_id == id)
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.prefs.pinned_sessions.iter().any(|p| p == id)
    }

    pub fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_prefs(cx, |p| {
            if let Some(i) = p.pinned_sessions.iter().position(|x| x == id) {
                p.pinned_sessions.remove(i);
            } else {
                p.pinned_sessions.push(id.to_string());
            }
        });
    }

    // ---- 改完之后的出口 ----

    pub fn sessions_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::SessionsChanged);
        cx.notify();
    }

    pub fn workspace_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::WorkspaceChanged);
        cx.notify();
        self.save();
    }

    pub fn update_prefs(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut NativeState)) {
        f(&mut self.prefs);
        cx.emit(AppEvent::PrefsChanged);
        cx.notify();
        self.save();
    }

    /// 当前全部持久化内容（工作区 + 偏好）
    pub fn snapshot(&self) -> NativeState {
        let mut s = self.prefs.clone();
        s.workspace = Some(self.workspace.state.clone());
        s
    }

    fn save(&self) {
        if let Some(saver) = &self.saver {
            saver.save(self.snapshot());
        }
    }

    /// 退出前调：立刻把防抖中的状态写掉
    pub fn flush(&self) {
        if let Some(saver) = &self.saver {
            saver.save(self.snapshot());
            saver.flush();
        }
    }
}
