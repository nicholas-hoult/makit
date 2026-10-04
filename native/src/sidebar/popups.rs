//! 侧栏的浮层：「显示选项」菜单（`.tree-menu`）、会话 / 项目右键菜单（`.context-menu`）、
//! 「+」的工具选择器（`.context-menu .tool-picker-btn`）、悬停详情卡（`.tree-hover-card`）。
//!
//! 右键菜单是**最小实现**：D 浮层包会做通用 ContextMenu 组件，好了之后 `render_context_menu` 换成它
//! （菜单项内容 `session_menu` / `project_menu` 不用动）。点菜单外关闭用 `on_mouse_down_out`，
//! Esc 走快捷键表（context `SidebarMenu` → `DismissMenu`）。

use crate::{tr, ts};
use gpui::{
    anchored, deferred, div, img, point, prelude::*, px, AnyElement, BoxShadow, ClickEvent, Context, Corner, Div, FontWeight,
    MouseDownEvent, Pixels, Point, Stateful, Window,
};

use super::groups::SortKey;
use super::hover;
use super::render::mix;
use crate::tooltip::tip;
use super::{Popup, SidebarView};
use crate::actions::{overlays as ov, sidebar as act};
use crate::theme::Theme;

/// 菜单项要做的事
#[derive(Clone)]
enum Cmd {
    TogglePin(String),
    ToggleArchive(String),
    Reveal(String),
    Copy(String),
    NewShell(String),
    OnlyProject(String),
    NewSession(String, &'static str),
    Detail(String),
}

/// 一个右键菜单项；`cmd` 为 None = 灰掉
struct Entry {
    label: String,
    cmd: Option<Cmd>,
}

enum MenuLine {
    Item(Entry),
    Sep,
}

fn item(label: impl Into<String>, cmd: Cmd) -> MenuLine {
    MenuLine::Item(Entry { label: label.into(), cmd: Some(cmd) })
}

fn disabled(label: impl Into<String>) -> MenuLine {
    MenuLine::Item(Entry { label: label.into(), cmd: None })
}

fn shadow(color: gpui::Hsla, y: f32, blur: f32) -> Vec<BoxShadow> {
    vec![BoxShadow { color, offset: point(px(0.), px(y)), blur_radius: px(blur), spread_radius: px(0.) }]
}

impl SidebarView {
    fn run_cmd(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popup(cx);
        match cmd {
            Cmd::TogglePin(id) => self.toggle_pin(&id, cx),
            Cmd::ToggleArchive(id) => self.toggle_archive(&id, window, cx),
            Cmd::Reveal(path) => self.reveal_in_finder(path),
            Cmd::Detail(id) => crate::overlays::open_detail(&id, window, cx),
            Cmd::Copy(text) => self.copy(text, cx),
            Cmd::NewShell(cwd) => self.new_shell_in(&cwd, cx),
            Cmd::OnlyProject(key) => self.only_this_project(&key, cx),
            Cmd::NewSession(cwd, tool) => self.new_session_in(&cwd, tool, cx),
        }
    }

    /// 会话行右键（SessionTree.tsx treeMenuItems 的 session 分支）。每次渲染重算：后台在刷状态，
    /// 「置顶 / 取消置顶」这类文案要跟着现在的状态走
    fn session_menu(&self, id: &str, cx: &Context<Self>) -> Vec<MenuLine> {
        let s = self.state.read(cx);
        let Some(m) = s.session(id) else { return vec![] };
        // last_cwd 优先：会话跑起来之后可能 cd 走了，右键要去的是它现在在的目录
        let cwd = if m.last_cwd.is_empty() { m.cwd.clone() } else { m.last_cwd.clone() };
        vec![
            item(if s.is_pinned(id) { tr!("sidebar.menu.unpin") } else { tr!("sidebar.menu.pin") }, Cmd::TogglePin(id.into())),
            item(if m.archived { tr!("sidebar.menu.unarchive") } else { tr!("sidebar.menu.archive") }, Cmd::ToggleArchive(id.into())),
            MenuLine::Sep,
            item(tr!("sidebar.menu.detail"), Cmd::Detail(id.into())),
            MenuLine::Sep,
            item(tr!("sidebar.menu.reveal"), Cmd::Reveal(cwd.clone())),
            item(tr!("sidebar.menu.copy_path"), Cmd::Copy(cwd)),
            item(tr!("sidebar.menu.copy_id"), Cmd::Copy(m.session_id.clone())),
            item(tr!("sidebar.menu.copy_resume"), Cmd::Copy(Self::resume_command_line(m))),
        ]
    }

