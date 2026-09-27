//! 「启动目录已不存在」恢复对话框（照 App.tsx:1564-1660, 2353-2422 + App.css `.recover-*`），按 #217 改进：
//! 输入框不预填原目录、「选择目录…」走系统选择框、边输边校验（不是已有目录时按钮置灰并说原因）、
//! 报错说人话。校验逻辑在 `recover_logic`。
//!
//! 入口 `cwd_missing(tab_id, cwd, window, cx)`：终端（A 包）发现启动目录不在、且这个标签不许回退
//! （resume 标签）时调。先静默试 last_cwd；不行才弹框；同一时间只弹一个（其余 pane 只留终端里的提示）。
//!
//! 恢复成功后：更新标签 cwd（持久化的布局里那个死路径也换掉）+ 把这个标签的终端关掉，
//! 工作区下一帧按新 cwd 重新起（= Tauri 版的 retrySpawn）。

use gpui::{
    div, prelude::*, px, App, AsyncApp, Context, Entity, EventEmitter, FocusHandle, FontWeight, PathPromptOptions, SharedString, Window,
};

use super::recover_logic::{check_relink_target, friendly_error, keyed_by_cwd, RelinkCheck};
use super::style::*;
use super::text_input::{TextInput, TextInputEvent};
use super::{host, show_toast, toast};
use crate::actions::overlays as act;
use crate::state::AppState;
use crate::theme::ActiveTheme;

pub enum RecoverEvent {
    Close,
}

pub struct RecoverDialog {
    state: Entity<AppState>,
    tab_id: String,
    cwd: String,
    session_id: Option<String>,
    input: Entity<TextInput>,
    check: RelinkCheck,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<RecoverEvent> for RecoverDialog {}

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
    let Some(h) = host(cx) else { return };
    h.update(cx, |h, cx| h.open_recover(tab_id, cwd, session_id, window, cx));
}

impl RecoverDialog {
    pub fn new(state: Entity<AppState>, tab_id: String, cwd: String, session_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // #217：不预填原目录（它已经不存在了，预填了点下去必报错）
        let input = cx.new(|cx| TextInput::new("/path/to/new/dir", cx));
        cx.subscribe(&input, |this, input, _: &TextInputEvent, cx| {
            this.check = check_relink_target(input.read(cx).text(), None);
            this.error = None;
            cx.notify();
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state, tab_id, cwd, session_id, input, check: RelinkCheck::Empty, busy: false, error: None, focus }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    fn run(&mut self, relink: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        // 会话信息到点击这一刻再查：弹框时列表可能还没扫完
        let meta = self.session_id.as_deref().and_then(|id| self.state.read(cx).session(id).cloned());
        if relink && meta.is_none() {
            self.error = Some("会话信息还没加载完，稍等一下再点".into());
            cx.notify();
            return;
        }
        let target = match (&self.check, relink) {
            (RelinkCheck::Ok(p), true) => p.clone(),
            (_, true) => return,
            (_, false) => String::new(),
        };
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

    fn pick_dir(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("选这个目录".into()) });
        let input = self.input.clone();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            if let Some(p) = paths.into_iter().next() {
                let _ = input.update(cx, |i, cx| i.set_text(p.display().to_string(), cx));
            }
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.busy {
            cx.emit(RecoverEvent::Close);
        }
    }
}

impl Render for RecoverDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let meta = self.session_id.as_deref().and_then(|id| self.state.read(cx).session(id).cloned());
        // codex 的会话按日期存，不按 cwd 索引：它没有钥匙可丢，措辞得跟着分岔
        let keyed = keyed_by_cwd(meta.as_ref().map(|m| m.storage_folder.as_str()));
        let tool_cmd = if meta.as_ref().map(|m| m.tool == "codex").unwrap_or(false) { "codex resume" } else { "resume" };
        let vp = window.viewport_size();
        let busy = self.busy;
        let can_relink = matches!(self.check, RelinkCheck::Ok(_)) && !busy;
        let code = |t: &'static str| div().font_family(MONO).text_size(px(11.0)).text_color(theme.fg).child(t);
        let note = |d: gpui::Div| d.flex().flex_wrap().items_baseline().text_size(px(12.0)).line_height(px(19.0)).text_color(theme.fg_muted);
        let desc = |t: String| div().text_size(px(11.0)).line_height(px(17.6)).text_color(theme.fg_muted).child(t);
        let option = || div().flex().flex_col().items_start().gap(px(6.0)).px(px(12.0)).py(px(10.0)).border_1().border_color(theme.border).rounded(px(RADIUS));
        let title = |t: &'static str| div().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).child(t);

