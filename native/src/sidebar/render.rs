//! 侧栏的画法。数值全部照 `SessionTree.css`（注释里写着对应的选择器），颜色走主题派生色。

use std::time::Duration;

use gpui::{
    div, img, list, prelude::*, px, radians, rgb, svg, Animation, AnimationExt, AnyElement, ClickEvent, Context,
    CursorStyle, DragMoveEvent, FontWeight, Hsla, MouseButton, MouseDownEvent, SharedString, Transformation, Window,
};

use super::groups::{meta_state, relative_time, session_title, status_label, waiting_label, RunState};
use super::tree::{self, Item, GROUP_LABEL_H, PROJECT_HEADER_H, SCROLLBAR_W, SESSION_H};
use crate::tooltip::tip;
use super::{now_secs, DraggedSession, Popup, ResizeDrag, SidebarView, ThumbDrag};
use crate::actions::sidebar as act;
use crate::theme::{ActiveTheme, Theme};
use crate::workspace::model::Dir;

/// 颜色按比例透明（CSS 的 `color-mix(in srgb, X N%, transparent)`）
pub(super) fn mix(c: Hsla, a: f32) -> Hsla {
    let mut c = c;
    c.a *= a;
    c
}

/// `--brand-codex`（不参与主题推导）
fn brand_codex() -> Hsla {
    rgb(0x10a37f).into()
}

/// 拖动条 / 滑块拖动时的占位（不画东西）
struct Nothing;

impl Render for Nothing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 折叠箭头（`.tree-group-arrow` / `.tree-project-arrow`：› 展开时转 90°）
fn chevron(size: f32, open: bool, color: Hsla, group: &'static str, hover_color: Hsla) -> impl IntoElement {
    svg()
        .path("icons/session-tree-chevron.svg")
        .size(px(size))
        .flex_none()
        .text_color(color)
        .group_hover(group, move |s| s.text_color(hover_color))
        .with_transformation(Transformation::rotate(radians(if open { std::f32::consts::FRAC_PI_2 } else { 0.0 })))
}

