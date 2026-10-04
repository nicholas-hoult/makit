//! 工作区视图：把 `AppState.workspace`（递归分割树，纯逻辑在 model.rs）画出来，并持有每个标签的终端实体。
//!
//! 对照源：`WorkspaceView.tsx`（布局 / 分割线 / 拖放）、`ContainerView.tsx`（标签条 / 欢迎卡）、
//! `App.tsx`（标题栏、闪牌、⌘数字 / ⌥⌘ 导航、右键菜单）、App.css 同名段落（尺寸 / 颜色 / 动画全照它的数值）。
//!
//! | 文件 | 内容 |
//! |---|---|
//! | `model.rs` | 递归分割树 + useWorkspace 的各个 handler（纯逻辑） |
//! | `drop.rs` | 落点四区、插入线、路径引号（纯逻辑） |
//! | `flash.rs` | pane 闪牌的图标套 / 名字 / 关键帧（纯逻辑） |
//! | `labels.rs` | 标题栏文字、标签标题、状态点（纯逻辑） |
//! | `splitter.rs` | 分割线 ratio、命中区；`PaneResizing` 全局（给终端包的行列分离接口） |
//! | `welcome.rs` | 欢迎卡（键位读单一快捷键表） |
//! | `titlebar.rs` | 自绘标题栏（铃铛 / 侧栏分隔线的挂点） |
//! | `dnd.rs` | 拖拽载荷（`TabDrag` / `SessionDrag`）+ 拖动影子 |
//! | `menu.rs` | 右键菜单的临时最小实现（等 D 包的通用组件） |
//! | `selftest.rs` | 无人值守自检 `MAKIT_NATIVE_SELFTEST=workspace` |
//!
//! 约定：
//! - 改树一律 `state.update(cx, |s, cx| { s.workspace.xxx(..); s.workspace_changed(cx) })`，
//!   这个视图 observe 了 AppState，会自己重画、自己把焦点给当前标签的终端。
//! - 关标签返回的 tab id 必须交给 `shutdown_tabs` 杀终端（模型不碰终端）。
//! - 布局用 flex 按 ratio 等比排（和 TS 的绝对定位等价：RESIZER_PX = 0），分割线是叠在 split 上的 10px 抓取盒。
//!   最大化时只画那一个 container；别的 container 的终端实体留在 `terminals` 里不销毁（= TS 的 display:none 保活）。

pub mod dnd;
pub mod drop;
pub mod empty_hint;
pub mod flash;
pub mod labels;
pub mod model;
pub mod selftest;
pub mod splitter;
pub mod titlebar;
pub mod welcome;

use crate::tr;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, point, prelude::*, px, relative, svg, Animation, AnimationExt, AnyElement, App, BoxShadow, ClickEvent, Context,
    DragMoveEvent, Empty, Entity, ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Size, Subscription, WeakEntity, Window,
};

use crate::actions::workspace as act;
use crate::state::AppState;
use crate::terminal::{SearchResults, SpawnSpec, TerminalEvent, TerminalSpawnEvent, TerminalView, UserInput};
use crate::theme::{ActiveTheme, Theme};
use dnd::{SessionDrag, TabDrag};
use drop::{accepts_pane_drop, insert_marker, is_tab_drag, overlay_fraction, tab_insert_index, DragKind};
use labels::{kind_icon, tab_status, tab_title, TabStatus};
use model::{
    collect_containers, find_container, find_nearest_container, layout_tree, resume_cmd, split_id, tabs_to_close, CloseScope,
    ContainerNode, Dir, Direction, LayoutNode, Rect, Side, SplitNode, TabKind,
};
use splitter::{drag_ratio, PaneResizing, HIT_PX};

/// 标签条高度（`.container-tab-bar { flex: 0 0 28px }`）
pub const TAB_BAR_H: f32 = 28.0;

struct Term {
    view: Entity<TerminalView>,
    _subs: Vec<Subscription>,
}

/// 拖分割线时 on_drag 的载荷（只用来让 GPUI 进入拖拽状态、保持光标）
#[derive(Clone)]
struct SplitDrag {
    id: String,
}

struct SplitDragStart {
    id: String,
    dir: Dir,
    start_pos: f32,
    start_ratio: f64,
}

struct Flash {
    container_id: String,
    icon: Option<&'static str>,
    name: String,
    /// 这是第几个 pane（1–9，对应 ⌥⌘N）；超过 9 个没有数字键，不显示提示
    number: Option<usize>,
    seq: u64,
}

#[derive(Clone)]
enum MenuTarget {
    Pane { cid: String },
    Tab { cid: String, tid: String },
}

pub struct WorkspaceView {
    state: Entity<AppState>,
    terminals: HashMap<String, Term>,
    focus: FocusHandle,
    /// 上次把焦点给了哪个标签：当前标签变了才重新给（不和侧栏 / 浮层抢焦点）
    focused_tab: Option<String>,
    _observe: Subscription,
    activation_sub: Option<Subscription>,
    /// 工作区的实际尺寸（⌥⌘方向键按真实比例算几何位置，同 TS 读 `.workspace-view` 的 clientWidth / Height）
    size: Rc<Cell<Size<Pixels>>>,
    split_drag: Option<SplitDragStart>,
    hover_split: Option<String>,
    /// 拖到哪个 pane 的哪一侧（落点浮层）
    hover_drop: Option<(String, Dir, Side)>,
    /// 标签拖到哪个 container 的第几个位置（插入线）
    tab_drop: Option<(String, usize)>,
    dragging_tab: Option<String>,
    flash: Option<Flash>,
    flash_seq: u64,
    /// 每个 container 的标签条滚动 + 上次滚到的 active 标签（active 变了才滚）
    tab_scroll: HashMap<String, (ScrollHandle, String)>,
    /// pane / tab 右键菜单要用 D 包的通用组件（`overlays::show_context_menu`），它的 build 闭包
    /// 只给 `&App`，没有 `cx.entity()` 可拿——自己留一份弱引用
    self_weak: WeakEntity<Self>,
    /// 每个 pane 上一帧的窗口坐标矩形（⌘F 搜索条贴当前 pane 的右上角）
    pane_bounds: Rc<RefCell<HashMap<String, gpui::Bounds<Pixels>>>>,
    /// 启动目录不在的标签各自带一个恢复选择器（#239），挂在那块 pane 的底部
    recover_pickers: HashMap<String, (Entity<crate::overlays::recover::RecoverPicker>, Subscription)>,
    /// 可重排视图（#231）：每个「绑定了会话」的标签一个，开关关掉 / 会话解绑时拆掉
    hybrids: HashMap<String, Entity<crate::transcript_view::hybrid::HybridView>>,
    /// 欢迎卡「查看全部快捷键」有没有被点开过（#256 A4）。不持久化：每次重开都从折叠开始，对新用户更合适
    shortcuts_expanded: bool,
}

