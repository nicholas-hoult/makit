//! 「启动目录已不存在」恢复选择器（#239）。原来是居中的模态对话框（照 App.tsx:1564-1660），太重：
//! 现在是挂在**那块 pane 底部**的一个下拉列表，照 Claude Code 的列表选择——↑↓ 移动、Enter 确认、Esc 暂不处理，
//! 也可以直接点。选项：会话最近待过的目录 / 上级目录 / 主目录 / 重建原目录 / 选择其他目录…（候选见 `recover_logic::choices`）。
//! 每块 pane 各带各的，不再「一次只弹一个」。
//!
//! 入口 `cwd_missing(tab_id, cwd, window, cx)`：终端（A 包）发现启动目录不在、且这个标签不许回退
//! （resume 标签）时调。先静默试 last_cwd；不行才出选择器。
//!
//! 恢复成功后：更新标签 cwd（持久化的布局里那个死路径也换掉）+ 把这个标签的终端关掉，
//! 工作区下一帧按新 cwd 重新起（= Tauri 版的 retrySpawn）。

use gpui::{div, prelude::*, px, App, AsyncApp, Context, Entity, EventEmitter, FocusHandle, FontWeight, PathPromptOptions, Window};

use super::recover_logic::{check_relink_target, choices, friendly_error, step, Choice, RelinkCheck};
use super::style::*;
use super::{host, show_toast, toast};
use crate::actions::overlays as act;
use crate::state::AppState;
use crate::theme::ActiveTheme;

pub enum RecoverEvent {
    Close,
}

