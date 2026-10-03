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

pub mod binding;
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

/// 检测本机的 Claude Code / Codex，并把结果记进日志（出问题时能从日志里看到「当时检测到了什么」，#254）
fn detect_tools() -> makit_core::environment::ToolPresence {
    let t = makit_core::environment::detect();
    log::info!(
        target: "environment",
        "工具检测：Claude Code 命令 {} 会话目录 {:?}；Codex 命令 {} 会话目录 {:?}",
        t.claude_bin, t.claude_dir, t.codex_bin, t.codex_dir
    );
    t
}

pub struct AppState {
    /// 全部会话，按 mtime 降序（最近在前）。`loaded` 之前是空的
    pub sessions: Vec<SessionMeta>,
    pub loaded: bool,
    /// 本机有没有 Claude Code / Codex 的迹象（#197，没有会话时欢迎卡的提示用）。启动时测一次，会话列表变化时刷新
    pub tools: makit_core::environment::ToolPresence,
    /// 工作区模型（纯逻辑，见 workspace/model.rs）。持久化时写进 `prefs.workspace`
    pub workspace: Workspace,
    /// 其余持久化的偏好。**`prefs.workspace` 只在存盘时由 `workspace` 填**，平时别读它
    pub prefs: NativeState,
    /// 正在归档写盘的会话（这期间增量结果不覆盖它，同 TS 版的 archivingRef）
    pub archiving: HashSet<String>,
    saver: Option<Saver>,
    /// 上次读 `~/.claude/sessions/*.json` 绑标签的时间（running-changed 活跃时每 500ms 一发，2s 节流，同 Tauri）
    last_bind_attempt: Option<std::time::Instant>,
    /// 最近一次扫描会话失败的原因（同 Tauri App.tsx 的 error 状态，Root 画一条「加载失败」横幅）。
    /// 失败时不清空已有的 sessions——不能拿一次扫描失败把用户已经看到的会话列表清没
    pub load_error: Option<String>,
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
                tools: detect_tools(),
                workspace: Workspace::new(ws),
                prefs,
                archiving: HashSet::new(),
                saver,
                last_bind_attempt: None,
                load_error: None,
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
                let first_is_file = matches!(first, WatchEvent::SessionsChanged(_));
                absorb(first);
                // 尾部合并只给 jsonl 那条路（每次都要解析文件，标题同步不在乎这点延迟）；
                // 运行状态收到就处理，同 Tauri（running-changed 立即 list_running_sessions）
                if first_is_file {
                    cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                }
                while let Ok(ev) = rx.try_recv() {
                    absorb(ev);
                }
                if running {
                    let list = cx.background_executor().spawn(async { makit_core::running::list_running_sessions() }).await;
                    if this
                        .update(cx, |s, cx| {
                            s.apply_running(list, cx);
                            s.bind_pending_tabs(false, cx);
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                if !paths.is_empty() {
                    paths.sort();
                    paths.dedup();
                    let updated = cx
                        .background_executor()
                        .spawn(async move { makit_core::sessions::list_sessions_by_paths(paths, Some("smart".into())) })
                        .await;
                    let ok = this.update(cx, |s, cx| match updated {
                        Ok(list) => {
                            s.load_error = None;
                            s.apply_updated(list, cx);
                            // codex 不写 ~/.claude/sessions，运行状态那条路触发不了绑定；它的会话文件一变就试一次（有 2 秒节流）
                            s.bind_pending_tabs(false, cx);
                        }
                        // 增量解析失败：不动现有列表，只记下原因，等下一次成功的扫描自然清掉
                        Err(e) => s.load_error = Some(e),
                    });
                    if ok.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        // 存活巡检：进程被 kill / 崩溃 / 终端被关时不会有任何文件事件（`sessions/<pid>.json` 没人删），
        // 靠监听永远等不到「已停止」。有会话在跑时每 3 秒重读一次运行状态（读的都是几十字节的小文件，
        // 死进程会被 `list_running_sessions` 过滤掉），没有在跑的会话就什么都不做
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            let any_running = match this.update(cx, |s, _| s.sessions.iter().any(|m| m.running && m.pid > 0)) {
                Ok(v) => v,
                Err(_) => break,
            };
            if !any_running {
                continue;
            }
            let list = cx.background_executor().spawn(async { makit_core::running::list_running_sessions() }).await;
            if this.update(cx, |s, cx| s.apply_running(list, cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    /// 全量重扫（启动、⌘R）
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let t = std::time::Instant::now();
            let result = cx
                .background_executor()
                .spawn(async { makit_core::sessions::list_sessions(Some("smart".into())) })
                .await;
            crate::perf::mark("会话列表返回");
            match &result {
                Ok(list) => log::debug!(target: "state", "list_sessions: {} 条，{:?}", list.len(), t.elapsed()),
                Err(e) => log::error!(target: "state", "list_sessions 失败：{e}"),
            }
            let _ = this.update(cx, |s, cx| {
                let first = !s.loaded;
                s.load_error = sessions::apply_full_scan(&mut s.sessions, result);
                s.loaded = true;
                s.sessions_changed(cx);
                if first {
                    s.bind_pending_tabs(true, cx);
                }
            });
        })
        .detach();
    }

    fn apply_running(&mut self, list: Vec<makit_core::RunningMeta>, cx: &mut Context<Self>) {
        let archiving = self.archiving.clone();
        // 已归档的不合并运行状态（同 Tauri mergeRunning 的 `s.archived || archivingRef`）
        if sessions::merge_running(&mut self.sessions, &list, |s| s.archived || archiving.contains(&s.session_id)) {
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
        // 只在还没有会话时刷新检测（有会话就不会显示提示；几次 stat，很便宜）
        if self.sessions.is_empty() {
            self.tools = detect_tools();
        }
        cx.emit(AppEvent::SessionsChanged);
        cx.notify();
        self.bind_running_tabs(cx);
    }

    /// 运行中会话的 `pty_id` 等于某个未绑定标签的 id → 绑上（同 Tauri App.tsx 的 `[sessions]` effect）
    fn bind_running_tabs(&mut self, cx: &mut Context<Self>) {
        let pending = binding::pending_tabs(&self.workspace.state.root);
        if pending.is_empty() {
            return;
        }
        let hits: Vec<(binding::PendingTab, String, String, String)> = binding::match_running(&pending, &self.sessions)
            .into_iter()
            .map(|(p, s)| (p, s.session_id.clone(), s.short_id.clone(), s.cwd.clone()))
            .collect();
        if hits.is_empty() {
            return;
        }
        for (p, sid, short, cwd) in &hits {
            self.workspace.bind_session_to_tab(&p.container_id, &p.tab_id, sid, short, None, Some(cwd));
        }
        self.workspace_changed(cx);
    }

    /// 还没发第一条消息（jsonl 不存在、不在会话列表里）的 claude：读 `~/.claude/sessions/<pid>.json`
    /// （core `resolve_pty_bindings`，同 Tauri `tryBindPtyTabs`）。没有待绑定标签就什么都不做；2s 节流，`force` 跳过节流
    pub fn bind_pending_tabs(&mut self, force: bool, cx: &mut Context<Self>) {
        let pending = binding::pending_tabs(&self.workspace.state.root);
        if pending.is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        if !force && self.last_bind_attempt.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(2)) {
            return;
        }
        self.last_bind_attempt = Some(now);
        let ids: Vec<String> = pending.iter().map(|p| p.tab_id.clone()).collect();
        cx.spawn(async move |this, cx| {
            // claude 靠 ~/.claude/sessions/<pid>.json；codex 靠进程环境里的 MAKIT_PTY_ID + 它开着的 rollout 文件
            let bindings = cx
                .background_executor()
                .spawn(async move {
                    let mut b = makit_core::running::resolve_pty_bindings(ids.clone());
                    b.extend(makit_core::running::codex_pty_bindings(&ids));
                    b
                })
                .await;
            if bindings.is_empty() {
                return;
            }
            let _ = this.update(cx, |s, cx| {
                // 等后台跑完的这段时间里标签可能已经被别的路径绑上 / 关掉：以现在的工作区为准
                let still = binding::pending_tabs(&s.workspace.state.root);
                let mut changed = false;
                for b in bindings {
                    let Some(p) = still.iter().find(|p| p.tab_id == b.pty_id) else { continue };
                    let cwd = s.session(&b.session_id).map(|m| m.cwd.clone());
                    let label = (!b.name.is_empty()).then_some(b.name.as_str());
                    s.workspace.bind_session_to_tab(&p.container_id, &p.tab_id, &b.session_id, &b.short_id, label, cwd.as_deref());
                    changed = true;
                }
                if changed {
                    s.workspace_changed(cx);
                }
            });
        })
        .detach();
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