    /// 项目头右键
    fn project_menu(&self, key: &str, cwd: &str) -> Vec<MenuLine> {
        vec![
            item(tr!("sidebar.menu.reveal"), Cmd::Reveal(cwd.into())),
            item(tr!("sidebar.menu.copy_path"), Cmd::Copy(cwd.into())),
            MenuLine::Sep,
            item(tr!("sidebar.menu.new_shell"), Cmd::NewShell(cwd.into())),
            MenuLine::Sep,
            // 只有一个组时这项没有意义，灰掉而不是让它假装能用
            if self.project_keys.len() <= 1 { disabled(tr!("sidebar.menu.only_project")) } else { item(tr!("sidebar.menu.only_project"), Cmd::OnlyProject(key.into())) },
        ]
    }

    /// 浮层外壳：拿焦点（Esc 关）、点外面关、挡住下面的 hover / 点击
    fn menu_shell(&self, id: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .key_context("SidebarMenu")
            .track_focus(&self.menu_focus)
            .occlude()
            .font_family(".SystemUIFont")
            .on_action(cx.listener(|this, _: &act::DismissMenu, window, cx| {
                this.close_popup(cx);
                this.list_focus.focus(window);
            }))
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.closed_by_outside_at = Some(ev.position);
                this.close_popup(cx);
            }))
    }

    /// `.context-menu`：padding 4、min-width 150、圆角 6、--bg-soft、1px --border-strong、阴影 0 8 24；
    /// 项 padding 6 12、圆角 4、12px；分隔线 margin 4 6
    fn render_context_menu(&self, id: &'static str, lines: Vec<MenuLine>, t: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut menu = self
            .menu_shell(id, cx)
            .min_w(px(150.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(t.bg_soft)
            .border_1()
            .border_color(t.border_strong)
            .rounded(px(6.))
            .shadow(shadow(t.var("--shadow-strong"), 8., 24.))
            .text_size(px(12.))
            .line_height(px(15.));
        for (i, line) in lines.into_iter().enumerate() {
            menu = match line {
                MenuLine::Sep => menu.child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(t.border)),
                MenuLine::Item(e) => {
                    let row = div().id(i).px(px(12.)).py(px(6.)).rounded(px(4.)).whitespace_nowrap().child(e.label);
                    menu.child(match e.cmd {
                        Some(cmd) => row
                            .text_color(t.fg)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.bg_hover))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.run_cmd(cmd.clone(), window, cx))),
                        None => row.text_color(t.fg_muted),
                    })
                }
            };
        }
        menu
    }

    /// 「显示选项」（`.tree-menu`）：原生菜单的样子，每行 24 高、左侧 28px ✓ 列、浅色分段标题。
    /// 选项类的行点了不关菜单（常常要连着改几项），刷新、设置点了就关
    fn render_options_menu(&self, t: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.state.read(cx).prefs.sidebar.clone();
        let project_view = p.view == "project";
        let sort = SortKey::parse(&p.sort);
        let title = |text: &str| div().pt(px(6.)).pb(px(2.)).pl(px(28.)).pr(px(12.)).text_size(px(10.)).line_height(px(14.)).text_color(t.fg_muted).child(text.to_string());
        let sep = || div().h(px(1.)).my(px(4.)).bg(t.border);
        let row = |id: &'static str, checked: bool, label: String, kbd: Option<&'static str>, enabled: bool| {
            div()
                .id(id)
                .h(px(24.))
                .flex()
                .items_center()
                .pr(px(12.))
                .text_color(if enabled { t.fg } else { t.fg_muted })
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(t.bg_hover)))
                .child(div().w(px(28.)).flex_none().flex().justify_center().text_size(px(11.)).child(if checked { "✓" } else { "" }))
                .child(div().flex_1().whitespace_nowrap().child(label))
                .children(kbd.map(|k| div().pl(px(12.)).text_size(px(11.)).text_color(t.fg_muted).child(k)))
        };
        let mut menu = self
            .menu_shell("tree-menu", cx)
            .min_w(px(176.))
            .py(px(4.))
            .flex()
            .flex_col()
            .bg(t.bg_soft)
            .border_1()
            .border_color(t.border_strong)
            .rounded(px(6.))
            .shadow(shadow(t.shadow, 4., 12.))
            .text_size(px(12.))
            .line_height(px(15.))
            .child(title(&tr!("sidebar.options.group")))
            .child(row("view-status", !project_view, ts!("sidebar.options.by_status"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.view = "status".into()))))
            .child(row("view-project", project_view, ts!("sidebar.options.by_project"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.view = "project".into()))));
        // 项目视图里组内固定按优先级排，排序选了也不起作用，干脆不给
        if !project_view {
            menu = menu.child(title(&tr!("sidebar.options.sort")));
            for (id, key, label) in [("sort-recent", SortKey::Recent, tr!("sidebar.options.sort_recent")), ("sort-count", SortKey::Count, tr!("sidebar.options.sort_count")), ("sort-first", SortKey::FirstMsg, tr!("sidebar.options.sort_first"))] {
                menu = menu.child(row(id, sort == key, label.into(), None, true).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.sort = key.as_str().into()))));
            }
        }
        menu.child(sep())
            .child(row("show-archived", p.show_archived, ts!("sidebar.options.show_archived"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.show_archived = !p.show_archived))))
            .child(row("show-logo", p.row_logo, ts!("sidebar.options.show_logo"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.row_logo = !p.row_logo))))
            .child(row("show-project", p.row_project, ts!("sidebar.options.show_project"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.row_project = !p.row_project))))
            .child(row("show-time", p.row_time, ts!("sidebar.options.show_time"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.row_time = !p.row_time))))
            .child(row("show-short-id", p.row_short_id, ts!("sidebar.options.show_short_id"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.row_short_id = !p.row_short_id))))
            .child(row("show-branch", p.row_branch, ts!("sidebar.options.show_branch"), None, true).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.update_prefs(cx, |p| p.row_branch = !p.row_branch))))
            .child(sep())
            .child({
                let r = row("refresh", false, if self.refreshing { ts!("sidebar.options.refreshing") } else { ts!("sidebar.options.refresh") }, Some("⌘R"), !self.refreshing);
                if self.refreshing {
                    r
                } else {
                    r.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.close_popup(cx);
                        this.refresh(cx);
                    }))
                }
            })
            .child(row("settings", false, ts!("sidebar.options.settings"), Some("⌘,"), true).on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_popup(cx);
                window.dispatch_action(Box::new(ov::OpenSettings), cx);
            })))
    }

    /// 「+」弹出的工具选择器：Claude / Codex，带 logo（没缓存时 ◆ / ⬡）
    fn render_tool_picker(&self, cwd: &str, t: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut menu = self
            .menu_shell("tool-picker", cx)
            .min_w(px(150.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(t.bg_soft)
            .border_1()
            .border_color(t.border_strong)
            .rounded(px(6.))
            .shadow(shadow(t.var("--shadow-strong"), 8., 24.))
            .text_size(px(12.))
            .line_height(px(16.));
        for (tool, label, fallback) in [("claude", "Claude", "◆"), ("codex", "Codex", "⬡")] {
            let cwd = cwd.to_string();
            let icon = match crate::assets::tool_logo(tool) {
                Some(src) => img(src).size(px(16.)).flex_none().rounded(px(3.)).into_any_element(),
                None => div().w(px(16.)).flex_none().flex().justify_center().text_size(px(13.)).child(fallback).into_any_element(),
            };
            menu = menu.child(
                div()
                    .id(tool)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(7.))
                    .rounded(px(4.))
                    .text_color(t.fg)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .child(icon)
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.run_cmd(Cmd::NewSession(cwd.clone(), tool), window, cx))),
            );
        }
        menu
    }

    /// `.tree-hover-card`：min 240 / max 360、padding 8 10、圆角 6、阴影 0 4 16；标题 12px 500；
    /// 表格 10px，标签列 50px 灰、值 --fg 任意处换行；点一行复制它的值（静默）
    fn render_hover_card(&self, id: &str, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.state.read(cx).session(id)?.clone();
        let title = super::groups::flatten(&hover::card_title(&m));
        let mut table = div().flex().flex_col().text_size(px(10.)).line_height(px(14.));
        for (i, (label, value)) in hover::card_rows(&m).into_iter().enumerate() {
            let copy = value.clone();
            table = table.child(
                div()
                    .id(("hover-row", i))
                    .flex()
                    .py(px(2.))
                    .rounded(px(3.))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.bg_hover))
                    .active(|s| s.bg(mix(t.accent, 0.2)))
                    .tooltip(tip(tr!("common.click_to_copy")))
                    .child(div().w(px(50.)).flex_none().pr(px(8.)).whitespace_nowrap().text_color(t.fg_muted).child(label))
                    .child(div().flex_1().min_w_0().text_color(t.fg).child(super::groups::flatten(&value)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.copy(copy.clone(), cx))),
            );
        }
        Some(
            div()
                .id("tree-hover-card")
                .occlude()
                .font_family(".SystemUIFont")
                .min_w(px(240.))
                .max_w(px(360.))
                .px(px(10.))
                .py(px(8.))
                .bg(t.bg_soft)
                .border_1()
                .border_color(t.border_strong)
                .rounded(px(6.))
                .shadow(shadow(t.var("--shadow-strong"), 4., 16.))
                .child(div().mb(px(6.)).truncate().text_size(px(12.)).line_height(px(17.)).font_weight(FontWeight::MEDIUM).text_color(t.fg).child(title))
                .child(table)
                // 移到卡片上保持；离开卡片立刻关（同 TS 的 onMouseLeave）
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.hover_task = None;
                    } else {
                        this.hover = None;
                        cx.notify();
                    }
                }))
                .into_any_element(),
        )
    }

    /// 全部浮层（放在侧栏元素树里，`deferred` 画在最上层，`anchored` 用窗口坐标定位）
    pub(super) fn render_popups(&mut self, t: &Theme, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // 记下侧栏在窗口里的位置，「显示选项」菜单贴着头部右下角
        let cell = self.aside_bounds.clone();
        let mut out = vec![gpui::canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().size_full().into_any_element()];
        let at = |p: Point<Pixels>, el: AnyElement| deferred(anchored().position(p).snap_to_window_with_margin(px(8.)).child(el)).with_priority(1).into_any_element();
        match self.popup.clone() {
            Some(Popup::Options) => {
                let b = self.aside_bounds.get();
                // `.tree-menu { top: 100%; right: 8px }`：头部高 40（padding 8 + 搜索框 24 + 8）
                let corner = point(b.right() - px(8.), b.top() + px(40.));
                let menu = self.render_options_menu(t, cx).into_any_element();
                out.push(deferred(anchored().anchor(Corner::TopRight).position(corner).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(1).into_any_element());
            }
            Some(Popup::SessionMenu { at: p, session_id }) => {
                let lines = self.session_menu(&session_id, cx);
                let el = self.render_context_menu("session-menu", lines, t, cx).into_any_element();
                out.push(at(p, el));
            }
            Some(Popup::ProjectMenu { at: p, key, cwd }) => {
                let lines = self.project_menu(&key, &cwd);
                let el = self.render_context_menu("project-menu", lines, t, cx).into_any_element();
                out.push(at(p, el));
            }
            Some(Popup::ToolPicker { at: p, cwd }) => {
                let el = self.render_tool_picker(&cwd, t, cx).into_any_element();
                out.push(at(p, el));
            }
            None => {}
        }
        if let Some(h) = self.hover.clone() {
            if let Some(card) = self.render_hover_card(&h.session_id, t, cx) {
                out.push(deferred(anchored().position(h.at).snap_to_window_with_margin(px(8.)).child(card)).with_priority(1).into_any_element());
            }
        }
        out
    }
}
