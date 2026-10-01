//! 可重排视图（#231 第 3.2 步）：一块连续的滚动面 = 会话文件画出来的历史 + 真终端的活动区。
//!
//! - 历史：GPUI 的变高列表（底部对齐，贴底时新内容追加视图自己跟着走），宽度一变当帧按新宽度折行，不经过 PTY
//! - 最后一项是真终端，高度 = 活动区行数 × 行高；终端只画活动区那几行（`TerminalView.active_only`）
//! - PTY 仍按整块 pane 的高度开（`layout_h_override`），不然 claude 按几行排菜单、选项被截掉
//! - 认不出活动区（claude 退出回到 shell、全屏模式、清屏）：整块画终端、**不显示历史**（不然上个会话的历史挂在 shell 上面）
//!
//! 设计见私有仓库 #231 TRD §15–§18。

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{canvas, div, list, prelude::*, px, Context, Entity, ListAlignment, ListState, Subscription, Window};
use makit_core::transcript::{Item, TranscriptReader};

use super::feed::poll;
use super::logic::{list_ops, ListOp};
use super::render::{render_item, Toggle};
use crate::terminal::active_region::Show;
use crate::terminal::{TerminalView, INSET_BOTTOM, INSET_LEFT, INSET_RIGHT, INSET_TOP};
use crate::theme::ActiveTheme;

/// 会话文件轮询间隔（3.3 再按 PTY 活跃度自适应）
const POLL_MS: u64 = 300;

pub struct HybridView {
    terminal: Entity<TerminalView>,
    pub session_id: String,
    items: Rc<RefCell<Vec<Item>>>,
    expanded: Rc<RefCell<HashSet<String>>>,
    list: ListState,
    /// 上一帧活动区那一项的行数（变了要让列表重新量最后一项的高度）
    last_rows: usize,
    /// 整块 pane 的高度（每帧由 canvas 量出来，传给终端当 PTY 的布局高度）
    pane_h: Rc<Cell<f32>>,
    _observe: Subscription,
}

impl HybridView {
    pub fn new(terminal: Entity<TerminalView>, session_id: String, cx: &mut Context<Self>) -> Self {
        terminal.update(cx, |t, _| t.active_only = true);
        // 终端每次重画（新输出、活动区变化）都要让这里重新渲染：活动区的行数可能变了
        let observe = cx.observe(&terminal, |_, _, cx| cx.notify());
        let id = session_id.clone();
        cx.spawn(async move |this, cx| {
            let first = cx
                .background_executor()
                .spawn(async move {
                    let (path, tool) = makit_core::sessions::locate_session_file(&id)?;
                    let mut reader = TranscriptReader::new(path, tool);
                    let p = poll(&mut reader).ok()?;
                    Some((Arc::new(Mutex::new(reader)), p))
                })
                .await;
            // 会话文件还没有（claude 刚启动、还没发第一条消息）：先空着，隔一阵再找
            let (reader, p) = match first {
                Some(v) => v,
                None => return,
            };
            if this.update(cx, |h, cx| h.apply(p, true, cx)).is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
                if this.update(cx, |_, _| ()).is_err() {
                    break;
                }
                let r = reader.clone();
                if let Ok(p) = cx.background_executor().spawn(async move { poll(&mut r.lock().unwrap()) }).await {
                    if !(p.changes.appended.is_empty() && p.changes.updated.is_empty() && !p.changes.reset) {
                        let _ = this.update(cx, |h, cx| h.apply(p, false, cx));
                    }
                }
            }
        })
        .detach();
        Self {
            terminal,
            session_id,
            items: Rc::default(),
            expanded: Rc::default(),
            // 列表项 = 历史条目 + 最后一项终端
            list: ListState::new(1, ListAlignment::Bottom, px(600.0)),
            last_rows: 0,
            pane_h: Rc::new(Cell::new(0.0)),
            _observe: observe,
        }
    }

    /// 底下那个真终端（拆掉可重排视图时，调用方把它恢复成整块画）
    pub fn terminal(&self) -> Entity<TerminalView> {
        self.terminal.clone()
    }

    fn apply(&mut self, p: super::feed::Poll, initial: bool, cx: &mut Context<Self>) {
        let prev_len = self.items.borrow().len();
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
            self.list.reset(p.total + 1);
        } else {
            // 旧 → 新：追加的插在终端那一项前面（下标 prev_len），更新的就地重量
            for op in list_ops(&p.changes, prev_len, p.total, false) {
                match op {
                    ListOp::Reset(n) => self.list.reset(n + 1),
                    ListOp::Splice { old, count } => self.list.splice(old, count),
                }
            }
        }
        cx.notify();
    }

    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        {
            let mut e = self.expanded.borrow_mut();
            if !e.remove(id) {
                e.insert(id.to_string());
            }
        }
        if let Some(ix) = self.items.borrow().iter().position(|i| i.id == id) {
            self.list.splice(ix..ix + 1, 1);
        }
        cx.notify();
    }
}

