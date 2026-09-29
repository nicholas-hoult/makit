//! ⌘K 命令面板视图（照 `src/CommandPalette.tsx` + App.css `.palette*`）。纯逻辑在 `palette_logic`。
//!
//! 每次打开新建一个实体：query 清空、光标回到第一条、筛选栏收起、历史重新读 —— 正是 TS 版
//! `useEffect([open])` 里重置的那几样。筛选 / 排序 / 折叠组 / 历史存在 `prefs.palette`。

use std::time::Duration;

use gpui::{
    div, prelude::*, px, relative, svg, Animation, AnimationExt, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, ScrollHandle, SharedString, StyledText, Transformation, Window,
};

use super::palette_logic::*;
use super::style::*;
use super::text_input::{TextInput, TextInputEvent};
use super::{icons, session_ops, MenuItem};
use crate::actions::overlays as act;
use crate::state::AppState;
use crate::theme::{ActiveTheme, Theme};
use crate::workspace::model::Dir;

pub enum PaletteEvent {
    Close,
}

#[derive(Clone, Copy)]
enum Mode {
    Default,
    SplitRight,
    SplitDown,
}

pub struct PaletteView {
    state: Entity<AppState>,
    input: Entity<TextInput>,
    query: String,
    active: usize,
    filter_open: bool,
    filter: PaletteFilter,
    sort: SortKey,
    history: Vec<String>,
    collapsed: Vec<String>,
    /// 鼠标真正移动过之后 hover 才改高亮（面板弹出时鼠标正好停在某一行上，不能抢走键盘光标）
    mouse_moved: bool,
    scroll: ScrollHandle,
    scrollbar: Entity<crate::scrollbar::Scrollbar>,
    scroll_to_active: bool,
}

impl EventEmitter<PaletteEvent> for PaletteView {}

