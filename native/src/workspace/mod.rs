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
pub mod flash;
pub mod labels;
pub mod menu;
pub mod model;
pub mod selftest;
pub mod splitter;
pub mod titlebar;
pub mod welcome;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, point, prelude::*, px, relative, svg, Animation, AnimationExt, AnyElement, App, BoxShadow, ClickEvent, Context,
    DragMoveEvent, Empty, Entity, EntityInputHandler, ExternalPaths, FocusHandle, Focusable, FontWeight, MouseButton,
    MouseDownEvent, MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Size, Subscription, WeakEntity, Window,
};

use crate::actions::workspace as act;
use crate::state::AppState;
use crate::terminal::{SpawnSpec, TerminalEvent, TerminalView};
use crate::theme::{ActiveTheme, Theme};
use dnd::{SessionDrag, TabDrag};
use drop::{accepts_pane_drop, format_paths_for_terminal, insert_marker, is_tab_drag, overlay_fraction, tab_insert_index, DragKind};
use labels::{kind_icon, tab_status, tab_title, TabStatus};
use menu::{item, item_if, render_menu, MenuItem};
use model::{
    collect_containers, find_container, find_nearest_container, layout_tree, resume_cmd, split_id, tabs_to_close, CloseScope,
    ContainerNode, Dir, Direction, LayoutNode, Rect, Side, SplitNode, TabKind,
};
use splitter::{drag_ratio, PaneResizing, HIT_PX};

/// 标签条高度（`.container-tab-bar { flex: 0 0 28px }`）
pub const TAB_BAR_H: f32 = 28.0;

