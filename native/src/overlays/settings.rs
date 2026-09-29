//! 设置面板 ⌘,（照 App.tsx:2424-2686 + App.css `.settings-*`）。Esc 能关（#224：Tauri 版不能）。
//!
//! 不迁的：「列表显示 → 顶部需要操作区域」开关（`makit-show-attention`，没有任何地方读，#224 死开关）。
//! 系统通知的授权状态 / 请求授权 / 测试通知归 E 通知包：它调 `set_notify_hooks` 接进来，没接之前显示「读取中…」。

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
use crate::state::AppState;
use crate::theme::derive::{is_light, ThemeSource};
use crate::theme::{builtin::builtin_themes, itermcolors::import_itermcolors, set_theme, ActiveTheme, Theme};

// ---------- 纯逻辑 ----------

/// pane 落点图标套（`src/paneIcons.ts`）。九个对应 ⌥⌘1~⌥⌘9；空 = 不显示
pub const PANE_ICON_SETS: [(&str, &str, &[&str]); 5] = [
    ("beasts", "灵兽", &["🐉", "🐯", "🦊", "🐳", "🦉", "🐝", "🦄", "🐙", "🐺"]),
    ("flowers", "花木", &["🌸", "🌹", "🌻", "🌷", "🌺", "🌼", "🪷", "💐", "🌾"]),
    ("fruits", "果园", &["🍎", "🍊", "🍋", "🍇", "🍓", "🍑", "🥝", "🍒", "🥭"]),
    ("dots", "色点", &["🔴", "🟠", "🟡", "🟢", "🔵", "🟣", "🟤", "⚫️", "⚪️"]),
    ("none", "不显示", &[]),
];

/// 按 id 取图标；认不出的用第一套（`paneIconsFor`）
pub fn pane_icons_for(id: &str) -> &'static [&'static str] {
    PANE_ICON_SETS.iter().find(|s| s.0 == id).unwrap_or(&PANE_ICON_SETS[0]).2
}

/// 导入的主题按 id 同名覆盖（位置不变），新的追加在后
pub fn upsert_imported(list: &[ThemeSource], t: ThemeSource) -> Vec<ThemeSource> {
    let mut out = list.to_vec();
    match out.iter_mut().find(|x| x.id == t.id) {
        Some(x) => *x = t,
        None => out.push(t),
    }
    out
}

pub const HOVER_MODES: [(&str, &str); 3] = [("always", "始终显示（400ms 延迟）"), ("cmd", "仅按住 ⌘ 时显示"), ("off", "关闭")];

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
}

impl EventEmitter<SettingsEvent> for SettingsView {}

/// 系统对话框（NSAlert）提示一句话
fn message(window: &mut Window, cx: &mut App, level: PromptLevel, title: &str, detail: &str) {
    let _ = window.prompt(level, title, Some(detail), &[PromptButton::ok("好")], cx);
}