impl Render for HybridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let pane_h = self.pane_h.clone();
        // 量整块 pane 的高度（这一帧量、下一帧用；PTY 行数跟它走）
        let measure = canvas(move |b, _, _| pane_h.set(f32::from(b.size.height)), |_, _, _, _| {}).absolute().size_full();
        let h = self.pane_h.get();
        let (show, line_h) = {
            let t = self.terminal.read(cx);
            (t.active_show, t.line_h_px())
        };
        self.terminal.update(cx, |t, _| t.layout_h_override = (h > 0.0).then_some(h - INSET_TOP - INSET_BOTTOM));

        let region = match show {
            Show::Region(r) => r,
            // 认不出活动区：整块画终端，不显示历史
            Show::Whole => {
                if let Some(path) = std::env::var_os("MAKIT_NATIVE_DUMP_HYBRID") {
                    let _ = std::fs::write(path, format!("items={} whole pane_h={h:.0}\n", self.items.borrow().len()));
                }
                return div().relative().size_full().bg(theme.bg).child(self.terminal.clone()).child(measure).into_any_element();
            }
        };
        let rows = region.rows();
        let n = self.items.borrow().len();
        // 自检导出（MAKIT_NATIVE_DUMP_HYBRID=<文件>）：历史条数、列表项数、活动区行数、PTY 布局高度
        if let Some(path) = std::env::var_os("MAKIT_NATIVE_DUMP_HYBRID") {
            let _ = std::fs::write(path, format!("items={n} list_count={} term_rows={rows} region={}..={} pane_h={h:.0}\n", self.list.item_count(), region.start, region.end));
        }
        if rows != self.last_rows {
            self.last_rows = rows;
            self.list.splice(n..n + 1, 1);
        }
        let term_h = rows as f32 * line_h + INSET_TOP + INSET_BOTTOM;
        let (items, expanded, th, terminal) = (self.items.clone(), self.expanded.clone(), theme.clone(), self.terminal.clone());
        let font = crate::terminal::text_font(window);
        let this = cx.entity().downgrade();
        let toggle: Toggle = Rc::new(move |id, _, cx| {
            let _ = this.update(cx, |h, cx| h.toggle(id, cx));
        });
        let body = list(self.list.clone(), move |ix, _, _| {
            let items = items.borrow();
            if ix >= items.len() {
                // 最后一项：真终端的活动区
                return div().w_full().h(px(term_h)).child(terminal.clone()).into_any_element();
            }
            let item = &items[ix];
            let open = expanded.borrow().contains(&item.id);
            div().pl(px(INSET_LEFT + 6.0)).pr(px(INSET_RIGHT + 6.0)).pt(if ix == 0 { px(8.0) } else { px(0.0) }).child(render_item(item, open, &th, &font, &toggle)).into_any_element()
        })
        .size_full();
        div().relative().size_full().bg(theme.bg).child(body).child(measure).into_any_element()
    }
}