impl SidebarView {
    pub(super) fn render_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(item) = self.tree.items.get(ix).cloned() else { return div().into_any_element() };
        match item {
            Item::Space(h) => div().h(px(h)).into_any_element(),
            Item::Empty(text) => div()
                .h(px(tree::EMPTY_H))
                .p(px(20.))
                .flex()
                .justify_center()
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(theme.fg_muted)
                .child(text)
                .into_any_element(),
            Item::GroupHeader { id, label, count, collapsed, warn } => self.render_group_header(id, label, count, collapsed, warn, &theme, cx),
            Item::ProjectHeader { key, name, cwd, count, collapsed, has_active } => {
                self.render_project_header(ix, key, name, cwd, count, collapsed, has_active, &theme, cx)
            }
            Item::Session { row, show_status } => self.render_session(ix, row, show_status, &theme, window, cx),
        }
    }

    /// `.tree-group-label`：10px 弱文字，padding 2 12 3 4，gap 5；可折叠时整行可点，hover 才给底色
    fn render_group_header(&mut self, id: String, label: String, count: usize, collapsed: bool, warn: bool, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let color = if warn { t.warning } else { t.fg_muted };
        let tooltip = if collapsed { format!("展开「{label}」") } else { format!("折叠「{label}」") };
        div()
            .h(px(GROUP_LABEL_H))
            // 列表条目默认按内容宽：不撑满的话 hover 底色只盖住文字（Tauri 的 .tree-group-label 是整行块级）
            .w_full()
            .pr(px(4.))
            .child(
                div()
                    .id(SharedString::from(format!("group-{id}")))
                    .group("tree-group-label")
                    .size_full()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .pt(px(2.))
                    .pb(px(3.))
                    .pl(px(4.))
                    .pr(px(12.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .text_color(color)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover).text_color(t.fg))
                    .tooltip(tip(tooltip))
                    .child(div().w(px(8.)).flex_none().flex().justify_center().opacity(0.6).child(chevron(10., !collapsed, color, "tree-group-label", t.fg)))
                    .child(label)
                    // 折叠起来之后计数是这一组唯一剩下的信息，所以它必须一直在
                    .child(div().opacity(0.6).child(count.to_string()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_group(&id, cx))),
            )
            .into_any_element()
    }

    /// `.tree-project-header`：padding 5 8、gap 4、11px；箭头、活跃点、名字（500）、计数药丸、hover 才出现的「+」
    #[allow(clippy::too_many_arguments)]
    fn render_project_header(
        &mut self,
        ix: usize,
        key: String,
        name: String,
        cwd: String,
        count: usize,
        collapsed: bool,
        has_active: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (key2, cwd2, cwd3, name2) = (key.clone(), cwd.clone(), cwd.clone(), name.clone());
        div()
            .id(SharedString::from(format!("proj-{key}")))
            .group("tree-project-header")
            .h(px(PROJECT_HEADER_H))
            // 块级元素撑满侧栏：名字 flex:1 才能把计数和「+」推到最右，hover 底色也是整行（同 .tree-project-header）
            .w_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(4.))
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(t.fg_muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.bg_hover).text_color(t.fg))
            .child(div().w(px(10.)).flex_none().flex().justify_center().opacity(0.6).child(chevron(12., !collapsed, t.fg_muted, "tree-project-header", t.fg)))
            .when(has_active, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("proj-dot-{key}")))
                        .flex_none()
                        .size(px(6.))
                        .mr(px(4.))
                        .rounded_full()
                        .bg(t.warning)
                        .tooltip(tip("有活跃 session")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if collapsed { t.fg_muted } else { t.fg })
                    .child(name),
            )
            // `.tree-section-count`：bg-hover 底、全圆角、padding 0 6、10px、500
            .child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .rounded_full()
                    .bg(t.bg_hover)
                    .text_size(px(10.))
                    .line_height(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(count.to_string()),
            )
            // `.tree-project-add`：16×16、圆角 3、--fg-subtle、14px；只在表头 hover 时出现
            .child(
                div()
                    .id(SharedString::from(format!("proj-add-{key}")))
                    .flex_none()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .text_size(px(14.))
                    .line_height(px(14.))
                    .text_color(t.fg_subtle)
                    .invisible()
                    .group_hover("tree-project-header", |s| s.visible())
                    .hover(|s| s.bg(t.bg_active).text_color(t.fg))
                    .tooltip(tip(format!("在 {name2} 新建 session（⌘点击新建 shell）")))
                    .child("+")
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        if ev.modifiers().platform {
                            this.new_shell_in(&cwd2, cx);
                            return;
                        }
                        // 按钮右边 +4、按钮顶：按钮贴在表头右侧 8px 内边距里
                        let at = match this.list.bounds_for_item(ix) {
                            Some(b) => gpui::point(b.right() - px(8.) + px(4.), b.top() + px(5.)),
                            None => ev.position(),
                        };
                        this.open_popup(Popup::ToolPicker { at, cwd: cwd2.clone() }, window, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_project(&key2, has_active, cx)))
            // 右键不改折叠状态，只弹菜单
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.open_popup(Popup::ProjectMenu { at: ev.position, key: key.clone(), cwd: cwd3.clone() }, window, cx);
                }),
            )
            .into_any_element()
    }

    /// `.tree-session`：两行（标题 + 元信息），padding 5 12 5 8、gap 6、圆角 4、左右 margin 4。
    /// `.focused`（当前打开的）= 15% accent 底 + 左侧 2px 竖条；`.selected`（键盘选中）= 1px accent 描边，两者正交。
    /// 描边只在列表有焦点（正在用键盘挑）时画：焦点回到终端后还挂着，就成了「两行同时高亮」（用户 2026-09-29）
    fn render_session(&mut self, ix: usize, row: usize, show_status: bool, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.read(cx);
        let Some(m) = s.sessions.get(row) else { return div().h(px(SESSION_H)).into_any_element() };
        let m = m.clone();
        let pinned = s.is_pinned(&m.session_id);
        let p = &s.prefs.sidebar;
        let (show_id, show_branch) = (p.row_short_id, p.row_branch);
        let focused = s.workspace.active_tab().and_then(|t| t.session_id.as_deref()) == Some(m.session_id.as_str());
        let selected = self.selected.as_deref() == Some(m.session_id.as_str()) && self.list_focus.is_focused(window);
        let rs = meta_state(&m);
        let title = session_title(&m.display_name, &m.first_user_msg, &m.short_id);
        let project = super::groups::basename(if m.git_root.is_empty() { &m.cwd } else { &m.git_root });
        let sid = m.session_id.clone();
        let (sid_click, sid_menu, sid_hover) = (sid.clone(), sid.clone(), sid.clone());
        let logo = crate::assets::tool_logo(&m.tool);

        let status = show_status.then(|| {
            let (color, opacity) = match rs {
                RunState::Waiting => (t.warning, 1.0),
                RunState::Busy => (t.info, 1.0),
                RunState::Idle => (mix(t.success, 0.7), 1.0),
                RunState::Stopped => (t.fg_muted, 0.4),
            };
            let dot = div()
                .id(SharedString::from(format!("st-{sid}")))
                .w(px(12.))
                .h(px(17.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .text_color(color)
                .opacity(opacity)
                .tooltip(tip(rs.title()))
                .child(rs.icon());
            if rs == RunState::Busy {
                // 正在跑的脉动（attention-pulse：2s ease-in-out，50% 时透明度 0.4）
                dot.with_animation(
                    SharedString::from(format!("pulse-{sid}")),
                    Animation::new(Duration::from_secs(2)).repeat(),
                    |el, delta| {
                        let tri = if delta < 0.5 { delta * 2.0 } else { (1.0 - delta) * 2.0 };
                        let eased = tri * tri * (3.0 - 2.0 * tri);
                        el.opacity(1.0 - 0.6 * eased)
                    },
                )
                .into_any_element()
            } else {
                dot.into_any_element()
            }
        });

        let row1 = div()
            .h(px(17.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().flex_1().min_w_0().truncate().text_color(t.fg).child(title))
            // 只有「等待审批 / 等待回答」值得挤占标题的横向空间
            .when(m.status == "waiting", |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded_full()
                        .bg(mix(t.warning, 0.2))
                        .text_color(t.warning)
                        .text_size(px(9.))
                        .line_height(px(13.))
                        .child(waiting_label(&m.waiting_for)),
                )
            })
            // `.tree-session-time`：10px、定宽右列 26、右对齐；tooltip 是 mtime_display
            .child(
                div()
                    .id(SharedString::from(format!("time-{sid}")))
                    .flex_none()
                    .min_w(px(26.))
                    .flex()
                    .justify_end()
                    .text_size(px(10.))
                    .text_color(t.fg_muted)
                    .tooltip(tip(m.mtime_display.clone()))
                    .child(relative_time(m.mtime, now_secs())),
            );

        // `.tree-session-row2`：gap 3、margin-top 2、10px --fg-muted
        let sub = |d: gpui::Div| d.flex_none().opacity(0.7);
        let row2 = div()
            .mt(px(2.))
            .h(px(14.))
            .flex()
            .items_center()
            .gap(px(3.))
            .text_size(px(10.))
            .line_height(px(14.))
            .text_color(t.fg_muted)
            .map(|d| match (&logo, m.tool.as_str()) {
                (Some(src), _) => d.child(img(src.clone()).size(px(10.)).flex_none().rounded(px(2.)).opacity(0.75)),
                (None, "codex") => d.child(
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(3.))
                        .bg(mix(brand_codex(), 0.18))
                        .text_color(brand_codex())
                        .text_size(px(8.))
                        .line_height(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .child("CX"),
                ),
                _ => d,
            })
            .when(m.status == "busy", |d| d.child(div().flex_none().text_color(t.info).child(status_label::BUSY)))
            .child(div().min_w_0().truncate().opacity(0.7).child(project))
            .when(show_id, |d| d.child(sub(div()).child("·")).child(sub(div()).child(format!("[{}]", m.short_id))))
            .when(show_branch && !m.git_branch.is_empty(), |d| {
                d.child(sub(div()).child("·")).child(div().flex_none().max_w(px(80.)).truncate().child(format!("⑂{}", m.git_branch)))
            });

        let li = div()
            .id(SharedString::from(format!("row-{sid}")))
            .relative()
            .size_full()
            .flex()
            .items_start()
            .gap(px(6.))
            // padding 5 12 5 8，减掉常驻的 1px 描边位（选中时才上色，免得选中那一刻内容跳 1px）
            .pt(px(4.))
            .pb(px(4.))
            .pl(px(7.))
            .pr(px(11.))
            .border_1()
            .border_color(if selected { t.accent } else { gpui::transparent_black() })
            .rounded(px(4.))
            .text_size(px(12.))
            .line_height(px(17.))
            .cursor_pointer()
            .when(focused, |d| d.bg(mix(t.accent, 0.15)).child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).rounded_l(px(3.)).bg(t.accent)))
            .when(!focused, |d| d.hover(|s| s.bg(t.bg_hover)))
            .children(status)
            .when(pinned, |d| d.child(div().flex_none().text_size(px(10.)).line_height(px(17.)).text_color(t.warning).child("★")))
            .child(div().flex_1().min_w_0().flex().flex_col().child(row1).child(row2))
            // 点了就是选了：鼠标和键盘落在同一个选中态上
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.selected = Some(sid_click.clone());
                this.list_focus.focus(window);
                this.open_session(&sid_click, cx);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    this.open_popup(Popup::SessionMenu { at: ev.position, session_id: sid_menu.clone() }, window, cx);
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| this.row_hovered(ix, sid_hover.clone(), *hovered, window, cx)))
            // 落点由 C 工作区包处理（workspace::dnd::SessionDrag，四区分屏 / 插进标签条）
            .on_drag(
                {
                    let d = DraggedSession::from_meta(&m);
                    crate::workspace::dnd::SessionDrag { spec: d.tab_spec(), title: d.label }
                },
                |d, _, _, cx| crate::workspace::dnd::ghost(&d.title, cx),
            );
        div().h(px(SESSION_H)).px(px(4.)).child(li).into_any_element()
    }

    /// `.tree-header`：padding 8；搜索框 + 「显示选项」按钮（22×22，margin-left 6，偏离默认时 --accent-text）
    fn render_header(&mut self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.state.read(cx).prefs.sidebar;
        let changed = p.show_archived || p.sort != "recent";
        let open = matches!(self.popup, Some(Popup::Options));
        div()
            .flex_none()
            .flex()
            .items_center()
            .p(px(8.))
            .child(self.search.clone())
            .child(
                div()
                    .id("tree-options-btn")
                    .ml(px(6.))
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .when(open, |d| d.bg(t.bg_hover))
                    .tooltip(tip("显示选项"))
                    .child(svg().path("icons/session-tree-options.svg").size(px(12.)).text_color(if changed { t.var("--accent-text") } else { t.fg_muted }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // 同一次按下刚在菜单外把它关掉了：这次按下就是「关」，别又打开
                            if this.closed_by_outside_at.take() == Some(ev.position) {
                                return;
                            }
                            if matches!(this.popup, Some(Popup::Options)) {
                                this.close_popup(cx);
                            } else {
                                this.open_popup(Popup::Options, window, cx);
                            }
                        }),
                    ),
            )
    }

    fn render_scrollbar(&mut self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let total = *self.tops.last().unwrap_or(&0.0);
        let vh = self.viewport_h();
        let (top, h) = tree::thumb_geometry(total, vh, self.scroll_top())?;
        let visible = self.scroll_active || self.thumb_dragging;
        Some(
            div()
                .id("tree-scroll-thumb")
                .absolute()
                .right_0()
                .top(px(top))
                .w(px(SCROLLBAR_W))
                .h(px(h))
                .py(px(3.))
                .px(px(2.))
                .child(
                    div()
                        .id("tree-scroll-thumb-inner")
                        .size_full()
                        .rounded_full()
                        .when(visible, |d| d.bg(mix(t.fg, 0.3)).hover(|s| s.bg(mix(t.fg, 0.5))))
                        .when(self.thumb_dragging, |d| d.bg(mix(t.fg, 0.5))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let body_top = f32::from(this.list.viewport_bounds().top());
                        this.thumb_grab = f32::from(ev.position.y) - body_top - top;
                        this.thumb_dragging = true;
                        cx.notify();
                    }),
                )
                .on_drag(ThumbDrag, |_, _, _, cx| cx.new(|_| Nothing))
                .into_any_element(),
        )
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pending_focus_search) {
            self.search.update(cx, |s, cx| s.focus_and_select_all(window, cx));
        }
        if std::mem::take(&mut self.pending_focus_list) {
            self.list_focus.focus(window);
        }
        self.apply_pending_reveal(cx);
        if !cx.has_active_drag() {
            self.resizing = false;
            self.thumb_dragging = false;
        }
        // 拖 / 悬停宽度条时标题栏那道竖线跟着亮（同 workspace::titlebar 文件头写的挂点）
        cx.set_global(crate::workspace::titlebar::TitlebarResizerHot(self.resizing || self.resizer_hovered));
        let theme = cx.theme().clone();
        let width = self.state.read(cx).prefs.sidebar.width;
        let total = *self.tops.last().unwrap_or(&0.0);
        let vh = self.viewport_h();
        let overflow = vh > 0.0 && total > vh;
        let weak = cx.entity().downgrade();
        let list_el = list(self.list.clone(), move |ix, window, cx| {
            weak.upgrade().map(|v| v.update(cx, |this, cx| this.render_item(ix, window, cx))).unwrap_or_else(|| div().into_any_element())
        })
        .size_full();
        let scrollbar = self.render_scrollbar(&theme, cx);
        let popups = self.render_popups(&theme, window, cx);

        let body = div()
            .id("tree-body")
            .key_context("SessionList")
            .track_focus(&self.list_focus)
            .relative()
            .flex_1()
            .min_h_0()
            // 经典滚动条占 9px 宽（WebKit 自定义 ::-webkit-scrollbar 不是 overlay）
            .child(div().size_full().when(overflow, |d| d.pr(px(SCROLLBAR_W))).child(list_el))
            .children(scrollbar)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, _| this.list_focus.focus(window)))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<ThumbDrag>, _, cx| {
                let total = *this.tops.last().unwrap_or(&0.0);
                let vh = f32::from(ev.bounds.size.height);
                let thumb_top = f32::from(ev.event.position.y - ev.bounds.top()) - this.thumb_grab;
                let y = tree::scroll_for_thumb(total, vh, thumb_top);
                this.set_scroll_top(y);
                this.thumb_dragging = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &act::SelectNext, _, cx| this.move_selection(true, cx)))
            .on_action(cx.listener(|this, _: &act::SelectPrev, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &act::OpenSelected, _, cx| {
                if let Some((_, m)) = this.selected_meta(cx) {
                    this.open_session(&m.session_id, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &act::OpenSelectedSplitRight, _, cx| {
                if let Some((_, m)) = this.selected_meta(cx) {
                    this.open_split(&m.session_id, Dir::V, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &act::OpenSelectedSplitDown, _, cx| {
                if let Some((_, m)) = this.selected_meta(cx) {
                    this.open_split(&m.session_id, Dir::H, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &act::ToggleHoverCard, window, cx| this.toggle_hover_for_selected(window, cx)))
            .on_action(cx.listener(|this, _: &act::CollapseGroup, _, cx| this.collapse_selected_group(true, cx)))
            .on_action(cx.listener(|this, _: &act::ExpandGroup, _, cx| this.collapse_selected_group(false, cx)))
            .on_action(cx.listener(|this, _: &act::ClearSelection, _, cx| this.clear_selection(cx)));

        // `.session-tree`：--bg-soft 底、右边 1px --border-strong（侧栏 ↔ 工作区的主结构线）
        let aside = div()
            .id("session-tree")
            .key_context("Sidebar")
            .w(px(width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(theme.bg_soft)
            .border_r_1()
            .border_color(theme.border_strong)
            .child(self.render_header(&theme, cx))
            .child(body)
            .children(popups);

        // `.resizer`：2px 透明抓取区，hover / 拖动时 --accent；拖动范围 180–480
        let resizer = div()
            .id("sidebar-resizer")
            .w(px(2.))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .when(self.resizing, |d| d.bg(theme.accent))
            .hover(|s| s.bg(theme.accent))
            .tooltip(tip("拖拽调整宽度"))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.resizer_hovered = *hovered;
                cx.notify();
            }))
            .on_drag(ResizeDrag, |_, _, _, cx| cx.new(|_| Nothing));

        div()
            .id("sidebar")
            .flex()
            .flex_none()
            .h_full()
            .when(self.resizing, |d| d.cursor(CursorStyle::ResizeLeftRight))
            .on_drag_move(cx.listener(|this, ev: &DragMoveEvent<ResizeDrag>, _, cx| {
                let w = tree::clamp_width(f32::from(ev.event.position.x - ev.bounds.left()));
                this.resizing = true;
                if (w - this.state.read(cx).prefs.sidebar.width).abs() >= 0.5 {
                    this.update_prefs(cx, |p| p.width = w);
                }
            }))
            .child(aside)
            .child(resizer)
    }
}
