//! 设置面板 ⌘,（照 App.tsx:2424-2686 + App.css `.settings-*`）。Esc 能关（#224：Tauri 版不能）。
//!
//! 不迁的：「列表显示 → 顶部需要操作区域」开关（`makit-show-attention`，没有任何地方读，#224 死开关）。
//! 系统通知的授权状态 / 请求授权 / 测试通知归 E 通知包：它调 `set_notify_hooks` 接进来，没接之前显示「读取中…」。

use crate::tr;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, App, AsyncApp, ClickEvent, Context, Entity, EventEmitter, FocusHandle, FontWeight, Global, PathPromptOptions,
    PromptButton, PromptLevel, SharedString, Window,
};

use super::shortcuts::{shortcut_groups, ShortcutRow};
use super::style::*;
use super::MenuItem;
use crate::actions::{keymap, overlays as act};
use rust_i18n::t;
use crate::state::AppState;
use crate::theme::derive::{is_light, ThemeSource};
use crate::theme::{builtin::builtin_themes, itermcolors::import_itermcolors, ActiveTheme, Theme};

// ---------- 纯逻辑 ----------

// pane 落点图标套的数据在 `workspace::flash`（闪牌也用它），这里只引用，不再留第二份
use crate::workspace::flash::{pane_icons_for, PANE_ICON_SETS};

/// 导入的主题按 id 同名覆盖（位置不变），新的追加在后
pub fn upsert_imported(list: &[ThemeSource], t: ThemeSource) -> Vec<ThemeSource> {
    let mut out = list.to_vec();
    match out.iter_mut().find(|x| x.id == t.id) {
        Some(x) => *x = t,
        None => out.push(t),
    }
    out
}

/// 主题菜单里的显示顺序：深色内置 → 浅色内置 → 导入（和 `open_theme_menu` 一致）
pub fn theme_menu_order(imported: &[ThemeSource]) -> Vec<String> {
    let builtin = builtin_themes();
    let dark = builtin.iter().filter(|t| !is_light(&t.bg));
    let light = builtin.iter().filter(|t| is_light(&t.bg));
    dark.chain(light).chain(imported.iter()).map(|t| t.id.clone()).collect()
}

/// 删掉导入的 `deleted` 后该用哪个主题：删的正是当前主题 → 菜单里它上面那一个（排第一就用下面那个）；删的不是当前 → 保持当前
pub fn theme_after_delete(order: &[String], current: &str, deleted: &str) -> String {
    if current != deleted {
        return current.to_string();
    }
    let i = order.iter().position(|id| id == deleted).unwrap_or(0);
    let neighbour = if i > 0 { order.get(i - 1) } else { order.get(1) };
    neighbour.cloned().unwrap_or_else(|| builtin_themes().remove(0).id)
}

/// 找主题、下载 `.itermcolors` 的网站（iTerm2-Color-Schemes 的官方继任站）。批量下载用它的 GitHub 仓库
pub const THEME_SITE: &str = "https://terminalthemes.com/";

/// 主题下拉列表最宽多少（选择框更宽时也不跟着撑满）
const THEME_MENU_MAX_W: f32 = 560.0;

/// 删掉导入的主题 `deleted` 之后要不要切换主题（#256 A6）：只有**主动选了它、而且它正是当前主题**才要切到菜单里它上面那个；
/// 没主动选过（跟随系统）时存着的 id 不是实际在用的，删别的主题也不该让自动模式变成固定
pub fn switch_after_delete(order: &[String], chosen: bool, current: &str, deleted: &str) -> Option<String> {
    (chosen && current == deleted).then(|| theme_after_delete(order, current, deleted))
}

pub const HOVER_MODES: [(&str, &str); 3] = [("always", "settings.hover.always"), ("cmd", "settings.hover.cmd"), ("off", "settings.hover.off")];

// ---------- 通知包的接口 ----------