impl WorkspaceView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        cx.on_app_quit(|this, cx| {
            this.shutdown_all(cx);
            async {}
        })
        .detach();
        cx.set_global(PaneResizing(false));
        Self {
            state,
            terminals: HashMap::new(),
            focus: cx.focus_handle(),
            focused_tab: None,
            _observe: observe,
            activation_sub: None,
            pane_bounds: Rc::default(),
            recover_pickers: HashMap::new(),
            hybrids: HashMap::new(),
            shortcuts_expanded: false,
            size: Rc::new(Cell::new(Size { width: px(1000.0), height: px(600.0) })),
            split_drag: None,
            hover_split: None,
            hover_drop: None,
            tab_drop: None,
            dragging_tab: None,
            flash: None,
            flash_seq: 0,
            tab_scroll: HashMap::new(),
            self_weak: cx.entity().downgrade(),
        }
    }

    fn update_ws(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut model::Workspace) -> Vec<String>) {
        let closed = self.state.update(cx, |s, cx| {
            let closed = f(&mut s.workspace);
            s.workspace_changed(cx);
            closed
        });
        self.shutdown_tabs(&closed, cx);
    }

    /// 杀掉这些标签的终端（关标签 / 关 pane 之后调）
    pub fn shutdown_tabs(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            self.recover_pickers.remove(id);
            self.hybrids.remove(id);
            if let Some(t) = self.terminals.remove(id) {
                t.view.update(cx, |v, _| v.shutdown());
            }
        }
    }

    /// 启动目录不在：给这个标签出恢复选择器（#239）。同一个标签已经有就把焦点给它
    pub fn show_recover(&mut self, tab_id: String, cwd: String, session_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((p, _)) = self.recover_pickers.get(&tab_id) {
            window.focus(&p.read(cx).focus_handle());
            return;
        }
        let is_active = self.state.read(cx).workspace.active_tab().is_some_and(|t| t.id == tab_id);
        let state = self.state.clone();
        let tid = tab_id.clone();
        let picker = cx.new(|cx| crate::overlays::recover::RecoverPicker::new(state, tid, cwd, session_id, is_active, window, cx));
        let tid = tab_id.clone();
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Self, _, ev: &crate::overlays::recover::RecoverEvent, _, cx| match ev {
            crate::overlays::recover::RecoverEvent::Close => {
                this.recover_pickers.remove(&tid);
                // 焦点还给终端：清掉「上次给过谁」，下一帧 sync_focus 重新给
                this.focused_tab = None;
                cx.notify();
            }
        });
        self.recover_pickers.insert(tab_id, (picker, sub));
        cx.notify();
    }

    /// 按「全局开关 + 这个标签有没有绑定会话」决定要不要给它一个可重排视图（#231 TRD §17）：
    /// 开关开着且绑定了会话 → 有（会话 id 变了就重建，读新会话的文件）；否则拆掉并让终端回到整块画
    fn sync_hybrid(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let want = {
            let s = self.state.read(cx);
            s.prefs.reflow_view.then(|| s.workspace.locate_tab(tab_id).and_then(|(_, t)| t.session_id.clone())).flatten()
        };
        let have = self.hybrids.get(tab_id).map(|h| h.read(cx).session_id.clone());
        if want == have {
            return;
        }
        if let Some(old) = self.hybrids.remove(tab_id) {
            let term = old.read(cx).terminal();
            term.update(cx, |t, _| {
                t.active_only = false;
                t.layout_h_override = None;
            });
        }
        if let (Some(sid), Some(t)) = (want, self.terminals.get(tab_id)) {
            let term = t.view.clone();
            let h = cx.new(|cx| crate::transcript_view::hybrid::HybridView::new(term, sid, cx));
            self.hybrids.insert(tab_id.to_string(), h);
        }
    }

    /// 自检用：这个标签的恢复选择器 (选项数, 选中项, 忙)
    /// 自检：欢迎卡「查看全部快捷键」的折叠状态（#256 A4）
    pub fn debug_shortcuts_expanded(&self) -> bool {
        self.shortcuts_expanded
    }

    /// 自检：点一下「查看全部快捷键」/「收起快捷键」
    pub fn debug_toggle_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts_expanded = !self.shortcuts_expanded;
        cx.notify();
    }

    pub fn debug_recover(&self, tab_id: &str, cx: &App) -> Option<(usize, usize, bool)> {
        self.recover_pickers.get(tab_id).map(|(p, _)| p.read(cx).debug_state())
    }

    fn shutdown_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.terminals.keys().cloned().collect();
        self.shutdown_tabs(&ids, cx);
    }

    fn active_container_id(&self, cx: &App) -> String {
        self.state.read(cx).workspace.state.active_container_id.clone()
    }

    fn pane_count(&self, cx: &App) -> usize {
        collect_containers(&self.state.read(cx).workspace.state.root).len()
    }

    // ---- 动作（根视图把全局 action 转发到这里）----

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.new_shell_in(&cid, cx);
    }

    pub fn new_shell_in(&mut self, cid: &str, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.new_shell_in(cid);
            vec![]
        });
    }

    pub fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        let Some((cid, tid)) = self.state.read(cx).workspace.active_container().map(|c| (c.id.clone(), c.active_tab_id.clone())) else { return };
        if tid.is_empty() {
            return;
        }
        self.close_tab(&cid, &tid, cx);
    }

    /// 关标签的唯一入口（⌘W / × / 右键「关闭」/ 终端退出）：先杀掉会话记下的逃出进程组的子进程，再关（#171 同形）
    pub fn close_tab(&mut self, cid: &str, tid: &str, cx: &mut Context<Self>) {
        let pids: Vec<u32> = {
            let s = self.state.read(cx);
            find_container(&s.workspace.state.root, cid)
                .and_then(|c| c.tabs.iter().find(|t| t.id == tid))
                .and_then(|t| t.session_id.as_deref())
                .and_then(|sid| s.session(sid))
                .map(|m| m.child_processes.iter().map(|p| p.pid).collect())
                .unwrap_or_default()
        };
        if !pids.is_empty() {
            makit_core::process::kill_pids(&pids);
        }
        self.update_ws(cx, |w| w.close_tab(cid, tid));
    }

    /// 批量关（关闭其他 / 关闭右侧）：倒序关，中间态不让 active 在剩下的标签之间来回跳
    fn close_tabs(&mut self, cid: &str, tids: &[String], cx: &mut Context<Self>) {
        for tid in tids.iter().rev() {
            self.close_tab(cid, tid, cx);
        }
    }

    pub fn split(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.split_container(&cid, dir, cx);
    }

    pub fn split_container(&mut self, cid: &str, dir: Dir, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.split(cid, dir);
            vec![]
        });
    }

    pub fn cycle_tab(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.cycle_tab(delta);
            vec![]
        });
    }

    pub fn activate_nth_tab(&mut self, n: usize, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.activate_nth_tab(n);
            vec![]
        });
    }

    /// ⌥⌘1–9：第 n 个 pane，并闪牌
    pub fn focus_nth_pane(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(id) = self.state.read(cx).workspace.nth_container(n) else { return };
        self.update_ws(cx, |w| {
            w.set_active(&id);
            vec![]
        });
        self.flash_container(&id, cx);
    }

    /// ⌥⌘方向键：几何方向切 pane（按工作区真实尺寸算），并闪牌；只有 1 个 pane 时不动
    pub fn focus_pane_dir(&mut self, dir: Direction, cx: &mut Context<Self>) {
        if self.pane_count(cx) <= 1 {
            return;
        }
        let sz = self.size.get();
        let target = {
            let w = &self.state.read(cx).workspace.state;
            let l = layout_tree(&w.root, Rect { x: 0.0, y: 0.0, width: f64::from(sz.width).max(1.0), height: f64::from(sz.height).max(1.0) });
            find_nearest_container(&l, &w.active_container_id, dir)
        };
        if let Some(id) = target {
            self.update_ws(cx, |w| {
                w.set_active(&id);
                vec![]
            });
            self.flash_container(&id, cx);
        }
    }

    pub fn toggle_maximize(&mut self, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.toggle_maximize_container(&cid, cx);
    }

    fn toggle_maximize_container(&mut self, cid: &str, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.toggle_maximize(cid);
            vec![]
        });
    }

    fn set_active(&mut self, cid: &str, cx: &mut Context<Self>) {
        if self.active_container_id(cx) != cid {
            self.update_ws(cx, |w| {
                w.set_active(cid);
                vec![]
            });
        }
    }

    /// 目标 pane 中央浮出「图标 + 名字」的牌，0.7s 淡出；连续触发直接替换。E 通知包「从通知跳转」也调它
    pub fn flash_container(&mut self, cid: &str, cx: &mut Context<Self>) {
        let (icon, name, number) = {
            let s = self.state.read(cx);
            let cs = collect_containers(&s.workspace.state.root);
            let Some(idx) = cs.iter().position(|c| c.id == cid) else { return };
            let c = cs[idx];
            let title = c.tabs.iter().find(|t| t.id == c.active_tab_id).map(|t| {
                let meta = t.session_id.as_deref().and_then(|id| s.session(id));
                tab_title(t, meta)
            });
            (flash::flash_icon(&s.prefs.pane_icons, idx), flash::flash_name(title.as_deref()), (idx < 9).then_some(idx + 1))
        };
        self.flash_seq += 1;
        let seq = self.flash_seq;
        self.flash = Some(Flash { container_id: cid.to_string(), icon, name, number, seq });
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(Duration::from_millis(flash::FLASH_MS)).await;
            let _ = this.update(cx, |v, cx| {
                if v.flash.as_ref().map(|f| f.seq) == Some(seq) {
                    v.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- 终端 ----

    fn ensure_terminal(&mut self, c: &ContainerNode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = c.tabs.iter().find(|t| t.id == c.active_tab_id) else { return };
        if self.terminals.contains_key(&tab.id) {
            return;
        }
        let spec = SpawnSpec {
            pty_id: Some(tab.id.clone()),
            cwd: Some(tab.cwd.clone()).filter(|c| !c.is_empty()),
            init_command: tab.init_command.clone(),
            session_id: tab.session_id.clone(),
            kind: match tab.kind {
                TabKind::Resume => "resume",
                TabKind::New => "new",
                TabKind::Shell => "shell",
            },
            clicked_at: Instant::now(),
        };
        let tab_id = tab.id.clone();
        let view = cx.new(|cx| TerminalView::new(spec, window, cx));
        let tid = tab_id.clone();
        let tid_input = tab_id.clone();
        let sub = cx.subscribe(&view, |_: &mut Self, _, ev: &TerminalEvent, cx| match ev {
            // 进程退出不关标签（同 Tauri）：终端里已经写了「[进程已退出]」，留着能看 claude 最后的输出，用户自己 ⌘W 关
            TerminalEvent::Exited => cx.notify(),
            TerminalEvent::TitleChanged => cx.notify(),
        });
        // 启动目录：不在了 → D 的恢复对话框；按会话起始目录校正了 → 写回标签的 cwd
        let spawn_sub = cx.subscribe_in(&view, window, move |this: &mut Self, _, ev: &TerminalSpawnEvent, window, cx| match ev {
            TerminalSpawnEvent::CwdMissing(dir) => crate::overlays::recover::cwd_missing(&tid, dir, window, cx),
            TerminalSpawnEvent::CwdCorrected(dir) => this.state.update(cx, |s, cx| {
                s.workspace.update_tab_cwd(&tid, dir);
                s.workspace_changed(cx);
            }),
        });
        // 在某个会话的终端里打字 = 看过它的通知（E）。会话 id 按标签**现在**的绑定查：
        // shell / 新建标签里手动起的 claude 是后绑上的，终端启动时的 spec 里没有它
        let input_sub = cx.subscribe(&view, move |this: &mut Self, _, ev: &UserInput, cx| {
            let bound = this.state.read(cx).workspace.locate_tab(&tid_input).and_then(|(_, t)| t.session_id.clone());
            if let Some(sid) = bound.or_else(|| ev.session_id.clone()) {
                crate::notify::Notifier::global(cx).update(cx, |n, cx| n.mark_session_read(&sid, cx));
            }
        });
        // 搜索进度 → D 的搜索条计数
        let search_sub = cx.subscribe(&view, |_, _, ev: &SearchResults, cx| {
            crate::overlays::search_bar::report_progress(Some(crate::overlays::search_count::SearchProgress { index: ev.index, count: ev.count }), cx);
        });
        self.terminals.insert(tab.id.clone(), Term { view, _subs: vec![sub, spawn_sub, input_sub, search_sub] });
    }

    fn title_of(&self, tab: &model::PaneTab, cx: &App) -> String {
        let s = self.state.read(cx);
        tab_title(tab, tab.session_id.as_deref().and_then(|id| s.session(id)))
    }

    // ---- 拖放 ----

    fn on_tab_bar_drop(&mut self, cid: &str, d: &TabDrag, cx: &mut Context<Self>) {
        let idx = self.tab_drop.take().filter(|(c, _)| c == cid).map(|(_, i)| i);
        if d.container_id == cid {
            // 同 container 内重排；没落在某个标签上（idx 为空）就不动
            if let Some(i) = idx {
                let tid = d.tab_id.clone();
                self.update_ws(cx, |w| {
                    w.reorder_tab(cid, &tid, i);
                    vec![]
                });
            }
        } else {
            let (src, tid) = (d.container_id.clone(), d.tab_id.clone());
            self.update_ws(cx, |w| {
                w.move_tab(&src, &tid, cid, idx);
                vec![]
            });
        }
    }

    fn on_pane_drag_move(&mut self, cid: &str, kind: DragKind, bounds: gpui::Bounds<Pixels>, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if !accepts_pane_drop(kind) {
            return;
        }
        let next = if bounds.contains(&pos) {
            let (dir, side) = drop::drop_zone(
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                f32::from(pos.x - bounds.origin.x),
                f32::from(pos.y - bounds.origin.y),
            );
            Some((cid.to_string(), dir, side))
        } else if self.hover_drop.as_ref().is_some_and(|(c, _, _)| c == cid) {
            None
        } else {
            return;
        };
        if next != self.hover_drop {
            self.hover_drop = next;
            cx.notify();
        }
    }

    fn drop_zone_of(&mut self, cid: &str) -> Option<(Dir, Side)> {
        self.hover_drop.take().filter(|(c, _, _)| c == cid).map(|(_, d, s)| (d, s))
    }

    /// Finder 拖文件进来：在这个 pane 当前标签的命令行里插路径（不分屏），然后焦点跟过去。
    /// 走 `insert_paths`（= 粘贴语义，按程序状态决定括号粘贴），不走 IME 上屏那条路——
    /// 文件名可以带换行，裸写进 PTY 的话那个换行就是回车，会把半截命令直接执行掉（#226 修）
    fn on_files_drop(&mut self, cid: &str, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        if paths.paths().is_empty() {
            return;
        }
        let tid = find_container(&self.state.read(cx).workspace.state.root, cid).map(|c| c.active_tab_id.clone());
        let Some(view) = tid.and_then(|t| self.terminals.get(&t)).map(|t| t.view.clone()) else { return };
        let list = paths.paths().to_vec();
        view.update(cx, |v, cx| v.insert_paths(&list, cx));
        self.set_active(cid, cx);
        view.read(cx).focus_handle(cx).focus(window);
    }

    // ---- 右键菜单 ----

    /// D 包的通用右键菜单组件（Esc 关、点外面关、关掉后焦点还回去，都是它管）。
    /// build 闭包每次弹菜单只给 `&App`，没有 `cx.entity()`，用 `self_weak` 代替
    fn menu_items(&self, target: &MenuTarget, cx: &App) -> Vec<crate::overlays::MenuItem> {
        use crate::overlays::{show_toast, toast, MenuItem as OM};
        let w = self.self_weak.clone();
        let s = self.state.read(cx);
        match target {
            MenuTarget::Pane { cid } => {
                let Some(c) = find_container(&s.workspace.state.root, cid) else { return vec![] };
                let (c1, c2, c3, c4, tid) = (cid.clone(), cid.clone(), cid.clone(), cid.clone(), c.active_tab_id.clone());
                let (w1, w2, w3, w4) = (w.clone(), w.clone(), w.clone(), w);
                let tid_empty = tid.is_empty();
                vec![
                    OM::action(tr!("workspace.menu.split_right"), move |_, cx| drop(w1.update(cx, |v, cx| v.split_container(&c1, Dir::V, cx)))),
                    OM::action(tr!("workspace.menu.split_down"), move |_, cx| drop(w2.update(cx, |v, cx| v.split_container(&c2, Dir::H, cx)))),
                    OM::action(tr!("workspace.menu.new_terminal"), move |_, cx| drop(w3.update(cx, |v, cx| v.new_shell_in(&c3, cx)))),
                    OM::separator(),
                    // 明确写「当前」：这个菜单是在 pane 上右键弹的，没有「某个 tab」可指
                    OM::action(tr!("workspace.menu.close_current"), move |_, cx| drop(w4.update(cx, |v, cx| v.close_tab(&c4, &tid, cx)))).disabled(tid_empty),
                ]
            }
            MenuTarget::Tab { cid, tid } => {
                let Some(c) = find_container(&s.workspace.state.root, cid) else { return vec![] };
                let Some(tab) = c.tabs.iter().find(|t| &t.id == tid) else { return vec![] };
                let others = tabs_to_close(&c.tabs, tid, CloseScope::Others);
                let right = tabs_to_close(&c.tabs, tid, CloseScope::Right);
                let meta = tab.session_id.as_deref().and_then(|id| s.session(id));
                // 会话的真实 cwd 优先：用户可能在终端里 cd 走了
                let cwd = meta
                    .map(|m| if !m.last_cwd.is_empty() { m.last_cwd.clone() } else { m.cwd.clone() })
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| tab.cwd.clone());
                let resume = tab.session_id.as_deref().map(|sid| format!("cd {cwd} && {}", resume_cmd(sid, meta.map(|m| m.tool.as_str()).filter(|t| !t.is_empty()))));
                let single = c.tabs.len() <= 1;
                let (a, b) = (cid.clone(), tid.clone());
                let w1 = w.clone();
                let close_one = move |_: &mut Window, cx: &mut App| drop(w1.update(cx, |v, cx| v.close_tab(&a, &b, cx)));
                let (a, w2, oth) = (cid.clone(), w.clone(), others.clone());
                let close_others = move |_: &mut Window, cx: &mut App| drop(w2.update(cx, |v, cx| v.close_tabs(&a, &oth, cx)));
                let (a, w3, rt) = (cid.clone(), w.clone(), right.clone());
                let close_right = move |_: &mut Window, cx: &mut App| drop(w3.update(cx, |v, cx| v.close_tabs(&a, &rt, cx)));
                let mv = |dir: Dir| {
                    let (a, b, w) = (cid.clone(), tid.clone(), w.clone());
                    move |_: &mut Window, cx: &mut App| {
                        drop(w.update(cx, |v, cx| {
                            v.update_ws(cx, |ws| {
                                ws.split_with_tab(&a, &b, &a, dir, Side::After);
                                vec![]
                            })
                        }))
                    }
                };
                let (p1, p2) = (cwd.clone(), cwd.clone());
                let resume_has = resume.is_some();
                vec![
                    OM::action(tr!("workspace.menu.close"), close_one),
                    OM::action(tr!("workspace.menu.close_others"), close_others).disabled(others.is_empty()),
                    OM::action(tr!("workspace.menu.close_right"), close_right).disabled(right.is_empty()),
                    OM::separator(),
                    // 只有一个 tab 时禁用：拆出去还是「一个 pane 一个 tab」，而且源 container 会先被摘掉
                    OM::action(tr!("workspace.menu.move_right"), mv(Dir::V)).disabled(single),
                    OM::action(tr!("workspace.menu.move_down"), mv(Dir::H)).disabled(single),
                    OM::separator(),
                    OM::action(tr!("sidebar.menu.reveal"), move |_, _| {
                        let _ = makit_core::paths::open_path(p1.clone(), true);
                    }),
                    OM::action(tr!("sidebar.menu.copy_path"), move |_, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(p2.clone()));
                        show_toast(tr!("common.copied"), toast::COPY_OK, cx);
                    }),
                    OM::action(tr!("sidebar.menu.copy_resume"), move |_, cx| {
                        if let Some(r) = &resume {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(r.clone()));
                            show_toast(tr!("common.copied"), toast::COPY_OK, cx);
                        }
                    })
                    .disabled(!resume_has),
                ]
            }
        }
    }

    // ---- 渲染 ----

    fn render_node(&mut self, node: &LayoutNode, maximized: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match node {
            LayoutNode::Container(c) => self.render_container(c, window, cx),
            LayoutNode::Split(s) => self.render_split(s, maximized, window, cx),
        }
    }

    fn render_split(&mut self, s: &SplitNode, maximized: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let a = self.render_node(&s.a, maximized, window, cx);
        let b = self.render_node(&s.b, maximized, window, cx);
        let theme = cx.theme().clone();
        let id = split_id(s);
        let (ra, rb) = (s.ratio as f32, 1.0 - s.ratio as f32);
        let hot = self.hover_split.as_deref() == Some(id.as_str()) || self.split_drag.as_ref().is_some_and(|d| d.id == id);
        let dir = s.dir;
        // 可见的线画在边界「前一个」pane 那一侧 1px（App.css `.pane-resizer-v::before` 的 translateX(-100%)）；
        // 上下分常态不画线（分界靠标签条的色阶），hover / 拖动时两个方向都亮 accent
        let line_color = if hot { Some(theme.accent) } else if dir == Dir::V { Some(theme.border) } else { None };
        let line = match dir {
            Dir::V => div().absolute().top_0().bottom_0().left(relative(ra)).ml(px(-1.0)).w(px(1.0)),
            Dir::H => div().absolute().left_0().right_0().top(relative(ra)).mt(px(-1.0)).h(px(1.0)),
        }
        .when_some(line_color, |d, c| d.bg(c));
        // 抓取盒：左右分时 10px 宽、从标签条下面开始（TS 里标签条 z-index 高过分割线，标签条那一截点不到分割线）；
        // 上下分时只有边界上方 5px（下方 5px 被下面 pane 的标签条盖住，同 TS）
        let half = (HIT_PX / 2.0) as f32;
        let handle = match dir {
            Dir::V => div().absolute().top(px(TAB_BAR_H)).bottom_0().left(relative(ra)).ml(px(-half)).w(px(HIT_PX as f32)).cursor_ew_resize(),
            Dir::H => div().absolute().left_0().right_0().top(relative(ra)).mt(px(-half)).h(px(half)).cursor_ns_resize(),
        };
        let (id1, id2, id3) = (id.clone(), id.clone(), id.clone());
        let ratio = s.ratio;
        let handle = handle
            .id(SharedString::from(format!("resizer-{id}")))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered { Some(id1.clone()) } else { None };
                if *hovered || this.hover_split.as_deref() == Some(id1.as_str()) {
                    this.hover_split = next;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let pos = if dir == Dir::V { ev.position.x } else { ev.position.y };
                    this.split_drag = Some(SplitDragStart { id: id2.clone(), dir, start_pos: f32::from(pos), start_ratio: ratio });
                    cx.set_global(PaneResizing(true));
                    cx.notify();
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_split_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_split_drag(cx)))
            .on_drag(SplitDrag { id: id3 }, |_, _, _, cx| cx.new(|_| gpui::EmptyView));
        let id4 = id.clone();
        let base = div()
            .id(SharedString::from(format!("split-{id}")))
            .relative()
            .flex()
            .size_full()
            .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<SplitDrag>, _, cx| {
                if ev.drag(cx).id != id4 {
                    return;
                }
                let Some(d) = this.split_drag.as_ref().filter(|d| d.id == id4) else { return };
                let (cur, outer) = match d.dir {
                    Dir::V => (ev.event.position.x, ev.bounds.size.width),
                    Dir::H => (ev.event.position.y, ev.bounds.size.height),
                };
                let r = drag_ratio(d.start_ratio, d.start_pos as f64, f64::from(cur), f64::from(outer));
                let id = d.id.clone();
                this.state.update(cx, |s, cx| {
                    s.workspace.set_split_ratio(&id, r);
                    s.workspace_changed(cx);
                });
            }));
        match dir {
            Dir::V => base
                .flex_row()
                .child(div().h_full().w(relative(ra)).min_w_0().overflow_hidden().child(a))
                .child(div().h_full().w(relative(rb)).min_w_0().overflow_hidden().child(b)),
            Dir::H => base
                .flex_col()
                .child(div().w_full().h(relative(ra)).min_h_0().overflow_hidden().child(a))
                .child(div().w_full().h(relative(rb)).min_h_0().overflow_hidden().child(b)),
        }
        .when(!maximized, |d| d.child(line).child(handle))
        .into_any_element()
    }

    fn end_split_drag(&mut self, cx: &mut Context<Self>) {
        if self.split_drag.take().is_some() {
            // 松手：终端把延后的列数重排立即做掉（#203，见 splitter::PaneResizing）
            cx.set_global(PaneResizing(false));
            for t in self.terminals.values() {
                t.view.update(cx, |v, cx| v.flush_resize(cx));
            }
            cx.refresh_windows();
            cx.notify();
        }
    }

    fn render_container(&mut self, c: &ContainerNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_terminal(c, window, cx);
        let theme = cx.theme().clone();
        let (is_active, is_max) = {
            let ws = &self.state.read(cx).workspace.state;
            (ws.active_container_id == c.id, ws.maximized_container_id.as_deref() == Some(c.id.as_str()))
        };
        let focused = window.is_window_active();
        let tab_bar = self.render_tab_bar(c, is_active, is_max, focused, &theme, cx);
        let body = self.render_body(c, &theme, window, cx);
        let cid = c.id.clone();
        let (rects, rect_id) = (self.pane_bounds.clone(), c.id.clone());
        div()
            .id(SharedString::from(format!("pane-{}", c.id)))
            .child(canvas(move |b, _, _| { rects.borrow_mut().insert(rect_id, b); }, |_, _, _, _| {}).absolute().size_full())
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .group(SharedString::from(format!("pane-{}", c.id)))
            .child(tab_bar)
            .child(body)
            .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.set_active(&cid, cx);
            }))
            .when_some(self.flash.as_ref().filter(|f| f.container_id == c.id), |d, f| d.child(render_flash(f, &theme)))
            .into_any_element()
    }

    fn render_tab_bar(&mut self, c: &ContainerNode, is_active: bool, is_max: bool, focused: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let active_idx = c.tabs.iter().position(|t| t.id == c.active_tab_id);
        let (handle, last) = self.tab_scroll.entry(c.id.clone()).or_insert_with(|| (ScrollHandle::new(), String::new()));
        // active 标签溢出到视野外时滚回来（只在 active 变了的时候滚，不跟用户手动横滚抢）
        if *last != c.active_tab_id {
            if let Some(i) = active_idx {
                handle.scroll_to_item(i);
            }
            *last = c.active_tab_id.clone();
        }
        let handle = handle.clone();
        let drop_idx = self.tab_drop.as_ref().filter(|(cid, _)| *cid == c.id).map(|(_, i)| *i);
        let len = c.tabs.len();
        let tabs: Vec<AnyElement> = c
            .tabs
            .iter()
            .enumerate()
            .map(|(idx, t)| {
                let active = t.id == c.active_tab_id;
                let prev_active = idx > 0 && c.tabs[idx - 1].id == c.active_tab_id;
                let title = self.title_of(t, cx);
                let status = {
                    let s = self.state.read(cx);
                    tab_status(t, t.session_id.as_deref().and_then(|id| s.session(id)))
                };
                let (before, after) = insert_marker(drop_idx, idx, len);
                let dragging = self.dragging_tab.as_deref() == Some(t.id.as_str());
                let accent_top = active && is_active && focused;
                let (cid, tid) = (c.id.clone(), t.id.clone());
                let this = cx.entity().downgrade();
                let drag = TabDrag { container_id: c.id.clone(), tab_id: t.id.clone(), title: title.clone() };
                let marker = |d: gpui::Div| d.absolute().top(px(2.0)).bottom(px(2.0)).w(px(2.0)).rounded(px(1.0)).bg(theme.accent);
                div()
                    .id(SharedString::from(format!("tab-{}", t.id)))
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(9.0))
                    .h_full()
                    .text_size(px(11.0))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.bg).text_color(theme.fg).font_weight(FontWeight::MEDIUM))
                    .when(!active, |d| d.text_color(theme.fg_muted).hover(|s| s.bg(theme.bg_hover)))
                    .when(dragging, |d| d.opacity(0.4))
                    // active 指示：顶部 2px（当前 pane 且窗口聚焦时是 accent，否则 border-strong）
                    .when(active, |d| {
                        d.child(div().absolute().top_0().left_0().right_0().h(px(2.0)).bg(if accent_top { theme.accent } else { theme.border_strong }))
                    })
                    // 两侧都不是 active 的缝里画一根 14px 的短竖线
                    .when(idx > 0 && !active && !prev_active, |d| {
                        d.child(div().absolute().left_0().top(relative(0.5)).mt(px(-7.0)).w(px(1.0)).h(px(14.0)).bg(theme.border_strong))
                    })
                    .when(before, |d| d.child(marker(div()).left(px(-2.0))))
                    .when(after, |d| d.child(marker(div()).right(px(-2.0))))
                    .child({
                        let (cid, tid) = (cid.clone(), tid.clone());
                        div()
                            .id(SharedString::from(format!("tab-icon-{}", t.id)))
                            .text_size(px(9.0))
                            .opacity(0.7)
                            .child(kind_icon(t.kind))
                            .tooltip(crate::tooltip::tip(tr!("workspace.tooltip.locate")))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                // 先切到这个标签，再让侧栏定位「当前会话」（B 包的 RevealActive）
                                this.update_ws(cx, |w| {
                                    w.tab_click(&cid, &tid);
                                    vec![]
                                });
                                window.dispatch_action(Box::new(crate::actions::sidebar::RevealActive), cx);
                            }))
                    })
                    .child(div().min_w(px(20.0)).max_w(px(180.0)).truncate().child(title))
                    .when_some(status, |d, st| d.child(render_status_dot(st, &t.id, theme)))
                    .child({
                        let (cid, tid) = (cid.clone(), tid.clone());
                        div()
                            .id(SharedString::from(format!("tab-close-{}", t.id)))
                            .text_size(px(12.0))
                            .line_height(px(12.0))
                            .px(px(2.0))
                            .rounded(px(2.0))
                            .opacity(0.5)
                            .hover(|s| s.opacity(1.0).bg(theme.bg_hover))
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(&cid, &tid, cx);
                            }))
                    })
                    .on_click({
                        let (cid, tid) = (cid.clone(), tid.clone());
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.update_ws(cx, |w| {
                                w.tab_click(&cid, &tid);
                                vec![]
                            });
                        })
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let (cid, tid) = (cid.clone(), tid.clone());
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            // 拦住：冒到 pane 上弹的是 pane 菜单，那个菜单的「关闭」关的是 active tab
                            cx.stop_propagation();
                            let (weak, target) = (this.self_weak.clone(), MenuTarget::Tab { cid: cid.clone(), tid: tid.clone() });
                            crate::overlays::show_context_menu(ev.position, window, cx, move |cx| {
                                weak.upgrade().map(|w| w.read(cx).menu_items(&target, cx)).unwrap_or_default()
                            });
                        })
                    })
                    .on_drag(drag, move |d: &TabDrag, _, _, cx| {
                        let id = d.tab_id.clone();
                        let _ = this.update(cx, |v, cx| {
                            v.dragging_tab = Some(id);
                            cx.notify();
                        });
                        dnd::ghost(&d.title, cx)
                    })
                    .on_drag_move({
                        let cid = cid.clone();
                        cx.listener(move |this, ev: &DragMoveEvent<TabDrag>, _, cx| {
                            if !is_tab_drag(DragKind::Tab) || !ev.bounds.contains(&ev.event.position) {
                                return;
                            }
                            let i = tab_insert_index(idx, f32::from(ev.event.position.x), f32::from(ev.bounds.origin.x), f32::from(ev.bounds.size.width));
                            let next = Some((cid.clone(), i));
                            if this.tab_drop != next {
                                this.tab_drop = next;
                                cx.notify();
                            }
                        })
                    })
                    .into_any_element()
            })
            .collect();

        let btn = |id: String, icon: &'static str, on: bool, tip: SharedString| {
            let gid = SharedString::from(id.clone());
            div()
                .id(SharedString::from(id))
                .group(gid.clone())
                .flex()
                .items_center()
                .justify_center()
                .px(px(2.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .opacity(if on { 1.0 } else { 0.6 })
                .hover(|s| s.bg(theme.bg_hover).opacity(1.0))
                .tooltip(crate::tooltip::tip(tip))
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.0))
                        .text_color(if on { theme.accent } else { theme.fg_muted })
                        .when(!on, |s| s.group_hover(gid, |s| s.text_color(theme.fg))),
                )
        };
        let (c1, c2, c3, c4, c5, c6) = (c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone());
        let pane_group = SharedString::from(format!("pane-{}", c.id));
        let actions = div()
            .flex()
            .flex_none()
            .gap(px(2.0))
            .ml_auto()
            .items_center()
            // 多 pane 时非活跃 pane 的按钮收起来，hover 再露出
            .when(!is_active, |d| d.opacity(0.0).group_hover(pane_group, |s| s.opacity(1.0)))
            .child(btn(format!("btn-new-{}", c.id), "icons/container-new-terminal.svg", false, tr!("workspace.tooltip.new_terminal")).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.new_shell_in(&c1, cx)
                },
            )))
            .child(btn(format!("btn-splitv-{}", c.id), "icons/container-split-v.svg", false, tr!("workspace.tooltip.split_right")).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.split_container(&c2, Dir::V, cx)
                },
            )))
            .child(btn(format!("btn-splith-{}", c.id), "icons/container-split-h.svg", false, tr!("workspace.tooltip.split_down")).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.split_container(&c3, Dir::H, cx)
                },
            )))
            .child(
                btn(
                    format!("btn-max-{}", c.id),
                    if is_max { "icons/container-restore.svg" } else { "icons/container-maximize.svg" },
                    is_max,
                    if is_max { tr!("workspace.tooltip.restore") } else { tr!("workspace.tooltip.maximize") },
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.toggle_maximize_container(&c4, cx)
                })),
            );

        div()
            .id(SharedString::from(format!("tabbar-{}", c.id)))
            .flex()
            .flex_none()
            .gap(px(6.0))
            .h(px(TAB_BAR_H))
            .pr(px(4.0))
            .overflow_hidden()
            .bg(theme.bg_soft)
            .child(
                div()
                    .id(SharedString::from(format!("tabs-{}", c.id)))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&handle)
                    .children(tabs),
            )
            .child(actions)
            // 空白处双击新建 shell（标签 / 按钮上的点击已经 stop_propagation）
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() == 2 {
                    this.new_shell_in(&c5, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, {
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let (weak, target) = (this.self_weak.clone(), MenuTarget::Pane { cid: c6.clone() });
                    crate::overlays::show_context_menu(ev.position, window, cx, move |cx| {
                        weak.upgrade().map(|w| w.read(cx).menu_items(&target, cx)).unwrap_or_default()
                    });
                })
            })
            .on_drag_move({
                let cid = c.id.clone();
                cx.listener(move |this, ev: &DragMoveEvent<TabDrag>, _, cx| {
                    // 拖出标签条：插入线收掉
                    if !ev.bounds.contains(&ev.event.position) && this.tab_drop.as_ref().is_some_and(|(c, _)| *c == cid) {
                        this.tab_drop = None;
                        cx.notify();
                    }
                })
            })
            .on_drop({
                let cid = c.id.clone();
                cx.listener(move |this, d: &TabDrag, _, cx| this.on_tab_bar_drop(&cid, d, cx))
            })
            .into_any_element()
    }

    fn render_body(&mut self, c: &ContainerNode, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let overlay = self.hover_drop.as_ref().filter(|(cid, _, _)| *cid == c.id).map(|(_, d, s)| overlay_fraction(*d, *s));
        let content: AnyElement = if c.tabs.is_empty() {
            let hint = {
                let s = self.state.read(cx);
                empty_hint::empty_hint(&s.tools, s.sessions.len(), s.loaded, &welcome::key_label("cmd-t"))
            };
            let expanded = self.shortcuts_expanded;
            let on_toggle = cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.shortcuts_expanded = !this.shortcuts_expanded;
                cx.notify();
            });
            welcome::render_welcome(theme, hint, expanded, on_toggle)
        } else {
            self.sync_hybrid(&c.active_tab_id, cx);
            match (self.hybrids.get(&c.active_tab_id), self.terminals.get(&c.active_tab_id)) {
                (Some(h), _) => div().size_full().child(h.clone()).into_any_element(),
                (None, Some(t)) => div().size_full().child(t.view.clone()).into_any_element(),
                (None, None) => Empty.into_any_element(),
            }
        };
        let (c1, c2, c3, c4, c5, c6) = (c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone(), c.id.clone());
        div()
            .id(SharedString::from(format!("pane-body-{}", c.id)))
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(theme.bg)
            .child(content)
            .children(if c.tabs.is_empty() { None } else { self.recover_pickers.get(&c.active_tab_id).map(|(p, _)| p.clone()) })
            .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<TabDrag>, _, cx| {
                this.on_pane_drag_move(&c1, DragKind::Tab, ev.bounds, ev.event.position, cx)
            }))
            .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<SessionDrag>, _, cx| {
                this.on_pane_drag_move(&c2, DragKind::Session, ev.bounds, ev.event.position, cx)
            }))
            .on_drop(cx.listener(move |this, d: &TabDrag, _, cx| {
                if let Some((dir, side)) = this.drop_zone_of(&c3) {
                    let (src, tid) = (d.container_id.clone(), d.tab_id.clone());
                    let target = c3.clone();
                    this.update_ws(cx, |w| {
                        w.split_with_tab(&src, &tid, &target, dir, side);
                        vec![]
                    });
                }
            }))
            .on_drop(cx.listener(move |this, d: &SessionDrag, _, cx| {
                if let Some((dir, side)) = this.drop_zone_of(&c4) {
                    let (target, spec) = (c4.clone(), d.spec.clone());
                    this.update_ws(cx, |w| {
                        w.split_with_session(&target, dir, side, &spec);
                        vec![]
                    });
                }
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| this.on_files_drop(&c5, paths, window, cx)))
            // 右键：落在终端里归终端菜单（A / D 包），这里只管没有终端的空 pane（欢迎卡）
            .when(c.tabs.is_empty(), |d| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        let (weak, target) = (this.self_weak.clone(), MenuTarget::Pane { cid: c6.clone() });
                        crate::overlays::show_context_menu(ev.position, window, cx, move |cx| {
                            weak.upgrade().map(|w| w.read(cx).menu_items(&target, cx)).unwrap_or_default()
                        });
                    }),
                )
            })
            .when_some(overlay, |d, (x, y, w, h)| {
                d.child(
                    div()
                        .absolute()
                        .left(relative(x))
                        .top(relative(y))
                        .w(relative(w))
                        .h(relative(h))
                        .opacity(0.18)
                        .bg(theme.accent)
                        .border_2()
                        .border_dashed()
                        .border_color(theme.accent)
                        .rounded(px(2.0)),
                )
            })
            .into_any_element()
    }

    /// 把焦点还给当前标签的终端（侧栏 Esc 用；B 侧栏包加的）：清掉「上次给过谁」，下一帧 sync_focus 重新给
    /// 当前标签的终端（没开过 / 空工作区为 None）
    pub fn active_terminal(&self, cx: &App) -> Option<Entity<TerminalView>> {
        let id = self.state.read(cx).workspace.active_tab()?.id.clone();
        self.terminals.get(&id).map(|t| t.view.clone())
    }

    /// 随便哪个标签的终端（不要求是当前 active 的）。目录丢失的通知要写进**那一个**面板，不是当前 pane
    pub fn terminal_for_tab(&self, tab_id: &str) -> Option<Entity<TerminalView>> {
        self.terminals.get(tab_id).map(|t| t.view.clone())
    }

    /// 当前 pane 上一帧的窗口坐标矩形
    pub fn active_pane_bounds(&self, cx: &App) -> Option<gpui::Bounds<Pixels>> {
        let cid = self.state.read(cx).workspace.state.active_container_id.clone();
        self.pane_bounds.borrow().get(&cid).copied()
    }

    pub fn refocus(&mut self, cx: &mut Context<Self>) {
        self.focused_tab = None;
        cx.notify();
    }

    /// 当前标签变了就把焦点给它的终端；窗口里谁都没焦点时也要给（否则快捷键全失灵，见下）
    fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.state.read(cx).workspace.active_tab().map(|t| t.id.clone());
        // 「谁都没焦点」必须补：GPUI 没有焦点时按键 / action 派发到 dispatch 树的根节点，而根视图的 on_action
        // 挂在它下面一层的 div 上，冒泡够不着 —— 空工作区启动（active 是 None，和初值相同）时 ⌘T ⌘D 全都没反应。
        // 有焦点（在侧栏 / 浮层里）时不抢
        if active == self.focused_tab && window.focused(cx).is_some() {
            return;
        }
        match active.as_ref().and_then(|id| self.terminals.get(id)) {
            Some(t) => t.view.read(cx).focus_handle(cx).focus(window),
            None => self.focus.focus(window),
        }
        // 切标签：⌘F 搜索条还开着的话，计数不能停在上一个终端（同 Tauri，切 tab 清空搜索状态）
        if active != self.focused_tab && self.focused_tab.is_some() {
            crate::overlays::search_bar::report_progress(None, cx);
        }
        self.focused_tab = active;
    }
}