impl PaletteView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("搜索 session...  (⌘K)", cx));
        cx.subscribe(&input, |this, input, _: &TextInputEvent, cx| {
            this.query = input.read(cx).text().to_string();
            this.active = 0;
            this.scroll_to_active = true;
            cx.notify();
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        window.focus(&input.focus_handle(cx));
        let scroll = ScrollHandle::new();
        let scrollbar = crate::scrollbar::Scrollbar::handle(&scroll, cx);
        let p = &state.read(cx).prefs.palette;
        Self {
            filter: PaletteFilter::from_json(p.filter.as_ref()),
            sort: SortKey::parse(p.sort.as_deref()),
            history: p.history.iter().take(MAX_HISTORY).cloned().collect(),
            collapsed: p.collapsed.clone(),
            state,
            input,
            query: String::new(),
            active: 0,
            filter_open: false,
            mouse_moved: false,
            scroll,
            scrollbar,
            scroll_to_active: true,
        }
    }

    /// 自检用：列表的滚动句柄和它的滚动条
    pub fn debug_scroll(&self) -> (ScrollHandle, Entity<crate::scrollbar::Scrollbar>) {
        (self.scroll.clone(), self.scrollbar.clone())
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    fn items(&self, cx: &App) -> Vec<PaletteItem> {
        let s = self.state.read(cx);
        build_items(&s.sessions, &s.prefs.pinned_sessions, self.sort)
    }

    fn flat(&self, cx: &App) -> Vec<PaletteItem> {
        let items = self.items(cx);
        let filtered = filter_items(&items, &self.filter, &self.query, now());
        let groups = group_items(filtered, self.sort);
        visible_flat(&groups, &self.collapsed, &self.query).into_iter().cloned().collect()
    }

    // ---- 持久化 ----

    fn save_filter(&mut self, cx: &mut Context<Self>) {
        let v = self.filter.to_json();
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.palette.filter = Some(v)));
        self.active = 0;
        cx.notify();
    }

    fn set_sort(&mut self, k: SortKey, cx: &mut Context<Self>) {
        self.sort = k;
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.palette.sort = Some(k.as_str().into())));
        cx.notify();
    }

    fn toggle_group(&mut self, g: String, cx: &mut Context<Self>) {
        if let Some(i) = self.collapsed.iter().position(|x| *x == g) {
            self.collapsed.remove(i);
        } else {
            self.collapsed.push(g);
        }
        let c = self.collapsed.clone();
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.palette.collapsed = c));
        self.active = 0;
        cx.notify();
    }

    fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.history.clear();
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.palette.history.clear()));
        cx.notify();
    }

    // ---- 键盘 ----

    fn step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let len = self.flat(cx).len();
        self.active = step_index(self.active, len, delta);
        self.scroll_to_active = true;
        cx.notify();
    }

    fn activate(&mut self, item: &PaletteItem, mode: Mode, cx: &mut Context<Self>) {
        let hist = push_history(&self.history, &self.query);
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.palette.history = hist));
        cx.emit(PaletteEvent::Close);
        let sid = item.session_id.clone();
        match mode {
            Mode::Default => session_ops::open_session(&sid, cx),
            Mode::SplitRight => session_ops::open_session_split(&sid, Dir::V, cx),
            Mode::SplitDown => session_ops::open_session_split(&sid, Dir::H, cx),
        }
    }

    fn confirm(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if let Some(it) = self.flat(cx).get(self.active).cloned() {
            self.activate(&it, mode, cx);
        }
    }

    fn open_project_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let roots = project_roots(&self.state.read(cx).sessions);
        let current = self.filter.project.clone();
        let this = cx.entity().downgrade();
        let pick = move |v: String| {
            let this = this.clone();
            move |_: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |p, cx| {
                    p.filter.project = v.clone();
                    p.save_filter(cx);
                });
            }
        };
        super::show_context_menu(ev.position(), window, cx, move |_| {
            let mut items = vec![MenuItem::action("全部", pick(String::new())).checked(current.is_empty())];
            for r in &roots {
                items.push(MenuItem::action(project_label(r).to_string(), pick(r.clone())).checked(*r == current));
            }
            items
        });
    }

    // ---- 渲染 ----

    fn render_item(&self, it: &PaletteItem, idx: usize, q: &str, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let active = idx == self.active;
        let accent_text = theme.var("--accent-text");
        // 命中高亮：字色 + 字重，不填底色；光标行上改用 --fg + 700（accent 底上 accent 字看不清）
        let mark = if active {
            HighlightStyle { color: Some(theme.fg), font_weight: Some(FontWeight::BOLD), ..Default::default() }
        } else {
            HighlightStyle { color: Some(accent_text), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() }
        };
        let hl = |text: &str| {
            let ranges = highlight_ranges(text, q);
            StyledText::new(text.to_string()).with_highlights(ranges.into_iter().map(|r| (r, mark)))
        };
        let dot_color = match it.status {
            ItemStatus::Waiting => theme.warning,
            ItemStatus::Busy => theme.info,
            ItemStatus::Idle => mix_alpha(theme.fg_muted, 0.5),
            ItemStatus::Stopped => mix_alpha(theme.fg_muted, 0.3),
            ItemStatus::Archived => mix_alpha(theme.accent_alt, 0.6),
        };
        let dot = div().size(px(8.0)).rounded(px(4.0)).flex_none().bg(dot_color);
        let dot = if it.status == ItemStatus::Waiting {
            // waiting 是唯一「需要人动手」的一档：外发光 + 呼吸（attention-pulse 2s）
            let ring = mix_alpha(theme.warning, 0.3);
            dot.border_2()
                .border_color(ring)
                .size(px(12.0))
                .rounded(px(6.0))
                .ml(px(-2.0))
                .mr(px(-2.0))
                .with_animation(
                    SharedString::from(format!("pulse-{}", it.id)),
                    Animation::new(Duration::from_secs(2)).repeat(),
                    |d, t| d.opacity(0.55 + 0.45 * (1.0 - (t * std::f32::consts::TAU).cos()) / 2.0),
                )
                .into_any_element()
        } else {
            dot.into_any_element()
        };
        let pill = (shows_status_pill(it.status) && !it.status_label.is_empty()).then(|| {
            let c = if it.status == ItemStatus::Waiting { theme.warning } else { theme.accent_alt };
            div()
                .text_size(px(10.0))
                .line_height(px(15.0))
                .px(px(7.0))
                .py(px(2.0))
                .rounded_full()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .bg(mix_alpha(c, 0.18))
                .text_color(c)
                .child(it.status_label.clone())
        });
        let item = it.clone();
        let hover_idx = idx;
        div()
            .id(SharedString::from(format!("pi-{}", it.id)))
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .mx(px(6.0))
            .px(px(8.0))
            .py(px(5.0))
            .min_h(px(34.0))
            .rounded(px(RADIUS))
            .cursor_pointer()
            .text_size(px(13.0))
            .when(active, |d| {
                d.bg(theme.bg_active).child(
                    // 光标左轨：竖着的短胶囊 2×16
                    div().absolute().left_0().top(px(0.0)).bottom(px(0.0)).flex().items_center().child(div().w(px(2.0)).h(px(16.0)).rounded(px(1.0)).bg(theme.accent)),
                )
            })
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if !this.mouse_moved {
                    this.mouse_moved = true;
                }
                if this.active != hover_idx {
                    this.active = hover_idx;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                let m = ev.modifiers();
                let mode = if m.platform && m.shift {
                    Mode::SplitDown
                } else if m.platform {
                    Mode::SplitRight
                } else {
                    Mode::Default
                };
                this.activate(&item, mode, cx);
            }))
            // 前导定宽槽 20px：状态点 + ★，空着也占位，标题左缘是一条直线
            .child(
                div()
                    .w(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(dot)
                    .when(it.pinned, |d| d.child(div().text_size(px(9.0)).line_height(px(9.0)).text_color(theme.warning).child("★"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_color(theme.fg)
                            .line_height(relative(1.35))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(active, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(hl(&it.title)),
                    )
                    .when(!it.subtitle.is_empty(), |d| {
                        d.child(
                            div()
                                .text_color(theme.fg_subtle)
                                .text_size(px(11.0))
                                .line_height(relative(1.3))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(hl(&it.subtitle)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_none()
                    .children(pill)
                    .when(!it.hint.is_empty(), |d| {
                        d.child(div().text_color(theme.fg_subtle).text_size(px(11.0)).min_w(px(52.0)).flex().justify_end().child(it.hint.clone()))
                    }),
            )
            .into_any_element()
    }

    fn render_filter_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let label = |t: &'static str| {
            div().text_size(px(11.0)).text_color(theme.fg_subtle).mb(px(6.0)).font_weight(FontWeight::SEMIBOLD).child(t)
        };
        let row = |id: SharedString, checked: bool, radio: bool, glyph: Option<(&'static str, Hsla)>, text: &'static str| {
            let hover = theme.bg_hover;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.0))
                .mx(px(-6.0))
                .px(px(6.0))
                .py(px(4.0))
                .rounded(px(RADIUS))
                .text_size(px(12.0))
                .text_color(theme.fg)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(check_box(theme, checked, radio, 13.0))
                .when_some(glyph, |d, (g, c)| d.child(div().w(px(13.0)).flex_none().flex().justify_center().text_size(px(11.0)).text_color(c).child(g)))
                .child(text)
        };
        let section = || div().px(px(14.0)).py(px(12.0));
        let project_name = if self.filter.project.is_empty() { "全部".to_string() } else { project_label(&self.filter.project).to_string() };

        let time_rows = TimeFilter::ALL.map(|t| {
            row(SharedString::from(format!("ft-{}", t.as_str())), self.filter.time == t, true, None, t.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter.time = t;
                    this.save_filter(cx);
                }))
        });
        let status_rows = StatusKey::ALL.map(|k| {
            let c = match k {
                StatusKey::WaitingApproval | StatusKey::WaitingUser => theme.warning,
                StatusKey::Busy => theme.info,
                StatusKey::Archived => theme.accent_alt,
                _ => theme.fg_subtle,
            };
            row(SharedString::from(format!("fs-{}", k.as_str())), self.filter.status.contains(&k), false, Some((k.glyph(), c)), k.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter.toggle_status(k);
                    this.save_filter(cx);
                }))
        });
        let sort_rows = SortKey::ALL.map(|k| {
            row(SharedString::from(format!("fo-{}", k.as_str())), self.sort == k, true, None, k.label()).on_click(cx.listener(move |this, _, _, cx| this.set_sort(k, cx)))
        });
        let (hover_bg, strong) = (theme.bg_hover, theme.border_strong);

        div()
            .id("palette-filter-panel")
            .w(px(240.0))
            .flex_none()
            .bg(theme.bg)
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                div()
                    .h(px(48.0))
                    .flex_none()
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("筛选")
                    .child(icon_btn(theme, "filter-close", "×", 22.0).on_click(cx.listener(|this, _, _, cx| {
                        this.filter_open = false;
                        cx.notify();
                    }))),
            )
            .child(
                section().child(label("项目")).child(
                    div()
                        .id("filter-project")
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .bg(theme.bg_soft)
                        .text_color(theme.fg)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(RADIUS))
                        .px(px(6.0))
                        .py(px(5.0))
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(project_name))
                        .child(div().text_color(theme.fg_subtle).child("▾"))
                        .on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_project_menu(ev, window, cx))),
                ),
            )
            .child(section().border_t_1().border_color(theme.border).child(label("时间")).children(time_rows))
            .child(section().border_t_1().border_color(theme.border).child(label("运行状态（多选）")).children(status_rows))
            .child(
                section()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(label("标签"))
                    .child(row("f-pinned".into(), self.filter.pinned_only, false, None, "只看置顶").on_click(cx.listener(|this, _, _, cx| {
                        this.filter.pinned_only = !this.filter.pinned_only;
                        this.save_filter(cx);
                    })))
                    .child(label("排序").mt(px(12.0)))
                    .children(sort_rows),
            )
            .when(self.filter.has_filters(), |d| {
                d.child(
                    div()
                        .id("filter-clear")
                        .mx(px(14.0))
                        .my(px(12.0))
                        .px(px(12.0))
                        .py(px(6.0))
                        .flex()
                        .justify_center()
                        .bg(theme.bg_soft)
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.fg)
                        .rounded(px(RADIUS))
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).border_color(strong))
                        .child("清除全部筛选")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filter.clear();
                            this.save_filter(cx);
                        })),
                )
            })
            .into_any_element()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let items = self.items(cx);
        let filtered = filter_items(&items, &self.filter, &self.query, now());
        let groups = group_items(filtered, self.sort);
        let flat_len = visible_flat(&groups, &self.collapsed, &self.query).len();
        if self.active >= flat_len {
            self.active = flat_len.saturating_sub(1);
        }
        let q = self.query.trim().to_string();
        let cap = max_per_group(&self.query);
        let has_filters = self.filter.has_filters();

        // body 的孩子是一条扁平序列（历史 / 组头 / 条目），这样 ScrollHandle 能按下标把光标行滚进视野
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut active_child = None;
        if self.query.is_empty() && !self.history.is_empty() {
            let chips = self.history.iter().enumerate().map(|(i, h)| {
                let h2 = h.clone();
                let (hover, strong, fg) = (theme.bg_hover, theme.border_strong, theme.fg);
                div()
                    .id(("hist", i))
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.fg_muted)
                    .px(px(10.0))
                    .py(px(3.0))
                    .text_size(px(11.0))
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover).text_color(fg).border_color(strong))
                    .child(h.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.input.update(cx, |i, cx| i.set_text(h2.clone(), cx))))
            });
            body.push(
                div()
                    .py(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(14.0))
                            .pt(px(10.0))
                            .pb(px(4.0))
                            .text_size(px(11.0))
                            .text_color(theme.fg_subtle)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("搜索历史")
                            .child(
                                div()
                                    .id("hist-clear")
                                    .text_size(px(14.0))
                                    .px(px(4.0))
                                    .cursor_pointer()
                                    .hover({
                                        let fg = theme.fg;
                                        move |s| s.text_color(fg)
                                    })
                                    .child("×")
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
                            ),
                    )
                    .child(div().flex().flex_wrap().gap(px(6.0)).px(px(14.0)).pt(px(4.0)).pb(px(8.0)).children(chips))
                    .into_any_element(),
            );
        }
        if groups.is_empty() {
            let (hover, strong, fg) = (theme.bg_hover, theme.border_strong, theme.fg);
            body.push(
                div()
                    .pt(px(44.0))
                    .pb(px(48.0))
                    .px(px(16.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .child(if q.is_empty() {
                        div().text_color(theme.fg_muted).text_size(px(13.0)).child("这里什么都没有")
                    } else {
                        div()
                            .flex()
                            .text_color(theme.fg_muted)
                            .text_size(px(13.0))
                            .child("没有匹配 ")
                            .child(div().text_color(theme.var("--accent-text")).font_weight(FontWeight::SEMIBOLD).child(q.clone()))
                            .child(" 的结果")
                    })
                    // 筛选是跨次启动留着的：上次筛了「只看置顶」忘了清，这里是唯一该说这句话的地方
                    .when(has_filters, |d| {
                        d.child(
                            div()
                                .id("empty-clear")
                                .border_1()
                                .border_color(theme.border)
                                .rounded_full()
                                .text_color(theme.fg_muted)
                                .px(px(12.0))
                                .py(px(4.0))
                                .text_size(px(11.0))
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover).text_color(fg).border_color(strong))
                                .child("当前有筛选生效，点这里清除")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.filter.clear();
                                    this.save_filter(cx);
                                })),
                        )
                    })
                    .into_any_element(),
            );
        }
        let mut idx = 0;
        for g in &groups {
            let collapsed = self.collapsed.contains(&g.name);
            if !g.name.is_empty() {
                let name = g.name.clone();
                let (fg, hover) = (theme.fg, theme.bg_hover);
                let chevron = svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size(px(9.0))
                    .text_color(theme.fg_subtle)
                    .with_transformation(Transformation::rotate(gpui::radians(if collapsed { 0.0 } else { std::f32::consts::FRAC_PI_2 })));
                body.push(
                    div()
                        .id(SharedString::from(format!("grp-{}", g.name)))
                        .mt(px(2.0))
                        .mx(px(6.0))
                        .px(px(8.0))
                        .pt(px(10.0))
                        .pb(px(4.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(RADIUS))
                        .text_size(px(11.0))
                        .text_color(theme.fg_subtle)
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .hover(move |s| s.text_color(fg).bg(hover))
                        .child(div().w(px(12.0)).flex_none().flex().justify_center().child(chevron))
                        .child(g.name.clone())
                        .child(
                            div()
                                .ml_auto()
                                .bg(theme.bg_hover)
                                .text_color(theme.fg_subtle)
                                .px(px(7.0))
                                .py(px(1.0))
                                .rounded_full()
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(g.items.len().to_string()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(name.clone(), cx)))
                        .into_any_element(),
                );
            }
            if collapsed {
                continue;
            }
            for it in g.items.iter().take(cap) {
                if idx == self.active {
                    active_child = Some(body.len());
                }
                let el = self.render_item(it, idx, &q, &theme, cx);
                body.push(el);
                idx += 1;
            }
        }
        if self.scroll_to_active {
            self.scroll_to_active = false;
            if let Some(c) = active_child {
                self.scroll.scroll_to_item(c);
            }
        }

        let vp = window.viewport_size();
        let accent_text = theme.var("--accent-text");
        let filter_btn = {
            let (hover, fg) = (theme.bg_hover, theme.fg);
            let open = self.filter_open;
            div()
                .id("palette-filter-btn")
                .flex()
                .items_center()
                .gap(px(5.0))
                .border_1()
                .rounded_full()
                .px(px(10.0))
                .py(px(4.0))
                .text_size(px(12.0))
                .line_height(px(12.0))
                .flex_none()
                .cursor_pointer()
                .map(|d| {
                    if open {
                        d.bg(theme.bg_active).text_color(accent_text).border_color(mix_alpha(theme.accent, 0.45))
                    } else {
                        d.border_color(theme.border).text_color(theme.fg_muted).hover(move |s| s.bg(hover).text_color(fg))
                    }
                })
                .child(svg().path(icons::FILTER).size(px(14.0)).text_color(if open { accent_text } else { theme.fg_muted }))
                .child("筛选")
                .when(has_filters, |d| d.child(div().size(px(5.0)).rounded(px(3.0)).bg(theme.warning).flex_none()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_open = !this.filter_open;
                    cx.notify();
                }))
        };
        let footer_kbd = |k: &'static str| kbd(&theme, k, theme.fg_muted, 5.0).mr(px(4.0));
        let hint = |k: &'static str, t: &'static str| div().flex().items_center().child(footer_kbd(k)).child(t);

        div()
            .id("palette-backdrop")
            .key_context("Overlay CommandPalette")
            .absolute()
            .size_full()
            .bg(theme.scrim)
            .flex()
            .justify_center()
            .items_start()
            .pt(vp.height * 0.12)
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))
            .on_action(cx.listener(|_, _: &act::Dismiss, _, cx| cx.emit(PaletteEvent::Close)))
            .on_action(cx.listener(|this, _: &act::SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &act::SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &act::Confirm, _, cx| this.confirm(Mode::Default, cx)))
            .on_action(cx.listener(|this, _: &act::ConfirmSplitRight, _, cx| this.confirm(Mode::SplitRight, cx)))
            .on_action(cx.listener(|this, _: &act::ConfirmSplitDown, _, cx| this.confirm(Mode::SplitDown, cx)))
            .child(
                div()
                    .id("palette")
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .w(relative(0.9))
                    .max_w(px(if self.filter_open { 980.0 } else { 760.0 }))
                    .max_h(vp.height * 0.7)
                    .bg(theme.bg_soft)
                    .border_1()
                    .border_color(theme.border_strong)
                    .rounded(px(RADIUS_LG))
                    .shadow(vec![shadow(2.0, 6.0, theme.shadow), shadow(16.0, 48.0, theme.var("--shadow-strong"))])
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .h(px(48.0))
                                    .flex_none()
                                    .px(px(14.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(svg().path(icons::SEARCH).size(px(16.0)).flex_none().text_color(theme.fg_subtle))
                                    .child(div().flex_1().min_w_0().text_size(px(15.0)).line_height(px(20.0)).text_color(theme.fg).child(self.input.clone()))
                                    .child(filter_btn)
                                    .child(icon_btn(&theme, "palette-close", "×", 22.0).on_click(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))),
                            )
                            .child(
                                // 滚动条是滚动容器的同级、绝对定位（见 scrollbar.rs 文件头）
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .id("palette-body")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.scroll)
                                            .pt(px(2.0))
                                            .pb(px(6.0))
                                            .children(body),
                                    )
                                    .child(self.scrollbar.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(14.0))
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .text_size(px(11.0))
                                    .text_color(theme.fg_subtle)
                                    .child(hint("↑↓", "选择"))
                                    .child(hint("↵", "打开"))
                                    .child(hint("⌘↵", "拆分"))
                                    .child(hint("⌘⇧↵", "新容器"))
                                    .child(hint("Esc", "取消").ml_auto()),
                            ),
                    )
                    .when(self.filter_open, |d| d.child(self.render_filter_panel(&theme, cx))),
            )
    }
}