/// E 通知包提供的系统通知能力。`permission`：Some(true) 已允许 / Some(false) 未允许 / None 读取中
#[derive(Clone)]
pub struct NotifyHooks {
    pub permission: Rc<dyn Fn(&App) -> Option<bool>>,
    pub request_permission: Rc<dyn Fn(&mut Window, &mut App)>,
    /// 发一条测试横幅；未授权时由实现方自己弹 warning 对话框
    pub send_test: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl Global for NotifyHooks {}

pub fn set_notify_hooks(h: NotifyHooks, cx: &mut App) {
    cx.set_global(h);
}

// ---------- 视图 ----------

pub enum SettingsEvent {
    Close,
}

pub struct SettingsView {
    state: Entity<AppState>,
    focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    scrollbar: Entity<crate::scrollbar::Scrollbar>,
    /// 主题选择框这一帧在窗口里的位置（`on_children_prepainted` 写入），下拉列表贴着它展开
    theme_select: Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

/// 系统对话框（NSAlert）提示一句话
fn message(window: &mut Window, cx: &mut App, level: PromptLevel, title: &str, detail: &str) {
    let _ = window.prompt(level, title, Some(detail), &[PromptButton::ok(tr!("common.ok"))], cx);
}

impl SettingsView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let scroll = gpui::ScrollHandle::new();
        let scrollbar = crate::scrollbar::Scrollbar::handle(&scroll, cx);
        Self { state, focus, scroll, scrollbar, theme_select: Rc::new(std::cell::Cell::new(None)) }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn apply_theme(state: &Entity<AppState>, id: String, imported: Option<Vec<ThemeSource>>, cx: &mut App) {
        state.update(cx, |s, cx| {
            s.update_prefs(cx, |p| {
                p.theme.id = id;
                p.theme.chosen = true; // 主动选了主题：不再跟系统外观（#256 A6）
                if let Some(v) = imported {
                    p.theme.imported = v;
                }
            })
        });
        crate::theme::auto::refresh(state, cx);
    }

    /// 「跟随系统」：把「主动选过」清掉，主题回到按系统外观自动选（#256 A6）
    fn follow_system(state: &Entity<AppState>, cx: &mut App) {
        state.update(cx, |s, cx| s.update_prefs(cx, |p| p.theme.chosen = false));
        crate::theme::auto::refresh(state, cx);
    }

    fn open_theme_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        // 贴着选择框展开：左边对齐、紧挨下方、宽度和选择框一致（最宽 THEME_MENU_MAX_W）；拿不到位置才退回鼠标点的地方
        let (pos, min_w) = match self.theme_select.get() {
            Some(b) => (gpui::point(b.origin.x, b.origin.y + b.size.height + px(2.0)), Some(f32::from(b.size.width).min(THEME_MENU_MAX_W))),
            None => (ev.position(), None),
        };
        super::show_searchable_menu(pos, &tr!("settings.theme.search"), min_w, window, cx, move |cx| {
            let p = &state.read(cx).prefs.theme;
            let (current, chosen) = (p.id.clone(), p.chosen);
            let pick = |t: &ThemeSource| {
                let (state, id) = (state.clone(), t.id.clone());
                let mut colors = vec![crate::theme::to_hsla(&t.bg)];
                colors.extend((1..=6).filter_map(|i| t.ansi.get(i)).map(|c| crate::theme::to_hsla(c)));
                MenuItem::action(t.name.clone(), move |_, cx| Self::apply_theme(&state, id.clone(), None, cx)).checked(chosen && t.id == current).preview(colors)
            };
            let builtin = builtin_themes();
            let follow = state.clone();
            let mut items = vec![MenuItem::action(tr!("settings.language.system"), move |_, cx| Self::follow_system(&follow, cx)).checked(!chosen), MenuItem::header(tr!("settings.theme.dark"))];
            items.extend(builtin.iter().filter(|t| !is_light(&t.bg)).map(pick));
            items.push(MenuItem::header(tr!("settings.theme.light")));
            items.extend(builtin.iter().filter(|t| is_light(&t.bg)).map(pick));
            if !p.imported.is_empty() {
                items.push(MenuItem::header(tr!("settings.theme.import")));
                items.extend(p.imported.iter().map(|t| {
                    let (state, id) = (state.clone(), t.id.clone());
                    pick(t).removable(move |_, cx| Self::delete_imported_theme(&state, &id, cx))
                }));
            }
            items
        });
    }