/// 标签状态点：waiting ● 红色 1.2s 脉动 / busy ⚡ 黄 / idle ○ 灰（App.css `.tab-dot`）
fn render_status_dot(st: TabStatus, _tab_id: &str, theme: &Theme) -> AnyElement {
    let base = div().text_size(px(10.0)).line_height(px(10.0)).ml(px(2.0));
    match st {
        TabStatus::Waiting => {
            // 1.2s 脉动，最淡 0.3。用 `pulse` 的低频时钟，不用 with_animation(.repeat())（那会 60Hz 一直重绘）
            crate::pulse::want_ticks();
            base.text_color(theme.danger).child("●").opacity(crate::pulse::opacity(crate::pulse::now_ms(), 1200, 0.3)).into_any_element()
        }
        TabStatus::Busy => base.text_color(theme.warning).child("⚡").into_any_element(),
        TabStatus::Idle => base.text_color(theme.fg_muted).child("○").into_any_element(),
    }
}

/// 闪牌（#201 加强版）：目标 pane 整圈边框闪一下强调色，正中一张牌——大图标「弹出」（先冲过头再回弹）、
/// 卡片外圈光晕向外扩散并淡掉、右下角提示 ⌥⌘N。全是一次性动画，只在 `FLASH_MS` 内逐帧重绘，结束后零开销。
/// 图标放在固定大小的格子里，弹出时只变字号，不引起卡片尺寸抖动
fn render_flash(f: &Flash, theme: &Theme) -> AnyElement {
    const ICON_PX: f32 = 46.0;
    const ICON_BOX: f32 = 62.0;
    let ms = Duration::from_millis(flash::FLASH_MS);
    let key = |what: &str| SharedString::from(format!("flash-{what}-{}", f.seq));
    let (accent, shadow_c) = (theme.accent, theme.var("--shadow-strong"));

    let label = div()
        .min_w_0()
        .flex()
        .flex_col()
        .child(div().truncate().text_size(px(15.0)).line_height(px(20.0)).font_weight(FontWeight::SEMIBOLD).child(f.name.clone()))
        .when_some(f.number, |d, n| d.child(div().text_size(px(10.5)).line_height(px(14.0)).opacity(0.75).child(format!("⌥⌘{n}"))));

    let card = div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .max_w(relative(0.8))
        .pl(px(12.0))
        .pr(px(20.0))
        .py(px(10.0))
        .rounded(px(26.0))
        .bg(accent)
        .text_color(theme.accent_fg)
        .when_some(f.icon, |d, icon| {
            d.child(
                div()
                    .flex_none()
                    .size(px(ICON_BOX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().child(icon).with_animation(key("icon"), Animation::new(ms), |d, t| {
                        let px_size = ICON_PX * flash::punch_scale(t);
                        d.text_size(px(px_size)).line_height(px(px_size)).opacity((t / 0.12).min(1.0))
                    })),
            )
        })
        .child(label)
        .with_animation(key("card"), Animation::new(ms), move |d, t| {
            let (spread, alpha) = flash::halo(t);
            let glow = BoxShadow { color: gpui::Hsla { a: alpha, ..accent }, offset: point(px(0.0), px(0.0)), blur_radius: px(spread * 1.4), spread_radius: px(spread) };
            let drop = BoxShadow { color: shadow_c, offset: point(px(0.0), px(6.0)), blur_radius: px(18.0), spread_radius: px(0.0) };
            d.opacity(flash::card_opacity(t)).shadow(vec![glow, drop])
        });

    let ring = div().absolute().inset_0().border_2().border_color(accent).with_animation(key("ring"), Animation::new(ms), |d, t| d.opacity(flash::ring_opacity(t)));
    div()
        .absolute()
        .inset_0()
        .child(ring)
        .child(div().absolute().inset_0().flex().items_center().justify_center().child(card))
        .into_any_element()
}

impl Focusable for WorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.activation_sub.is_none() {
            // 窗口聚焦与否决定 active 标签顶线是不是 accent（TS 的 body.window-focused）
            self.activation_sub = Some(cx.observe_window_activation(window, |_, _, cx| cx.notify()));
        }
        // 拖拽结束（落下 / 取消 / 拖出窗口）后清掉所有拖拽中的状态：外部拖拽不一定有明确的结束事件（#175）
        if !cx.has_active_drag() && (self.hover_drop.is_some() || self.tab_drop.is_some() || self.dragging_tab.is_some()) {
            self.hover_drop = None;
            self.tab_drop = None;
            self.dragging_tab = None;
        }
        let ws = self.state.read(cx).workspace.state.clone();
        // 最大化时只画那一个 container（省掉隐藏 pane 的布局 / 绘制开销），但其余 pane 当前标签的终端照样要跑：
        // 不然重启后只有最大化的那个 pane 会恢复，其它 pane 里的 resume 标签不起进程、没有状态也没有通知（#226 修）。
        // 没被真实画出来的那些（本帧不会走 TerminalElement::prepaint → layout()）兜底按默认尺寸 spawn，
        // 等真的被看见时第一次 layout 会立即校正
        for c in collect_containers(&ws.root) {
            self.ensure_terminal(c, window, cx);
            let hidden = ws.maximized_container_id.as_deref().is_some_and(|max_id| max_id != c.id);
            if hidden {
                if let Some(t) = self.terminals.get(&c.active_tab_id) {
                    t.view.update(cx, |v, cx| v.spawn_hidden_fallback(window, cx));
                }
            }
        }
        let content = match ws.maximized_container_id.as_deref().and_then(|id| model::find_container(&ws.root, id)) {
            Some(c) => self.render_container(c, window, cx),
            None => self.render_node(&ws.root, false, window, cx),
        };
        // 被关掉 / 已不在树上的标签的终端（比如别处改了树）一并收掉
        let alive = model::collect_all_tab_ids(&ws.root);
        let dead: Vec<String> = self.terminals.keys().filter(|k| !alive.contains(k)).cloned().collect();
        self.shutdown_tabs(&dead, cx);
        self.tab_scroll.retain(|cid, _| find_container(&ws.root, cid).is_some());
        self.sync_focus(window, cx);
        let size = self.size.clone();
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(cx.theme().bg)
            .child(canvas(move |b, _, _| size.set(b.size), |_, _, _, _| {}).absolute().size_full())
            .child(content)
    }
}

