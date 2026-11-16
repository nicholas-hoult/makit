//! 会话详情面板（照 App.tsx:1667-1688, 2194-2252 + App.css `.detail-*` / `.msg-*`）。
//! 右侧滑出、占 50vw（最多 720，用户 2026-09-29 反馈原来 70vw/960 太宽），点遮罩 / 「关闭」/ **Esc** 关（#224：Tauri 版没有 Esc）。
//! 内容是 #231 的阅读视图：`makit_core::transcript` 的 `Item` 列表（claude / codex 同一个视图），
//! 用 GPUI 的 `list`（变高虚拟列表）只排版看得见的那几条；面板开着时每 300ms 增量读文件，新内容即时出现。

use crate::ts;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{div, list, prelude::*, px, Context, Entity, EventEmitter, FocusHandle, FontWeight, ListAlignment, ListState, Window};
use makit_core::diff::{uncommitted_diff, DiffStat};
use makit_core::transcript::Item;
use makit_core::SessionMeta;

use super::detail_logic::{diff_summary_line, subtitle};
use super::style::*;
use crate::actions::overlays as act;
use crate::theme::ActiveTheme;
use crate::transcript_view::feed::{poll, Poll};
use crate::transcript_view::logic::{list_ops, ListOp};
use crate::transcript_view::render::{render_item, Toggle};

pub enum DetailEvent {
    Close,
}

/// 旧 → 新用 `Bottom` 对齐：贴在底部时新内容追加，视图自己跟着走；往上翻过就锚在原位不动，不会被推来推去。
/// 新 → 旧用 `Top`：新内容出现在最上面
fn make_list(reversed: bool) -> ListState {
    ListState::new(0, if reversed { ListAlignment::Top } else { ListAlignment::Bottom }, px(600.0))
}

/// 实时跟随的轮询间隔（TRD §13.2：≤ 300ms 出现）
const POLL_MS: u64 = 300;

enum Load {
    Loading,
    Error(String),
    Ready,
}

pub struct DetailView {
    session: SessionMeta,
    load: Load,
    /// 读取顺序的 Item（新→旧显示时按倒序取）
    items: Rc<RefCell<Vec<Item>>>,
    /// 展开的折叠项（Item id）；重排 / 实时追加不丢
    expanded: Rc<RefCell<HashSet<String>>>,
    /// 默认旧 → 新、贴底跟随（像终端）；按钮可切成新 → 旧，不持久化
    reversed: bool,
    list: ListState,
    scrollbar: Entity<crate::scrollbar::Scrollbar>,
    focus: FocusHandle,
    opened: std::time::Instant,
    /// 后台线程读第一遍文件花的毫秒数
    load_ms: f64,
    /// 这个会话 cwd 相对 HEAD 的未提交改动（#310）；`None` 表示还没读完，或者 cwd 不是 git 仓库
    diff: Option<DiffStat>,
}

impl EventEmitter<DetailEvent> for DetailView {}