impl SettingsView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let scroll = gpui::ScrollHandle::new();
        let scrollbar = crate::scrollbar::Scrollbar::handle(&scroll, cx);
        Self { state, focus, scroll, scrollbar }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn apply_theme(state: &Entity<AppState>, id: String, imported: Option<Vec<ThemeSource>>, cx: &mut App) {
        state.update(cx, |s, cx| {
            s.update_prefs(cx, |p| {
                p.theme.id = id;
                if let Some(v) = imported {
                    p.theme.imported = v;
                }
            })
        });
        let t = state.read(cx).prefs.theme.clone();
        set_theme(&t.id, &t.imported, cx);
    }

    fn open_theme_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        super::show_context_menu(ev.position(), window, cx, move |cx| {
            let p = &state.read(cx).prefs.theme;
            let current = p.id.clone();
            let pick = |t: &ThemeSource| {
                let (state, id) = (state.clone(), t.id.clone());
                MenuItem::action(t.name.clone(), move |_, cx| Self::apply_theme(&state, id.clone(), None, cx)).checked(t.id == current)
            };
            let builtin = builtin_themes();
            let mut items = vec![MenuItem::header("深色")];
            items.extend(builtin.iter().filter(|t| !is_light(&t.bg)).map(pick));
            items.push(MenuItem::header("浅色"));
            items.extend(builtin.iter().filter(|t| is_light(&t.bg)).map(pick));
            if !p.imported.is_empty() {
                items.push(MenuItem::header("导入"));
                items.extend(p.imported.iter().map(pick));
            }
            items
        });
    }

    /// 选文件 → 解析 → 同名覆盖存下 → 立即切过去；失败弹系统错误框「导入失败」
    fn import_itermcolors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("导入".into()) });
        let (state, handle) = (self.state.clone(), window.window_handle());
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path): Option<PathBuf> = paths.into_iter().next() else { return };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let result = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}")).and_then(|xml| import_itermcolors(&name, &xml));
            let _ = cx.update_window(handle, |_, window, cx| match result {
                Ok(t) => {
                    let list = upsert_imported(&state.read(cx).prefs.theme.imported, t.clone());
                    Self::apply_theme(&state, t.id, Some(list), cx);
                }
                Err(e) => message(window, cx, PromptLevel::Critical, "导入失败", &e),
            });
        })
        .detach();
    }

    fn delete_current_theme(&mut self, cx: &mut Context<Self>) {
        let p = self.state.read(cx).prefs.theme.clone();
        let list: Vec<ThemeSource> = p.imported.into_iter().filter(|t| t.id != p.id).collect();
        let first = builtin_themes().remove(0).id;
        Self::apply_theme(&self.state, first, Some(list), cx);
    }

    fn open_icon_menu(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        super::show_context_menu(ev.position(), window, cx, move |cx| {
            let cur = state.read(cx).prefs.pane_icons.clone();
            PANE_ICON_SETS
                .iter()
                .map(|(id, name, _)| {
                    let (state, id) = (state.clone(), id.to_string());
                    let checked = id == cur;
                    MenuItem::action(*name, move |_, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.pane_icons = id.clone()))).checked(checked)
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
                    MenuItem::action(*label, move |_, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.hover_mode = id.clone()))).checked(checked)
                })
                .collect()
        });
    }

    fn install_hook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let r = cx.background_executor().spawn(async { makit_core::hook::install_claude_hook() }).await;
            let _ = cx.update_window(handle, |_, window, cx| match r {
                Ok(s) if s == "already_installed" => message(window, cx, PromptLevel::Info, "安装 Hook", "Hook 已安装，无需重复操作。"),
                Ok(_) => message(window, cx, PromptLevel::Info, "安装 Hook", "Hook 安装成功！Claude Code 重启后生效。"),
                Err(e) => message(window, cx, PromptLevel::Critical, "安装失败", &e),
            });
        })
        .detach();
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
        let label = |t: &'static str| div().text_size(px(11.0)).text_color(theme.fg_muted).mb(px(8.0)).font_weight(FontWeight::MEDIUM).child(t);
        let hint = |t: &'static str| div().text_size(px(11.0)).text_color(theme.fg_muted).mt(px(6.0)).child(t);
        let section = || div().px(px(16.0)).py(px(12.0)).border_b_1().border_color(theme.border);
        let toggle = |id: &'static str, on: bool, text: &'static str| {
            div().id(id).flex().items_center().gap(px(8.0)).text_size(px(13.0)).cursor_pointer().child(check_box(&theme, on, false, 16.0)).child(text)
        };
        let set_notify = |f: fn(&mut crate::persist::state::NotifyPrefs)| {
            cx.listener(move |this: &mut Self, _: &ClickEvent, _, cx| this.state.update(cx, |s, cx| s.update_prefs(cx, |p| f(&mut p.notify))))
        };
        let theme_name = Theme::by_id(&prefs.theme.id, &prefs.theme.imported).source.name;
        let swatches = (0..8).map(|i| div().flex_1().min_w_0().h(px(14.0)).rounded(px(2.0)).bg(theme.ansi[i]).border_1().border_color(theme.border));
        let icons = pane_icons_for(&prefs.pane_icons);
        let icon_name = PANE_ICON_SETS.iter().find(|s| s.0 == prefs.pane_icons).unwrap_or(&PANE_ICON_SETS[0]).1;
        let hover_name = HOVER_MODES.iter().find(|m| m.0 == prefs.sidebar.hover_mode).unwrap_or(&HOVER_MODES[0]).1;
        let hooks = cx.try_global::<NotifyHooks>().cloned();
        let permission = hooks.as_ref().and_then(|h| (h.permission)(cx));
        let accent_text = theme.var("--accent-text");
        let danger = theme.danger;

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
                            .child("设置")
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
                            // 外观主题
                            .child(
                                section()
                                    .child(label("外观主题"))
                                    .child(select_box(&theme, "theme-select", theme_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_theme_menu(ev, window, cx))))
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
                                            .child(action_btn(&theme, "import-iterm", "导入 .itermcolors").on_click(cx.listener(|this, _, window, cx| this.import_itermcolors(window, cx))))
                                            .when(prefs.theme.id.starts_with("imported:"), |d| {
                                                d.child(
                                                    div()
                                                        .id("theme-delete")
                                                        .px(px(6.0))
                                                        .py(px(5.0))
                                                        .text_size(px(12.0))
                                                        .text_color(danger)
                                                        .rounded(px(RADIUS))
                                                        .cursor_pointer()
                                                        .hover(move |s| s.bg(mix_alpha(danger, 0.14)))
                                                        .child("删除当前")
                                                        .on_click(cx.listener(|this, _, _, cx| this.delete_current_theme(cx))),
                                                )
                                            }),
                                    )
                                    .child(hint("UI 配色由终端 16 色 + 前景/背景推导，因此任何 iTerm2 色板都能直接用。")),
                            )
                            // pane 落点图标
                            .child(
                                section()
                                    .child(label("pane 落点图标"))
                                    .child(select_box(&theme, "icon-select", icon_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_icon_menu(ev, window, cx))))
                                    .child(if icons.is_empty() {
                                        div().mt(px(6.0)).child(hint("切 pane 时只浮出名字，不带图标。"))
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
                                    .child(hint("⌥⌘方向键 / ⌥⌘数字 切 pane 时，目标 pane 中央浮出「图标 + 名字」。图标按 pane 序号固定 —— 第 N 个 pane 永远是第 N 个图标，也就是 ⌥⌘N 里的 N。")),
                            )
                            // 系统通知
                            .child(
                                section()
                                    .child(label("系统通知"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.0))
                                            .child(toggle("n-system", prefs.notify.system, "启用 macOS 系统通知").on_click(set_notify(|n| n.system = !n.system)))
                                            .child(toggle("n-approval", prefs.notify.approval, "等待审批时提醒（工具调用需确认）").on_click(set_notify(|n| n.approval = !n.approval)))
                                            .child(toggle("n-user", prefs.notify.user, "等待回答时提醒（Claude 等待你输入）").on_click(set_notify(|n| n.user = !n.user)))
                                            // #215 新增的两项（Tauri 版没有）：不给开关的话「已完成」横幅和提示音就关不掉
                                            .child(toggle("n-completed", prefs.notify.completed, "任务完成时提醒（静音横幅，不打断）").on_click(set_notify(|n| n.completed = !n.completed)))
                                            .child(toggle("n-sound", prefs.notify.sound, "需要处理的提醒带提示音").on_click(set_notify(|n| n.sound = !n.sound))),
                                    )
                                    .child(hint("每次 session 进入 waiting 状态触发一次，再次 waiting 才重新提醒。"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .text_size(px(11.0))
                                            .text_color(theme.fg_muted)
                                            .mt(px(6.0))
                                            .child("系统授权：")
                                            .map(|d| match permission {
                                                Some(true) => d.child(div().font_weight(FontWeight::BOLD).child("已允许")),
                                                Some(false) => d
                                                    .child(div().font_weight(FontWeight::BOLD).child("未允许"))
                                                    .child(" —— 横幅会被系统丢弃。到「系统设置 › 通知 › makit」里打开，或")
                                                    .child(
                                                        div()
                                                            .id("req-perm")
                                                            .text_color(accent_text)
                                                            .underline()
                                                            .cursor_pointer()
                                                            .child("请求授权")
                                                            .on_click(cx.listener(|_, _, window, cx| {
                                                                if let Some(h) = cx.try_global::<NotifyHooks>().cloned() {
                                                                    (h.request_permission)(window, cx);
                                                                }
                                                            })),
                                                    ),
                                                None => d.child("读取中…"),
                                            }),
                                    )
                                    .child(action_btn(&theme, "send-test", "发送测试通知").mt(px(8.0)).on_click(cx.listener(|_, _, window, cx| {
                                        match cx.try_global::<NotifyHooks>().cloned() {
                                            Some(h) => (h.send_test)(window, cx),
                                            None => message(window, cx, PromptLevel::Warning, "发送失败", "系统通知还没接入（归通知模块）。"),
                                        }
                                    })))
                                    .child(hint("立刻发一条横幅验证链路，不必等真有 session 进入等待状态。"))
                                    .child(action_btn(&theme, "install-hook", "安装 Claude Code Hook（推送模式）").mt(px(8.0)).on_click(cx.listener(|this, _, window, cx| this.install_hook(window, cx))))
                                    .child(hint("将 makit-hook.sh 注册到 ~/.claude/settings.json，Claude Code 发出通知时实时推送（无需轮询）。")),
                            )
                            // Hover 详情卡片
                            .child(
                                section()
                                    .child(label("Hover 详情卡片"))
                                    .child(select_box(&theme, "hover-select", hover_name).on_click(cx.listener(|this, ev: &ClickEvent, window, cx| this.open_hover_menu(ev, window, cx))))
                                    .child(hint("Hover 时在 session 旁弹出详细信息卡片（ID、路径、分支、话题等），点击可复制。")),
                            )
                            // 快捷键（读唯一的快捷键表）
                            .child(div().px(px(16.0)).py(px(12.0)).child(label("快捷键")).children(shortcut_groups_el))
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
        for (id, _, icons) in PANE_ICON_SETS {
            assert!(icons.len() == 9 || (id == "none" && icons.is_empty()), "{id}");
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
}