/// 根视图把工作区 action 全部转发过来（action 在 actions::workspace 里声明）
pub fn register_actions<V: 'static>(el: gpui::Stateful<gpui::Div>, ws: Entity<WorkspaceView>, cx: &mut Context<V>) -> gpui::Stateful<gpui::Div> {
    let _ = cx;
    macro_rules! fwd {
        ($el:expr, $action:ty, |$w:ident, $cx:ident| $body:expr) => {{
            let ws = ws.clone();
            $el.on_action(move |_: &$action, _window, app| ws.update(app, |$w, $cx| $body))
        }};
    }
    let el = fwd!(el, act::NewTab, |w, cx| w.new_tab(cx));
    let el = fwd!(el, act::CloseTab, |w, cx| w.close_active_tab(cx));
    let el = fwd!(el, act::SplitRight, |w, cx| w.split(Dir::V, cx));
    let el = fwd!(el, act::SplitDown, |w, cx| w.split(Dir::H, cx));
    let el = fwd!(el, act::NextTab, |w, cx| w.cycle_tab(1, cx));
    let el = fwd!(el, act::PrevTab, |w, cx| w.cycle_tab(-1, cx));
    let el = fwd!(el, act::ToggleMaximize, |w, cx| w.toggle_maximize(cx));
    let el = fwd!(el, act::ActivateTab1, |w, cx| w.activate_nth_tab(0, cx));
    let el = fwd!(el, act::ActivateTab2, |w, cx| w.activate_nth_tab(1, cx));
    let el = fwd!(el, act::ActivateTab3, |w, cx| w.activate_nth_tab(2, cx));
    let el = fwd!(el, act::ActivateTab4, |w, cx| w.activate_nth_tab(3, cx));
    let el = fwd!(el, act::ActivateTab5, |w, cx| w.activate_nth_tab(4, cx));
    let el = fwd!(el, act::ActivateTab6, |w, cx| w.activate_nth_tab(5, cx));
    let el = fwd!(el, act::ActivateTab7, |w, cx| w.activate_nth_tab(6, cx));
    let el = fwd!(el, act::ActivateTab8, |w, cx| w.activate_nth_tab(7, cx));
    let el = fwd!(el, act::ActivateTab9, |w, cx| w.activate_nth_tab(8, cx));
    let el = fwd!(el, act::FocusPane1, |w, cx| w.focus_nth_pane(0, cx));
    let el = fwd!(el, act::FocusPane2, |w, cx| w.focus_nth_pane(1, cx));
    let el = fwd!(el, act::FocusPane3, |w, cx| w.focus_nth_pane(2, cx));
    let el = fwd!(el, act::FocusPane4, |w, cx| w.focus_nth_pane(3, cx));
    let el = fwd!(el, act::FocusPane5, |w, cx| w.focus_nth_pane(4, cx));
    let el = fwd!(el, act::FocusPane6, |w, cx| w.focus_nth_pane(5, cx));
    let el = fwd!(el, act::FocusPane7, |w, cx| w.focus_nth_pane(6, cx));
    let el = fwd!(el, act::FocusPane8, |w, cx| w.focus_nth_pane(7, cx));
    let el = fwd!(el, act::FocusPane9, |w, cx| w.focus_nth_pane(8, cx));
    let el = fwd!(el, act::FocusPaneLeft, |w, cx| w.focus_pane_dir(Direction::Left, cx));
    let el = fwd!(el, act::FocusPaneRight, |w, cx| w.focus_pane_dir(Direction::Right, cx));
    let el = fwd!(el, act::FocusPaneUp, |w, cx| w.focus_pane_dir(Direction::Up, cx));
    fwd!(el, act::FocusPaneDown, |w, cx| w.focus_pane_dir(Direction::Down, cx))
}