impl DetailView {
    pub fn new(session: SessionMeta, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let id = session.session_id.clone();
        cx.spawn(async move |this, cx| {
            let first = cx
                .background_executor()
                .spawn(async move {
                    let t0 = std::time::Instant::now();
                    let mut reader = makit_core::sessions::open_transcript(&id).ok_or_else(|| ts!("detail.not_found", id = id))?;
                    let p = poll(&mut reader).map_err(|e| e.to_string())?;
                    Ok::<_, String>((Arc::new(Mutex::new(reader)), p, t0.elapsed().as_secs_f64() * 1000.0))
                })
                .await;
            let reader = match first {
                Ok((reader, p, load_ms)) => {
                    let _ = this.update(cx, |d, cx| {
                        d.load = Load::Ready;
                        d.load_ms = load_ms;
                        d.apply(p, true, cx);
                    });
                    reader
                }
                Err(e) => {
                    let _ = this.update(cx, |d, cx| {
                        d.load = Load::Error(e);
                        cx.notify();
                    });
                    return;
                }
            };
            loop {
                cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
                if this.update(cx, |_, _| ()).is_err() {
                    break; // 面板已关
                }
                let r = reader.clone();
                let next = cx.background_executor().spawn(async move { poll(&mut r.lock().unwrap()) }).await;
                if let Ok(p) = next {
                    if !(p.changes.appended.is_empty() && p.changes.updated.is_empty() && !p.changes.reset) {
                        let _ = this.update(cx, |d, cx| d.apply(p, false, cx));
                    }
                }
            }
        })
        .detach();
        // diff 读取独立起一个后台任务：和对话内容一样面板打开时读一次，但不纳入上面 300ms 的轮询——
        // 一次 git diff 不便宜，diff 也不需要像对话内容那样秒级跟随（#310）
        let cwd = session.cwd.clone();
        cx.spawn(async move |this, cx| {
            let stat = cx.background_executor().spawn(async move { uncommitted_diff(&cwd) }).await;
            let _ = this.update(cx, |d, cx| {
                d.diff = stat;
                cx.notify();
            });
        })
        .detach();
        let list = make_list(false);
        // 列表上面垫着 14px 的 padding（见渲染），轨道要和列表视口对齐
        let scrollbar = cx.new(|_| crate::scrollbar::Scrollbar::new(crate::scrollbar::ScrollSource::List(list.clone())).with_inset_top(14.0));
        Self { session, load: Load::Loading, items: Rc::default(), expanded: Rc::default(), reversed: false, list, scrollbar, focus, opened: std::time::Instant::now(), load_ms: 0.0, diff: None }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 自检用：(已读到的 Item 数, 展开的折叠项数)
    pub fn debug_state(&self) -> (usize, usize) {
        (self.items.borrow().len(), self.expanded.borrow().len())
    }

    /// 自检用：点开 / 收起第一个工具调用（走和点击同一条 `toggle`），返回有没有找到
    pub fn debug_toggle_first_tool(&mut self, cx: &mut Context<Self>) -> bool {
        let id = self.items.borrow().iter().find(|i| matches!(i.kind, makit_core::transcript::ItemKind::ToolCall { .. })).map(|i| i.id.clone());
        match id {
            Some(id) => {
                self.toggle(&id, cx);
                true
            }
            None => false,
        }
    }

    /// 把一次 poll 的结果并进 Item 列表和 `ListState`
    fn apply(&mut self, p: Poll, initial: bool, cx: &mut Context<Self>) {
        let prev_len = self.items.borrow().len();
        if !initial {
            self.scrollbar.update(cx, |s, _| s.content_changed());
        }
        let t_apply = std::time::Instant::now();
        {
            let mut items = self.items.borrow_mut();
            if p.changes.reset {
                items.clear();
            }
            items.extend(p.appended);
            for (i, item) in p.updated {
                if let Some(slot) = items.get_mut(i) {
                    *slot = item;
                }
            }
        }
        if initial {
            self.list.reset(p.total);
        } else {
            for op in list_ops(&p.changes, prev_len, p.total, self.reversed) {
                match op {
                    ListOp::Reset(n) => self.list.reset(n),
                    ListOp::Splice { old, count } => self.list.splice(old, count),
                }
            }
        }
        // 埋点（perf.log）：打开耗时；之后每次并入超过 8ms 的记一笔
        let apply_ms = t_apply.elapsed().as_secs_f64() * 1000.0;
        if initial {
            crate::perf::record(serde_json::json!({
                "kind": "detail-open", "app": "gpui", "ms": (self.opened.elapsed().as_secs_f64() * 1000.0).round(),
                "load_ms": self.load_ms.round(), "apply_ms": (apply_ms * 10.0).round() / 10.0, "items": p.total,
            }));
        } else if apply_ms > 8.0 {
            crate::perf::record(serde_json::json!({"kind": "detail-apply", "app": "gpui", "ms": (apply_ms * 10.0).round() / 10.0, "items": p.total}));
        }
        cx.notify();
    }

    /// 点折叠行：翻转展开状态，只让这一项重新量高度
    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        self.scrollbar.update(cx, |s, _| s.content_changed());
        {
            let mut e = self.expanded.borrow_mut();
            if !e.remove(id) {
                e.insert(id.to_string());
            }
        }
        let items = self.items.borrow();
        if let Some(r) = items.iter().position(|i| i.id == id) {
            let ix = if self.reversed { items.len() - 1 - r } else { r };
            self.list.splice(ix..ix + 1, 1);
        }
        cx.notify();
    }
}

/// 「这个会话改了什么」(#310)：空文件列表显示「无改动」；否则摘要行 + 文件列表，`+N`/`-N` 用既有语义色，
/// 二进制文件不显示行数。文件列表本身是静态的，不依赖悬停之类的交互。
fn render_diff(stat: &DiffStat, theme: &crate::theme::Theme) -> impl IntoElement {
    if stat.files.is_empty() {
        return div().text_size(px(11.0)).text_color(theme.fg_muted).mt(px(6.0)).child(ts!("detail.diff_none"));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .mt(px(6.0))
        .child(div().text_size(px(11.0)).text_color(theme.fg_muted).child(diff_summary_line(stat)))
        .children(stat.files.iter().map(|f| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(11.0))
                .font(mono_font())
                .child(div().flex_1().min_w_0().text_color(theme.fg_muted).child(f.path.clone()))
                .when(f.binary, |row| row.child(div().text_color(theme.fg_subtle).child(ts!("detail.diff_binary"))))
                .when(!f.binary, |row| {
                    row.child(div().text_color(theme.success).child(format!("+{}", f.added))).child(div().text_color(theme.danger).child(format!("-{}", f.removed)))
                })
        }))
        .when(stat.truncated, |d| d.child(div().text_size(px(11.0)).text_color(theme.fg_subtle).child(ts!("detail.diff_truncated", shown = stat.files.len()))))
}

