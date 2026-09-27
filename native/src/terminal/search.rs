//! ⌘F 终端内搜索的引擎部分（搜索条 UI 归 D 浮层包，调这里的 `search_*` 并订阅 `SearchResults`）。
//!
//! 口径照 xterm search addon（TS 版就是用它）：
//! - 不区分大小写、按字面匹配（不是正则），整个缓冲区（回滚 + 屏幕），折行的也算一处；
//! - 输入时实时找：搜索词变了从「当前命中的开头」（含）往后找，同一个词再找从「当前命中的结尾」往后找；
//!   都没有就从头绕回；上一个同理倒着找。当前命中顺手选中（⌘C 能复制），不在视口里就滚到视口中间；
//! - 全部命中画黄底、当前命中画橙底，两级都描 1px 边（颜色由主题推导，必须不透明，见 theme `--search-match-*`）；
//! - 计数：超过高亮上限（1000）时 index 报 -1（搜索条只显示「N 个结果」），和 addon 一致。
//! 输出还在刷时，结果最多每 250ms 重算一次。

use std::time::Duration;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Point as AlacPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;
use gpui::Context;

use super::zoom::HIGHLIGHT_LIMIT;
use super::{Listener, SearchResults, TerminalView};

/// 最多数多少个命中（防一个字母把 2000 行回滚全标一遍时卡住）
const MAX_MATCHES: usize = 20_000;

#[derive(Default)]
pub(crate) struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    refresh_scheduled: bool,
}

impl SearchState {
    pub fn active(&self) -> bool {
        !self.query.is_empty()
    }
}

fn find_all(term: &Term<Listener>, query: &str) -> Vec<Match> {
    // (?i) 强制不区分大小写（alacritty 默认是「有大写才区分」）；regex::escape 保证按字面匹配
    let Ok(mut re) = RegexSearch::new(&format!("(?i){}", regex::escape(query))) else { return Vec::new() };
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    RegexIter::new(start, end, Direction::Right, term, &mut re).take(MAX_MATCHES).collect()
}

impl TerminalView {
    /// 搜索词变了（输入时实时调）。空串 = 清除高亮
    pub fn search_set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if query == self.search.query {
            return;
        }
        let anchor = self.search_anchor();
        self.search.query = query.to_string();
        if query.is_empty() {
            self.search_clear(cx);
            return;
        }
        self.search.matches = find_all(&self.term.lock(), query);
        // 新词：从当前命中（或选区）的开头往后找，含它自己
        let next = match anchor {
            Some(a) => self.search.matches.iter().position(|m| *m.start() >= a.0).or(if self.search.matches.is_empty() { None } else { Some(0) }),
            None => (!self.search.matches.is_empty()).then_some(0),
        };
        self.search_select(next, cx);
    }

    /// Enter / ↓：下一个；⇧Enter / ↑：上一个（绕回）
    pub fn search_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.search.matches.is_empty() {
            self.emit_results(cx);
            return;
        }
        let n = self.search.matches.len();
        let next = match (self.search.current, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => match self.search_anchor() {
                Some((_, end)) => self.search.matches.iter().position(|m| *m.start() > end).unwrap_or(0),
                None => 0,
            },
            (None, false) => match self.search_anchor() {
                Some((start, _)) => self.search.matches.iter().rposition(|m| *m.start() < start).unwrap_or(n - 1),
                None => n - 1,
            },
        };
        self.search_select(Some(next), cx);
    }

    /// 关闭搜索条：清掉高亮（选区留着，同 xterm clearDecorations）
    pub fn search_clear(&mut self, cx: &mut Context<Self>) {
        self.search.query.clear();
        self.search.matches.clear();
        self.search.current = None;
        self.emit_results(cx);
        cx.notify();
    }

    /// 当前结果（给搜索条画「3/17」：`zoom::format_search_count(query, progress)`）
    pub fn search_progress(&self) -> Option<(i64, usize)> {
        if !self.search.active() {
            return None;
        }
        let count = self.search.matches.len();
        let index = match self.search.current {
            Some(i) if count <= HIGHLIGHT_LIMIT => i as i64,
            _ => -1,
        };
        Some((index, count))
    }

    pub fn search_query(&self) -> &str {
        &self.search.query
    }

    /// 起点：当前命中，否则当前选区（xterm 用的是选区位置）
    fn search_anchor(&self) -> Option<(AlacPoint, AlacPoint)> {
        if let Some(m) = self.search.current.and_then(|i| self.search.matches.get(i)) {
            return Some((*m.start(), *m.end()));
        }
        let term = self.term.lock();
        term.selection.as_ref().and_then(|s| s.to_range(&term)).map(|r| (r.start, r.end))
    }

    fn search_select(&mut self, idx: Option<usize>, cx: &mut Context<Self>) {
        self.search.current = idx;
        if let Some(m) = idx.and_then(|i| self.search.matches.get(i)).cloned() {
            let mut term = self.term.lock();
            let mut sel = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
            sel.update(*m.end(), Side::Right);
            term.selection = Some(sel);
            // 不在视口里就滚到视口中间
            let rows = term.screen_lines() as i32;
            let offset = term.grid().display_offset() as i32;
            let (top, bottom) = (-offset, -offset + rows - 1);
            let line = m.start().line.0;
            if line < top || line > bottom {
                let history = term.grid().history_size() as i32;
                let target = (rows / 2 - line).clamp(0, history);
                term.scroll_display(Scroll::Delta(target - offset));
            }
        }
        self.emit_results(cx);
        cx.notify();
    }

    fn emit_results(&mut self, cx: &mut Context<Self>) {
        let (index, count) = self.search_progress().unwrap_or((-1, 0));
        cx.emit(SearchResults { index, count });
    }

    /// 有新输出时：节流重算（命中位置会被新输出推走），尽量保住「当前是哪一个」
    pub(super) fn search_schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.search.refresh_scheduled {
            return;
        }
        self.search.refresh_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(super::SEARCH_REFRESH_MS)).await;
            let _ = this.update(cx, |v, cx| {
                v.search.refresh_scheduled = false;
                if !v.search.active() {
                    return;
                }
                let cur = v.search.current.and_then(|i| v.search.matches.get(i)).map(|m| *m.start());
                let q = v.search.query.clone();
                v.search.matches = find_all(&v.term.lock(), &q);
                v.search.current = cur.and_then(|c| v.search.matches.iter().position(|m| *m.start() >= c));
                v.emit_results(cx);
                cx.notify();
            });
        })
        .detach();
    }
}