pub struct RecoverPicker {
    state: Entity<AppState>,
    tab_id: String,
    cwd: String,
    session_id: Option<String>,
    choices: Vec<Choice>,
    sel: usize,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<RecoverEvent> for RecoverPicker {}

/// 恢复完成：换掉标签 cwd，关掉旧终端让工作区按新 cwd 重起
fn apply_recovered(tab_id: &str, new_cwd: &str, cx: &mut App) {
    let state = AppState::global(cx);
    state.update(cx, |s, cx| {
        s.workspace.update_tab_cwd(tab_id, new_cwd);
        s.workspace_changed(cx);
    });
    if let Some(h) = host(cx) {
        let ws = h.read(cx).workspace.clone();
        let id = tab_id.to_string();
        ws.update(cx, |w, cx| {
            w.shutdown_tabs(&[id], cx);
            cx.notify();
        });
    }
}

/// 终端发现启动目录不在时调（A 包）。见文件头
pub fn cwd_missing(tab_id: &str, cwd: &str, window: &mut Window, cx: &mut App) {
    let state = AppState::global(cx);
    let (session_id, meta) = {
        let s = state.read(cx);
        let sid = s.workspace.locate_tab(tab_id).and_then(|(_, t)| t.session_id.clone());
        let meta = sid.as_deref().and_then(|id| s.session(id).cloned());
        (sid, meta)
    };
    // 会话最近待过的地方还在 → 悄悄用那儿（「指到新位置」恢复过一次之后走的就是这条路）
    if let Some(m) = meta.filter(|m| !m.last_cwd.is_empty() && m.last_cwd != cwd) {
        let (tab, orig, handle) = (tab_id.to_string(), cwd.to_string(), window.window_handle());
        cx.spawn(async move |cx: &mut AsyncApp| {
            let last = m.last_cwd.clone();
            let orig2 = orig.clone();
            let ok = cx
                .background_executor()
                .spawn(async move {
                    makit_core::recovery::dir_exists(m.last_cwd.clone())
                        && makit_core::recovery::recover_session_cwd("relink".into(), m.session_id, orig2, m.last_cwd, m.storage_folder).is_ok()
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                if ok {
                    apply_recovered(&tab, &last, cx);
                } else {
                    open_dialog(tab.clone(), orig.clone(), session_id.clone(), window, cx);
                }
            });
        })
        .detach();
        return;
    }
    open_dialog(tab_id.to_string(), cwd.to_string(), session_id, window, cx);
}

fn open_dialog(tab_id: String, cwd: String, session_id: Option<String>, window: &mut Window, cx: &mut App) {
    // 推迟到这一轮更新结束再做：真实路径上 `cwd_missing` 是在**工作区自己的事件回调里**被调的（终端发 CwdMissing →
    // 工作区订阅），那时 WorkspaceView 正在被更新，下面再读 / 写它就是 double lease，GPUI 直接 panic、整个应用闪退
    window.defer(cx, move |window, cx| open_dialog_now(tab_id, cwd, session_id, window, cx));
}

fn open_dialog_now(tab_id: String, cwd: String, session_id: Option<String>, window: &mut Window, cx: &mut App) {
    let Some(h) = host(cx) else { return };
    // 面板自己先留一行说明（同 Tauri）：选择器被 Esc 掉之后，这块面板不能是一块没有线索的死屏
    let ws = h.read(cx).workspace.clone();
    if let Some(t) = ws.read(cx).terminal_for_tab(&tab_id) {
        t.update(cx, |t, cx| t.notice(&crate::terminal::pty::cwd_missing_notice(&cwd), cx));
    }
    ws.update(cx, |w, cx| w.show_recover(tab_id, cwd, session_id, window, cx));
}

impl RecoverPicker {
    pub fn new(state: Entity<AppState>, tab_id: String, cwd: String, session_id: Option<String>, focus_now: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let meta = session_id.as_deref().and_then(|id| state.read(cx).session(id).cloned());
        let last = meta.as_ref().map(|m| m.last_cwd.clone());
        let home = dirs::home_dir().map(|h| h.display().to_string());
        let choices = choices(&cwd, last.as_deref(), home.as_deref(), &|p| std::path::Path::new(p).is_dir());
        let focus = cx.focus_handle();
        if focus_now {
            window.focus(&focus);
        }
        Self { state, tab_id, cwd, session_id, choices, sel: 0, busy: false, error: None, focus }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 自检用：(选项数, 选中项, 忙)
    pub fn debug_state(&self) -> (usize, usize, bool) {
        (self.choices.len(), self.sel, self.busy)
    }

    fn move_sel(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.busy {
            self.sel = step(self.sel, self.choices.len(), delta);
            cx.notify();
        }
    }

    fn confirm(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.choices.get(ix).cloned() {
            Some(Choice::Relink { path, .. }) => self.apply(true, path, window, cx),
            Some(Choice::Recreate) => self.apply(false, String::new(), window, cx),
            Some(Choice::Pick) => self.pick_dir(window, cx),
            None => {}
        }
    }

    fn apply(&mut self, relink: bool, target: String, window: &mut Window, cx: &mut Context<Self>) {
        // 会话信息到点击这一刻再查：出选择器时列表可能还没扫完
        let meta = self.session_id.as_deref().and_then(|id| self.state.read(cx).session(id).cloned());
        if relink && meta.is_none() {
            self.error = Some("会话信息还没加载完，稍等一下再选".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let mode = if relink { "relink" } else { "recreate" };
        let (sid, orig, storage) = (self.session_id.clone().unwrap_or_default(), self.cwd.clone(), meta.map(|m| m.storage_folder).unwrap_or_default());
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { makit_core::recovery::recover_session_cwd(mode.into(), sid, orig, target, storage) })
                .await;
            let _ = cx.update_window(handle, |_, _, cx| {
                let _ = this.update(cx, |d, cx| {
                    d.busy = false;
                    match r {
                        Ok(r) => {
                            cx.emit(RecoverEvent::Close);
                            show_toast(r.detail, toast::RECOVERED, cx);
                            apply_recovered(&d.tab_id, &r.cwd, cx);
                        }
                        Err(e) => d.error = Some(friendly_error(&e)),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn pick_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("选这个目录".into()) });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(p) = paths.into_iter().next() else { return };
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |d, cx| match check_relink_target(&p.display().to_string(), None) {
                    RelinkCheck::Ok(path) => d.apply(true, path, window, cx),
                    RelinkCheck::Invalid(m) => {
                        d.error = Some(m);
                        cx.notify();
                    }
                    RelinkCheck::Empty => {}
                });
            });
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.busy {
            cx.emit(RecoverEvent::Close);
        }
    }
}

impl Render for RecoverPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let accent = theme.var("--accent-text");
        let busy = self.busy;
        let rows: Vec<gpui::AnyElement> = self
            .choices
            .iter()
            .enumerate()
            .map(|(ix, c)| {
                let selected = ix == self.sel;
                let (main, note): (String, String) = match c {
                    Choice::Relink { path, why } => (path.clone(), (*why).to_string()),
                    Choice::Recreate => ("重建原目录".into(), "在原路径建空目录；代码没了，但会话能接着聊".into()),
                    Choice::Pick => ("选择其他目录…".into(), "打开系统目录选择框".into()),
                };
                div()
                    .id(gpui::SharedString::from(format!("recover-choice-{ix}")))
                    .flex()
                    .items_baseline()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(RADIUS))
                    .when(selected, |d| d.bg(theme.bg_active))
                    .when(!busy, |d| d.cursor_pointer().on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.sel != ix {
                            this.sel = ix;
                            cx.notify();
                        }
                    })))
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm(ix, window, cx)))
                    .child(div().flex_none().w(px(12.0)).text_color(accent).child(if selected { "❯" } else { "" }))
                    .child(div().min_w_0().font(mono_font()).text_size(px(12.0)).text_color(theme.fg).child(main))
                    .child(div().flex_none().text_size(px(11.0)).text_color(theme.fg_muted).child(note))
                    .into_any_element()
            })
            .collect();

        div()
            .id("recover-picker")
            .key_context("Overlay Recover")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Dismiss, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &act::Confirm, window, cx| {
                let ix = this.sel;
                this.confirm(ix, window, cx)
            }))
            .on_action(cx.listener(|this, _: &act::SelectNext, _, cx| this.move_sel(1, cx)))
            .on_action(cx.listener(|this, _: &act::SelectPrev, _, cx| this.move_sel(-1, cx)))
            .occlude()
            .absolute()
            .left(px(12.0))
            .right(px(12.0))
            .bottom(px(12.0))
            .max_w(px(640.0))
            .bg(theme.bg_soft)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(px(RADIUS_LG))
            .shadow(vec![shadow(8.0, 24.0, theme.var("--shadow-strong"))])
            .px(px(6.0))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .px(px(10.0))
                    .pb(px(4.0))
                    .text_size(px(12.0))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(theme.warning).child("启动目录已不存在"))
                    .child(div().font(mono_font()).text_size(px(11.0)).text_color(theme.fg_muted).child(self.cwd.clone()))
                    .child(div().text_size(px(11.0)).text_color(theme.fg_subtle).child("会话记录没丢。选一个目录继续  ·  ↑↓ 选择  Enter 确认  Esc 暂不处理")),
            )
            .children(rows)
            .children(if busy { Some(div().px(px(10.0)).pt(px(4.0)).text_size(px(11.0)).text_color(theme.fg_muted).child("处理中…")) } else { None })
            .children(self.error.clone().map(|e| div().px(px(10.0)).pt(px(4.0)).text_size(px(11.0)).text_color(theme.danger).child(e)))
    }
}