    /// 选文件 → 解析 → 同名覆盖存下 → 立即切过去；失败弹系统错误框「导入失败」
    fn import_itermcolors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(tr!("settings.theme.import").into()) });
        let (state, handle) = (self.state.clone(), window.window_handle());
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let result = std::fs::read_to_string(&path).map_err(|e| t!("settings.theme.read_failed", error = e).to_string()).and_then(|xml| import_itermcolors(&name, &xml));
            let _ = cx.update_window(handle, |_, window, cx| match result {
                Ok(t) => {
                    let list = upsert_imported(&state.read(cx).prefs.theme.imported, t.clone());
                    Self::apply_theme(&state, t.id, Some(list), cx);
                }
                Err(e) => message(window, cx, PromptLevel::Critical, &tr!("settings.theme.import_failed"), &e),
            });
        })
        .detach();
    }

    /// 删掉一个导入的主题；删的正是主动选中的当前主题就换成菜单里它上面的那一个（`switch_after_delete`），否则只是从列表里去掉
    fn delete_imported_theme(state: &Entity<AppState>, id: &str, cx: &mut App) {
        let p = state.read(cx).prefs.theme.clone();
        let next = switch_after_delete(&theme_menu_order(&p.imported), p.chosen, &p.id, id);
        let list: Vec<ThemeSource> = p.imported.into_iter().filter(|t| t.id != id).collect();
        match next {
            Some(next) => Self::apply_theme(state, next, Some(list), cx),
            None => {
                state.update(cx, |s, cx| s.update_prefs(cx, |pp| pp.theme.imported = list));
                crate::theme::auto::refresh(state, cx);
            }
        }
    }

    fn open_language_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        super::show_context_menu(ev.position(), window, cx, move |cx| {
            let cur = state.read(cx).prefs.language.clone();
            [crate::i18n::PREF_SYSTEM, crate::i18n::PREF_ZH, crate::i18n::PREF_EN]
                .into_iter()
                .map(|pref| {
                    let state = state.clone();
                    MenuItem::action(language_name(pref), move |_, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.language = pref.to_string())))
                        .checked(language_matches(&cur, pref))
                })
                .collect()
        });
    }

    fn open_icon_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        super::show_context_menu(ev.position(), window, cx, move |cx| {
            let cur = state.read(cx).prefs.pane_icons.clone();
            PANE_ICON_SETS
                .iter()
                .map(|set| {
                    let (state, id) = (state.clone(), set.id.to_string());
                    let checked = id == cur;
                    MenuItem::action(set.name, move |_, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.pane_icons = id.clone()))).checked(checked)
                })
                .collect()
        });
    }

    fn open_hover_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        super::show_context_menu(ev.position(), window, cx, move |cx| {
            let cur = state.read(cx).prefs.sidebar.hover_mode.clone();
            HOVER_MODES
                .iter()
                .map(|(id, label)| {
                    let (state, id) = (state.clone(), id.to_string());
                    let checked = id == cur;
                    MenuItem::action(t!(*label).to_string(), move |_, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.hover_mode = id.clone()))).checked(checked)
                })
                .collect()
        });
    }

    fn install_hook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let r = cx.background_executor().spawn(async { makit_core::hook::install_claude_hook() }).await;
            match &r {
                Ok(s) => log::info!(target: "hook", "安装 Claude Code Hook：{s}"),
                Err(e) => log::error!(target: "hook", "安装 Claude Code Hook 失败：{e}"),
            }
            let _ = cx.update_window(handle, |_, window, cx| match r {
                Ok(s) if s == "already_installed" => message(window, cx, PromptLevel::Info, &tr!("settings.hook.title"), &tr!("settings.hook.already")),
                Ok(_) => message(window, cx, PromptLevel::Info, &tr!("settings.hook.title"), &tr!("settings.hook.done")),
                Err(e) => message(window, cx, PromptLevel::Critical, &tr!("settings.hook.failed"), &e),
            });
        })
        .detach();
    }
}