impl Render for DetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let s = &self.session;
        let n = self.items.borrow().len();
        let vp = window.viewport_size();
        let accent_text = theme.var("--accent-text");

        let body: gpui::AnyElement = match &self.load {
            Load::Loading => div().p(px(24.0)).flex().justify_center().text_color(theme.fg_muted).child(ts!("sidebar.empty.loading")).into_any_element(),
            Load::Error(e) => div().px(px(12.0)).py(px(8.0)).text_color(theme.danger).child(e.clone()).into_any_element(),
            Load::Ready if n == 0 => div().p(px(24.0)).flex().justify_center().text_color(theme.fg_muted).child(ts!("detail.no_content")).into_any_element(),
            Load::Ready => {
                let (items, expanded, th, reversed) = (self.items.clone(), self.expanded.clone(), theme.clone(), self.reversed);
                let font = crate::terminal::text_font(window);
                let this = cx.entity().downgrade();
                let toggle: Toggle = Rc::new(move |id, _, cx| {
                    let _ = this.update(cx, |d, cx| d.toggle(id, cx));
                });
                list(self.list.clone(), move |ix, _, _| {
                    let items = items.borrow();
                    // 列表刚被 reset / splice、渲染还没跟上时，下标可能暂时越界：画个空占位
                    let Some(item) = items.get(if reversed { items.len().wrapping_sub(1).wrapping_sub(ix) } else { ix }) else {
                        return div().into_any_element();
                    };
                    let open = expanded.borrow().contains(&item.id);
                    render_item(item, open, &th, &font, &toggle)
                })
                .size_full()
                .into_any_element()
            }
        };

        div()
            .id("detail-overlay")
            .key_context("Overlay Detail")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &act::Dismiss, _, cx| cx.emit(DetailEvent::Close)))
            .absolute()
            .size_full()
            .bg(theme.var("--shadow-strong"))
            .flex()
            .justify_end()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Close)))
            .child(
                div()
                    .id("detail-panel")
                    .occlude()
                    .w((vp.width * 0.5).min(px(720.0)))
                    .h_full()
                    .bg(theme.bg)
                    .flex()
                    .flex_col()
                    .shadow(vec![shadow_xy(-8.0, 0.0, 24.0, theme.shadow)])
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(12.0))
                            .px(px(18.0))
                            .py(px(14.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .bg(theme.bg_soft)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(format!("[{}]", s.short_id))
                                            .when(!s.display_name.is_empty(), |d| d.child(div().font_weight(FontWeight::MEDIUM).text_color(accent_text).child(s.display_name.clone()))),
                                    )
                                    .child(div().text_size(px(11.0)).text_color(theme.fg_muted).mt(px(4.0)).child(subtitle(s, n)))
                                    .child(div().text_size(px(11.0)).text_color(theme.fg_muted).font(mono_font()).mt(px(2.0)).child(s.cwd.clone()))
                                    .when_some(self.diff.as_ref(), |d, stat| d.child(render_diff(stat, &theme))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.0))
                                    .flex_none()
                                    .child(btn(&theme, "detail-order", if self.reversed { ts!("detail.order_desc") } else { ts!("detail.order_asc") }, false).on_click(cx.listener(|this, _, _, cx| {
                                        this.reversed = !this.reversed;
                                        // 对齐方式在建列表时定死，换排序要重建，并把滚动条指过去
                                        this.list = make_list(this.reversed);
                                        this.list.reset(this.items.borrow().len());
                                        let src = crate::scrollbar::ScrollSource::List(this.list.clone());
                                        this.scrollbar.update(cx, |s, _| s.set_source(src));
                                        cx.notify();
                                    })))
                                    .child(btn(&theme, "detail-close", ts!("workspace.menu.close"), false).on_click(cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Close)))),
                            ),
                    )
                    .child(div().relative().flex_1().min_h_0().px(px(18.0)).pt(px(14.0)).child(body).child(self.scrollbar.clone())),
            )
    }
}