struct Term {
    view: Entity<TerminalView>,
    _sub: Subscription,
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
    menu: Option<(Point<Pixels>, MenuTarget)>,
    /// 每个 container 的标签条滚动 + 上次滚到的 active 标签（active 变了才滚）
    tab_scroll: HashMap<String, (ScrollHandle, String)>,
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
            size: Rc::new(Cell::new(Size { width: px(1000.0), height: px(600.0) })),
            split_drag: None,
            hover_split: None,
            hover_drop: None,
            tab_drop: None,
            dragging_tab: None,
            flash: None,
            flash_seq: 0,
            menu: None,
            tab_scroll: HashMap::new(),
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
            if let Some(t) = self.terminals.remove(id) {
                t.view.update(cx, |v, _| v.shutdown());
            }
        }
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
        let (icon, name) = {
            let s = self.state.read(cx);
            let cs = collect_containers(&s.workspace.state.root);
            let Some(idx) = cs.iter().position(|c| c.id == cid) else { return };
            let c = cs[idx];
            let title = c.tabs.iter().find(|t| t.id == c.active_tab_id).map(|t| {
                let meta = t.session_id.as_deref().and_then(|id| s.session(id));
                tab_title(t, meta)
            });
            (flash::flash_icon(&s.prefs.pane_icons, idx), flash::flash_name(title.as_deref()))
        };
        self.flash_seq += 1;
        let seq = self.flash_seq;
        self.flash = Some(Flash { container_id: cid.to_string(), icon, name, seq });
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
        let sub = cx.subscribe(&view, move |this: &mut Self, _, ev: &TerminalEvent, cx| match ev {
            TerminalEvent::Exited => {
                let loc = this.state.read(cx).workspace.locate_tab(&tab_id).map(|(c, _)| c.id.clone());
                if let Some(cid) = loc {
                    this.close_tab(&cid, &tab_id, cx);
                }
            }
            TerminalEvent::TitleChanged => cx.notify(),
        });
        self.terminals.insert(tab.id.clone(), Term { view, _sub: sub });
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

    /// Finder 拖文件进来：在这个 pane 当前标签的命令行里插路径（不分屏），然后焦点跟过去
    fn on_files_drop(&mut self, cid: &str, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let list: Vec<String> = paths.paths().iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let text = format_paths_for_terminal(&list);
        if text.is_empty() {
            return;
        }
        let tid = find_container(&self.state.read(cx).workspace.state.root, cid).map(|c| c.active_tab_id.clone());
        let Some(view) = tid.and_then(|t| self.terminals.get(&t)).map(|t| t.view.clone()) else { return };
        // 走终端的文字输入口（和输入法上屏同一条路，= 像打字一样写进 PTY）。A 终端包有专门的 write_text 之后换过去
        view.update(cx, |v, cx| v.replace_text_in_range(None, &text, window, cx));
        self.set_active(cid, cx);
        view.read(cx).focus_handle(cx).focus(window);
    }

    // ---- 右键菜单 ----

    fn menu_items(&self, target: &MenuTarget, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let w = cx.entity().downgrade();
        let s = self.state.read(cx);
        match target {
            MenuTarget::Pane { cid } => {
                let Some(c) = find_container(&s.workspace.state.root, cid) else { return vec![] };
                let (c1, c2, c3, c4, tid) = (cid.clone(), cid.clone(), cid.clone(), cid.clone(), c.active_tab_id.clone());
                let (w1, w2, w3, w4) = (w.clone(), w.clone(), w.clone(), w);
                vec![
                    item("左右分屏", move |_, cx| drop(w1.update(cx, |v, cx| v.split_container(&c1, Dir::V, cx)))),
                    item("上下分屏", move |_, cx| drop(w2.update(cx, |v, cx| v.split_container(&c2, Dir::H, cx)))),
                    item("新终端", move |_, cx| drop(w3.update(cx, |v, cx| v.new_shell_in(&c3, cx)))),
                    MenuItem::Sep,
                    // 明确写「当前」：这个菜单是在 pane 上右键弹的，没有「某个 tab」可指
                    item_if(!tid.is_empty(), "关闭当前 tab", move |_, cx| drop(w4.update(cx, |v, cx| v.close_tab(&c4, &tid, cx)))),
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
                vec![
                    item("关闭", close_one),
                    item_if(!others.is_empty(), "关闭其他", close_others),
                    item_if(!right.is_empty(), "关闭右侧", close_right),
                    MenuItem::Sep,
                    // 只有一个 tab 时禁用：拆出去还是「一个 pane 一个 tab」，而且源 container 会先被摘掉
                    item_if(!single, "移到左右分屏", mv(Dir::V)),
                    item_if(!single, "移到上下分屏", mv(Dir::H)),
                    MenuItem::Sep,
                    item("在 Finder 中显示", move |_, _| {
                        let _ = makit_core::paths::open_path(p1.clone(), true);
                    }),
                    item("复制路径", move |_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(p2.clone()))),
                    item_if(resume.is_some(), "复制恢复命令", move |_, cx| {
                        if let Some(r) = &resume {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(r.clone()));
                        }
                    }),
                ]
            }
        }
    }

    fn open_menu(&mut self, pos: Point<Pixels>, target: MenuTarget, cx: &mut Context<Self>) {
        self.menu = Some((pos, target));
        cx.notify();
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
        div()
            .id(SharedString::from(format!("pane-{}", c.id)))
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
                            .tooltip(|window, cx| titlebar::gpui_tooltip("在侧栏定位 session", window, cx))
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
                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            // 拦住：冒到 pane 上弹的是 pane 菜单，那个菜单的「关闭」关的是 active tab
                            cx.stop_propagation();
                            this.open_menu(ev.position, MenuTarget::Tab { cid: cid.clone(), tid: tid.clone() }, cx);
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

        let btn = |id: String, icon: &'static str, on: bool, tip: &'static str| {
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
                .tooltip(move |window, cx| titlebar::gpui_tooltip(tip, window, cx))
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
            .child(btn(format!("btn-new-{}", c.id), "icons/container-new-terminal.svg", false, "新终端 (⌘T)").on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.new_shell_in(&c1, cx)
                },
            )))
            .child(btn(format!("btn-splitv-{}", c.id), "icons/container-split-v.svg", false, "左右分屏 (⌘D)").on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.split_container(&c2, Dir::V, cx)
                },
            )))
            .child(btn(format!("btn-splith-{}", c.id), "icons/container-split-h.svg", false, "上下分屏 (⌘⇧D)").on_click(cx.listener(
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
                    if is_max { "还原 (⌘⌥↩)" } else { "最大化 (⌘⌥↩)" },
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
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_menu(ev.position, MenuTarget::Pane { cid: c6.clone() }, cx);
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
            welcome::render_welcome(theme)
        } else {
            match self.terminals.get(&c.active_tab_id) {
                Some(t) => div().size_full().child(t.view.clone()).into_any_element(),
                None => Empty.into_any_element(),
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
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(ev.position, MenuTarget::Pane { cid: c6.clone() }, cx)),
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
        self.focused_tab = active;
    }
}

/// 标签状态点：waiting ● 红色 1.2s 脉动 / busy ⚡ 黄 / idle ○ 灰（App.css `.tab-dot`）
fn render_status_dot(st: TabStatus, tab_id: &str, theme: &Theme) -> AnyElement {
    let base = div().text_size(px(10.0)).line_height(px(10.0)).ml(px(2.0));
    match st {
        TabStatus::Waiting => base
            .text_color(theme.danger)
            .child("●")
            .with_animation(SharedString::from(format!("dot-{tab_id}")), Animation::new(Duration::from_millis(1200)).repeat(), |d, t| {
                // 0% / 100% 不透明，50% 0.3（ease-in-out 近似成三角波）
                d.opacity(1.0 - 0.7 * (1.0 - (2.0 * t - 1.0).abs()))
            })
            .into_any_element(),
        TabStatus::Busy => base.text_color(theme.warning).child("⚡").into_any_element(),
        TabStatus::Idle => base.text_color(theme.fg_muted).child("○").into_any_element(),
    }
}

/// 闪牌：pane 正中的药丸（App.css `.pane-flash`：accent 底、圆角 999、内边距 6/13/6/10、间距 7、12px 500、阴影）
fn render_flash(f: &Flash, theme: &Theme) -> AnyElement {
    let card = div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .max_w(relative(0.8))
        .pl(px(10.0))
        .pr(px(13.0))
        .py(px(6.0))
        .rounded(px(999.0))
        .bg(theme.accent)
        .text_color(theme.accent_fg)
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .shadow(vec![BoxShadow { color: theme.var("--shadow-strong"), offset: point(px(0.0), px(4.0)), blur_radius: px(14.0), spread_radius: px(0.0) }])
        .when_some(f.icon, |d, icon| {
            // 图标：前 22% 从左侧 16px 处滑进来、淡入（TS 版还有转一圈 + 彗尾，GPUI 文字不支持旋转 / text-shadow，省掉）
            d.child(
                div().flex_none().text_size(px(20.0)).line_height(px(20.0)).child(icon).with_animation(
                    SharedString::from(format!("flash-icon-{}", f.seq)),
                    Animation::new(Duration::from_millis(flash::FLASH_MS)),
                    |d, t| {
                        let p = (t / 0.22).min(1.0);
                        d.opacity(p).ml(px(-16.0 * (1.0 - p)))
                    },
                ),
            )
        })
        .child(div().min_w_0().truncate().child(f.name.clone()))
        .with_animation(SharedString::from(format!("flash-{}", f.seq)), Animation::new(Duration::from_millis(flash::FLASH_MS)), |d, t| {
            d.opacity(flash::card_opacity(t))
        });
    div().absolute().inset_0().flex().items_center().justify_center().child(card).into_any_element()
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
        let menu = self.menu.clone().map(|(pos, target)| {
            let items = self.menu_items(&target, cx);
            let this = cx.entity().downgrade();
            let close: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |_, cx| {
                let _ = this.update(cx, |v, cx| {
                    v.menu = None;
                    cx.notify();
                });
            });
            render_menu(pos, items, close, cx)
        });
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
            .children(menu)
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