        let hint: Option<SharedString> = match (&self.error, &self.check) {
            (Some(e), _) => Some(e.clone().into()),
            (None, RelinkCheck::Invalid(m)) => Some(m.clone().into()),
            _ => None,
        };

        div()
            .id("recover-backdrop")
            .key_context("Overlay Recover")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &act::Dismiss, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &act::Confirm, window, cx| this.run(true, window, cx)))
            .absolute()
            .size_full()
            .bg(theme.scrim)
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
            .child(
                div()
                    .id("recover-modal")
                    .occlude()
                    .w((vp.width * 0.34).max(px(400.0)).min(px(520.0)))
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
                            .child("启动目录已不存在")
                            .child(
                                div()
                                    .id("recover-close")
                                    .px(px(6.0))
                                    .text_size(px(18.0))
                                    .text_color(theme.fg_muted)
                                    .when(busy, |d| d.opacity(0.5))
                                    .when(!busy, |d| d.cursor_pointer())
                                    .child("×")
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("recover-body")
                            .overflow_y_scroll()
                            .px(px(16.0))
                            .pt(px(14.0))
                            .pb(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(if keyed {
                                note(div())
                                    .child("会话记录一个字节都没丢，丢的只是「在哪个目录启动」这把钥匙 —— ")
                                    .child(code("claude -r"))
                                    .child(" 只认原目录算出来的存储键。选一条路把钥匙对上：")
                            } else {
                                note(div()).child("这条会话不按目录索引，").child(code(tool_cmd)).child(" 在哪儿都能找到它 —— 缺的只是一个能干活的目录。给它一个即可：")
                            })
                            .child(
                                div()
                                    .font_family(MONO)
                                    .text_size(px(11.0))
                                    .line_height(px(16.5))
                                    .text_color(theme.warning)
                                    .bg(theme.bg)
                                    .border_1()
                                    .border_color(theme.border)
                                    .rounded(px(RADIUS))
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .child(self.cwd.clone()),
                            )
                            .child(
                                option()
                                    .child(title("重建原目录"))
                                    .child(desc(if keyed {
                                        "在原路径建一个空目录，钥匙自然对上，不动 ~/.claude。代码没了，但这条会话能接着聊 —— 会话里提到的文件路径都是空的。".into()
                                    } else {
                                        "在原路径建一个空目录，会话在那儿接着跑。代码没了，会话里提到的文件路径都是空的。".into()
                                    }))
                                    .child(btn(&theme, "recover-recreate", "重建并打开", busy).when(!busy, |d| d.on_click(cx.listener(|this, _, window, cx| this.run(false, window, cx))))),
                            )
                            .child(
                                option()
                                    .child(title("指到新位置"))
                                    .child(desc(if keyed {
                                        "代码搬家了就填新目录。会在新目录的存储键下建一个指向原会话记录的软链，不拷贝（拷贝会变成同一个会话的两份分叉）。".into()
                                    } else {
                                        "代码搬家了就填新目录。这个工具不按目录索引，所以只是换个工作目录，不动任何存储。".into()
                                    }))
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .bg(theme.bg)
                                                    .text_color(theme.fg)
                                                    .border_1()
                                                    .border_color(if self.input.read(cx).focus_handle(cx).is_focused(window) { theme.accent } else { theme.border })
                                                    .rounded(px(RADIUS))
                                                    .px(px(8.0))
                                                    .py(px(6.0))
                                                    .font_family(MONO)
                                                    .text_size(px(11.0))
                                                    .line_height(px(15.0))
                                                    .child(self.input.clone()),
                                            )
                                            .child(btn(&theme, "recover-pick", "选择目录…", busy).when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.pick_dir(cx))))),
                                    )
                                    .child(btn(&theme, "recover-relink", "指过去并打开", !can_relink).when(can_relink, |d| d.on_click(cx.listener(|this, _, window, cx| this.run(true, window, cx))))),
                            )
                            .children(hint.map(|h| div().text_size(px(11.0)).line_height(px(16.5)).text_color(theme.danger).child(h))),
                    ),
            )
    }
}

use gpui::Focusable as _;