/// Menu / select label for a language preference (the two languages are named in themselves, so they stay readable after a wrong switch).
fn language_name(pref: &str) -> String {
    match pref {
        crate::i18n::PREF_ZH => t!("settings.language.zh").to_string(),
        crate::i18n::PREF_EN => t!("settings.language.en").to_string(),
        _ => t!("settings.language.system").to_string(),
    }
}

/// Whether the stored preference is the menu entry `pref` (unknown stored values count as "follow system").
fn language_matches(stored: &str, pref: &str) -> bool {
    match stored {
        crate::i18n::PREF_ZH | crate::i18n::PREF_EN => stored == pref,
        _ => pref == crate::i18n::PREF_SYSTEM,
    }
}

fn select_box(theme: &Theme, id: &'static str, label: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    // `.settings-select`：--bg 底、--border 边、6px 8px、12px
    div()
        .id(id)
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .bg(theme.bg)
        .text_color(theme.fg)
        .border_1()
        .border_color(theme.border)
        .rounded(px(RADIUS))
        .px(px(8.0))
        .py(px(6.0))
        .text_size(px(12.0))
        .cursor_pointer()
        .child(label.into())
        .child(div().text_color(theme.fg_subtle).child("▾"))
}

fn shortcut_row(theme: &Theme, r: &ShortcutRow) -> gpui::Div {
    let key = |k: &String| kbd(theme, k.clone(), theme.fg, 6.0);
    let keys = if r.range {
        div().flex().items_center().justify_end().gap(px(3.0)).child(key(&r.keys[0])).child(div().opacity(0.45).text_size(px(10.0)).child("–")).child(key(&r.keys[1]))
    } else {
        div().flex().flex_wrap().items_center().justify_end().gap(px(3.0)).children(r.keys.iter().map(key))
    };
    div().flex().items_center().gap(px(8.0)).text_size(px(12.0)).text_color(theme.fg_muted).child(keys.w(px(104.0)).flex_none()).child(r.desc.clone())
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let prefs = self.state.read(cx).prefs.clone();
        let vp = window.viewport_size();
        let label = |t: &str| div().text_size(px(11.0)).text_color(theme.fg_muted).mb(px(8.0)).font_weight(FontWeight::MEDIUM).child(t.to_string());
        let hint = |t: &str| div().text_size(px(11.0)).text_color(theme.fg_muted).mt(px(6.0)).child(t.to_string());
        let section = || div().px(px(16.0)).py(px(12.0)).border_b_1().border_color(theme.border);
        let toggle = |id: &'static str, on: bool, text: gpui::SharedString| {
            div().id(id).flex().items_center().gap(px(8.0)).text_size(px(13.0)).cursor_pointer().child(check_box(&theme, on, false, 16.0)).child(text)
        };
        let set_notify = |f: fn(&mut crate::persist::state::NotifyPrefs)| {
            cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| this.state.update(cx, |s, cx| s.update_prefs(cx, |p| f(&mut p.notify))))
        };
        let effective_id = crate::theme::auto::effective_theme_id(prefs.theme.chosen, &prefs.theme.id, crate::theme::auto::system_is_dark(cx));
        let theme_name = Theme::by_id(effective_id, &prefs.theme.imported).source.name;
        let theme_name = if prefs.theme.chosen { theme_name } else { t!("settings.theme.follow_system", name = theme_name).to_string() };
        let swatches = (0..8).map(|i| div().flex_1().min_w_0().h(px(14.0)).rounded(px(2.0)).bg(theme.ansi[i]).border_1().border_color(theme.border));
        let icons = pane_icons_for(&prefs.pane_icons);
        let icon_name = PANE_ICON_SETS.iter().find(|s| s.id == prefs.pane_icons).unwrap_or(&PANE_ICON_SETS[0]).name;
        let hover_name = t!(HOVER_MODES.iter().find(|m| m.0 == prefs.sidebar.hover_mode).unwrap_or(&HOVER_MODES[0]).1).to_string();
        let hooks = cx.try_global::<NotifyHooks>().cloned();
        let permission = hooks.as_ref().and_then(|h| (h.permission)(cx));
        let accent_text = theme.var("--accent-text");

        let groups = shortcut_groups(&keymap());
        let shortcut_groups_el = groups.iter().enumerate().map(|(i, g)| {
            div()
                .when(i + 1 < groups.len(), |d| d.mb(px(12.0)))
                .child(div().text_size(px(10.0)).text_color(accent_text).font_weight(FontWeight::MEDIUM).mb(px(6.0)).child(g.title))
                .child(div().flex().flex_col().gap(px(6.0)).children(g.rows.iter().map(|r| shortcut_row(&theme, r))))
        });

        div()
            .id("settings-backdrop")
            .key_context("Overlay Settings")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &act::Dismiss, _, cx| cx.emit(SettingsEvent::Close)))
            .absolute()
            .size_full()
            .bg(theme.scrim)
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
            .child(
                div()
                    .id("settings-modal")
                    .occlude()
                    .w((vp.width * 0.5).max(px(420.0)).min(px(700.0)))
                    .max_h(vp.height * 0.8)
                    .bg(theme.bg_soft)
                    .border_1()
                    .border_color(theme.border_strong)
                    .rounded(px(RADIUS_LG))
                    .shadow(vec![shadow(12.0, 40.0, theme.var("--shadow-strong"))])
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr!("settings.title"))
                            .child(
                                div()
                                    .id("settings-close")
                                    .px(px(6.0))
                                    .text_size(px(18.0))
                                    .text_color(theme.fg_muted)
                                    .cursor_pointer()
                                    .hover({
                                        let fg = theme.fg;
                                        move |s| s.text_color(fg)
                                    })
                                    .child("×")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                            ),
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
                            .id("settings-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .py(px(8.0))
                            // Language (#71)
                            .child(
                                section()
                                    .child(label(&t!("settings.language.title")))
                                    .child(select_box(&theme, "language-select", language_name(&prefs.language)).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_language_menu(ev, window, cx))))
                                    .child(hint(&t!("settings.language.hint"))),
                            )
                            // 外观主题
                            .child(
                                section()
                                    .child(label(&tr!("settings.theme.title")))
                                    .child({
                                        let cell = self.theme_select.clone();
                                        div()
                                            .w_full()
                                            .on_children_prepainted(move |b, _, _| cell.set(b.first().copied()))
                                            .child(select_box(&theme, "theme-select", theme_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_theme_menu(ev, window, cx))))
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(4.0))
                                            .mt(px(8.0))
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .bg(theme.bg)
                                            .border_1()
                                            .border_color(theme.border)
                                            .rounded(px(RADIUS))
                                            .children(swatches),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .mt(px(8.0))
                                            .child(action_btn(&theme, "import-iterm", tr!("settings.theme.import_iterm")).on_click(cx.listener(|this, _, window, cx| this.import_itermcolors(window, cx))))
                                            .child(action_btn(&theme, "get-themes", tr!("settings.theme.download")).on_click(|_, _, cx| cx.open_url(THEME_SITE)))
                                    )
                                    .child(hint(&tr!("settings.theme.hint"))),
                            )
                            // pane 落点图标
                            .child(
                                section()
                                    .child(label(&tr!("settings.icons.title")))
                                    .child(select_box(&theme, "icon-select", icon_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_icon_menu(ev, window, cx))))
                                    .child(if icons.is_empty() {
                                        div().mt(px(6.0)).child(hint(&tr!("settings.icons.none_hint")))
                                    } else {
                                        div().flex().flex_wrap().gap(px(8.0)).mt(px(6.0)).children(icons.iter().enumerate().map(|(i, ic)| {
                                            div()
                                                .w(px(24.0))
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .gap(px(1.0))
                                                .child(div().text_size(px(17.0)).line_height(px(19.0)).child(*ic))
                                                .child(div().text_size(px(9.0)).text_color(theme.fg_muted).child((i + 1).to_string()))
                                        }))
                                    })
                                    .child(hint(&tr!("settings.icons.hint"))),
                            )
                            // 系统通知
                            .child(
                                section()
                                    .child(label(&tr!("settings.notify.title")))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.0))
                                            .child(toggle("n-system", prefs.notify.system, tr!("settings.notify.enable")).on_click(set_notify(|n| n.system = !n.system)))
                                            .child(toggle("n-approval", prefs.notify.approval, tr!("settings.notify.approval")).on_click(set_notify(|n| n.approval = !n.approval)))
                                            .child(toggle("n-user", prefs.notify.user, tr!("settings.notify.user")).on_click(set_notify(|n| n.user = !n.user)))
                                            // #215 新增的两项（Tauri 版没有）：不给开关的话「已完成」横幅和提示音就关不掉
                                            .child(toggle("n-completed", prefs.notify.completed, tr!("settings.notify.completed")).on_click(set_notify(|n| n.completed = !n.completed)))
                                            .child(toggle("n-sound", prefs.notify.sound, tr!("settings.notify.sound")).on_click(set_notify(|n| n.sound = !n.sound))),
                                    )
                                    .child(hint(&tr!("settings.notify.hint")))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .text_size(px(11.0))
                                            .text_color(theme.fg_muted)
                                            .mt(px(6.0))
                                            .child(tr!("settings.notify.permission"))
                                            .map(|d| match permission {
                                                Some(true) => d.child(div().font_weight(FontWeight::BOLD).child(tr!("settings.notify.allowed"))),
                                                Some(false) => d
                                                    .child(div().font_weight(FontWeight::BOLD).child(tr!("settings.notify.denied")))
                                                    .child(tr!("settings.notify.denied_hint"))
                                                    .child(
                                                        div()
                                                            .id("req-perm")
                                                            .text_color(accent_text)
                                                            .underline()
                                                            .cursor_pointer()
                                                            .child(tr!("settings.notify.request"))
                                                            .on_click(cx.listener(|_, _, window, cx| {
                                                                if let Some(h) = cx.try_global::<NotifyHooks>().cloned() {
                                                                    (h.request_permission)(window, cx);
                                                                }
                                                            })),
                                                    ),
                                                None => d.child(tr!("settings.notify.loading")),
                                            }),
                                    )
                                    .child(action_btn(&theme, "send-test", tr!("settings.notify.send_test")).mt(px(8.0)).on_click(cx.listener(|_, _, window, cx| {
                                        match cx.try_global::<NotifyHooks>().cloned() {
                                            Some(h) => (h.send_test)(window, cx),
                                            None => message(window, cx, PromptLevel::Warning, &tr!("settings.notify.send_failed"), &tr!("settings.notify.not_wired")),
                                        }
                                    })))
                                    .child(hint(&tr!("settings.notify.test_hint")))
                                    .child(action_btn(&theme, "install-hook", tr!("settings.hook.install")).mt(px(8.0)).on_click(cx.listener(|this, _, window, cx| this.install_hook(window, cx))))
                                    .child(hint(&tr!("settings.hook.hint"))),
                            )
                            // 诊断（#254）：出问题时把日志目录和诊断信息交出去
                            .child(
                                section()
                                    .child(label(&tr!("settings.diagnostics.title")))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(8.0))
                                            .child(action_btn(&theme, "open-logs", tr!("settings.diagnostics.open_logs")).on_click(|_, _, cx| {
                                                if let Some(dir) = crate::logging::log_dir() {
                                                    let _ = std::fs::create_dir_all(&dir);
                                                    cx.reveal_path(&dir);
                                                }
                                            }))
                                            .child(action_btn(&theme, "copy-diagnostics", tr!("settings.diagnostics.copy")).on_click(|_, _, cx| {
                                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(crate::logging::diagnostics_text()));
                                                super::show_toast(tr!("settings.diagnostics.copied"), super::toast::COPY_OK, cx);
                                            })),
                                    )
                                    .child(hint(&tr!("settings.diagnostics.hint"))),
                            )
                            // 关于（#199）：提 issue 时要报版本号
                            .child(
                                section()
                                    .child(label(&tr!("settings.about.title")))
                                    .child(div().text_size(px(12.0)).text_color(theme.fg).child(concat!("makit ", env!("CARGO_PKG_VERSION"))))
                                    .child(hint(&tr!("settings.about.hint"))),
                            )
                            // 对话视图（#231）
                            .child(
                                section()
                                    .child(label(&tr!("settings.reflow.title")))
                                    .child(toggle("reflow-view", prefs.reflow_view, tr!("settings.reflow.toggle")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.reflow_view = !p.reflow_view)))))
                                    .child(hint(&tr!("settings.reflow.hint"))),
                            )
                            // Hover 详情卡片
                            .child(
                                section()
                                    .child(label(&tr!("settings.hover.title")))
                                    .child(select_box(&theme, "hover-select", hover_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_hover_menu(ev, window, cx))))
                                    .child(hint(&tr!("settings.hover.hint"))),
                            )
                            // 快捷键（读唯一的快捷键表）
                            .child(div().px(px(16.0)).py(px(12.0)).child(label(&tr!("settings.shortcuts.title"))).children(shortcut_groups_el))
                            )
                            .child(self.scrollbar.clone()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_icon_sets_have_nine_or_none() {
        for s in PANE_ICON_SETS {
            assert!(s.icons.len() == 9 || (s.id == "none" && s.icons.is_empty()), "{}", s.id);
        }
        assert_eq!(pane_icons_for("fruits")[0], "🍎");
        assert_eq!(pane_icons_for("???")[0], "🐉", "认不出的回到默认套");
    }

    #[test]
    fn imported_theme_same_name_overwrites_in_place() {
        let t = |id: &str, bg: &str| ThemeSource { id: id.into(), name: id.into(), bg: bg.into(), fg: "#fff".into(), ansi: vec![], selection: None };
        let list = vec![t("imported:a", "#000"), t("imported:b", "#111")];
        let out = upsert_imported(&list, t("imported:a", "#222"));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bg, "#222", "同名覆盖、位置不变");
        let out = upsert_imported(&list, t("imported:c", "#333"));
        assert_eq!(out.last().unwrap().id, "imported:c");
    }

    fn imported(id: &str) -> ThemeSource {
        ThemeSource { id: id.into(), name: id.into(), bg: "#000000".into(), fg: "#ffffff".into(), ansi: vec![], selection: None }
    }

    /// 为什么要测：删掉正在用的导入主题后，错了在界面上就是回到一个莫名其妙的主题（以前总是回到第一个深色），或者指向已被删掉的 id 导致配色错乱
    #[test]
    fn deleting_current_imported_theme_falls_back_to_the_one_above() {
        let list = [imported("imported:a"), imported("imported:b"), imported("imported:c")];
        let order = theme_menu_order(&list);
        assert_eq!(theme_after_delete(&order, "imported:b", "imported:b"), "imported:a", "上面是上一个导入的");
        assert_eq!(theme_after_delete(&order, "imported:c", "imported:c"), "imported:b");
        let above_first = theme_after_delete(&order, "imported:a", "imported:a");
        let light_last = order[order.iter().position(|i| i == "imported:a").unwrap() - 1].clone();
        assert_eq!(above_first, light_last, "第一个导入的上面是浅色内置的最后一套");
        assert!(!above_first.starts_with("imported:"));
    }

    #[test]
    fn deleting_another_imported_theme_keeps_the_current_one() {
        let list = [imported("imported:a"), imported("imported:b")];
        let order = theme_menu_order(&list);
        assert_eq!(theme_after_delete(&order, "dracula", "imported:b"), "dracula");
        assert_eq!(theme_after_delete(&order, "imported:a", "imported:b"), "imported:a");
    }

    /// 为什么要测：跟随系统时删一个导入的主题，如果被当成「选了新主题」，自动模式就悄悄变成固定了
    #[test]
    fn deleting_a_theme_only_switches_when_the_current_chosen_one_goes() {
        let order = theme_menu_order(&[imported("imported:a"), imported("imported:b")]);
        assert_eq!(switch_after_delete(&order, true, "imported:b", "imported:b"), Some("imported:a".to_string()), "选了它又删了它：切到上面那个");
        assert_eq!(switch_after_delete(&order, true, "nord", "imported:b"), None, "删的不是当前主题：不动");
        assert_eq!(switch_after_delete(&order, false, "imported:b", "imported:b"), None, "跟随系统时存着的 id 不是实际在用的：不动，自动模式保持");
        assert_eq!(switch_after_delete(&order, false, "nord", "imported:a"), None);
    }
}
