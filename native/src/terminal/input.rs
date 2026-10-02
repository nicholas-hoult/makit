//! 键盘、输入法、鼠标、滚轮（TerminalView 的方法，从 element 的监听里调进来）。
//!
//! 对齐清单不变量：
//! - 9 IME：组字（marked text）期间不向 PTY 发任何东西（方向键、回车、退格都交给输入法）；上屏时只发一次最终文本。
//! - 11 全局快捷键只认 ⌘：⌃ 组合键原样送进 PTY（`keys::key_to_bytes`）；⌘ 组合键不写 PTY，交给快捷键表。
//! - 12 / 13 滚动条显隐与拖动、触控板 对标终端 算法 + 离散滚轮每格 3 行；只在普通缓冲区、程序没开鼠标上报时滚回看，
//!   否则把滚轮交给程序（鼠标上报 / 方向键）。
//! - 14 / 15 / 16 链接：整条逻辑行识别，⌘ 才画下划线、⌘+点击才打开；双击选词（链接整选 → 中文分词 → 对标终端 分隔符）。

use std::ops::Range;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as AlacPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use gpui::{
    point, size, Bounds, Context, EntityInputHandler, KeyDownEvent, Modifiers, MouseButton, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, UTF16Selection, Window,
};

use super::keys::{key_to_bytes, mouse_report, Mods, MouseBtn};
use super::links::{self, cjk_word_range, link_selection_at, osc8_target, row_links, Cell, LinkKind, Osc8Target, Row};
use super::scrollbar::{self, SCROLLBAR_W};
use super::{segment, wheel, HoverLink, LinkTarget, Listener, TerminalView};

/// 链接识别沿折行上下各最多摸多少行
const MAX_WRAP_ROWS: i32 = 50;

/// 只认 ⌘、不认 ⌃（keys.ts `isCmd`：metaKey && !ctrlKey）
pub(crate) fn is_cmd(m: &Modifiers) -> bool {
    m.platform && !m.control
}

fn mods_of(m: &Modifiers) -> Mods {
    Mods { ctrl: m.control, alt: m.alt, shift: m.shift, cmd: m.platform }
}

/// 滚动条几何（相对终端元素左上角）
pub(crate) struct ScrollbarGeom {
    pub x: f32,
    pub track_h: f32,
    pub slider_top: f32,
    pub slider_h: f32,
    pub history: usize,
}

impl TerminalView {
    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    // ───────────────────────── 键盘 ─────────────────────────

    pub(super) fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        // 组字中：什么都不发，交给输入法（不变量 9）
        if self.marked_text.is_some() {
            return;
        }
        let m = &ev.keystroke.modifiers;
        let mode = self.mode();
        if let Some(bytes) = key_to_bytes(&ev.keystroke.key, mods_of(m), mode.contains(TermMode::APP_CURSOR)) {
            self.write_user_cx(bytes, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }

    // ───────────────────────── 鼠标 ─────────────────────────

    pub(crate) fn cell_at(&self, local: Point<Pixels>) -> (usize, usize) {
        super::grid::point_to_cell(
            f32::from(local.x),
            // 活动区模式：画面往上平移了 row_shift 行，点到的是网格里更靠下的行
            f32::from(local.y) + f32::from(self.line_h) * self.row_shift() as f32,
            f32::from(self.cell_w),
            f32::from(self.line_h),
            self.size.cols,
            self.size.rows,
        )
    }

    fn side_at(&self, local: Point<Pixels>) -> Side {
        let w = f32::from(self.cell_w);
        if w <= 0.0 {
            return Side::Left;
        }
        if (f32::from(local.x).max(0.0) % w) < w / 2.0 { Side::Left } else { Side::Right }
    }

    /// 视口里的 (列, 行) → alacritty 网格坐标（带上回看偏移）
    fn grid_point(&self, col: usize, row: usize) -> AlacPoint {
        let offset = self.term.lock().grid().display_offset() as i32;
        AlacPoint::new(Line(row as i32 - offset), Column(col))
    }

    /// 程序开了鼠标上报，且没按 Shift（按住 Shift 强制本地选择，同 xterm / iTerm2）
    pub(super) fn mouse_reporting(&self, m: &Modifiers) -> bool {
        self.mode().intersects(TermMode::MOUSE_MODE) && !m.shift
    }

    fn report(&mut self, btn: MouseBtn, pressed: bool, motion: bool, col: usize, row: usize, m: &Modifiers) {
        let sgr = self.mode().contains(TermMode::SGR_MOUSE);
        if let Some(b) = mouse_report(btn, pressed, motion, col, row, mods_of(m), sgr) {
            self.write_pty(b);
        }
    }

    /// 有可滚内容（普通缓冲区、有回滚）时滚动条存在：可命中、可拖（显隐只管透明度）
    pub(crate) fn scrollbar_geom(&self, width: f32, height: f32) -> Option<ScrollbarGeom> {
        let term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let history = term.grid().history_size();
        if history == 0 || height <= 0.0 {
            return None;
        }
        let rows = term.screen_lines();
        let top_line = history - term.grid().display_offset();
        let (size, top) = scrollbar::xterm_slider(height, rows, history + rows, top_line);
        let (slider_h, slider_top) = scrollbar::cap_slider(height, size, top, scrollbar::MAX_SLIDER_RATIO);
        Some(ScrollbarGeom { x: width - SCROLLBAR_W, track_h: height, slider_top, slider_h, history })
    }

    pub(crate) fn mouse_down(
        &mut self,
        btn: MouseButton,
        local: Point<Pixels>,
        width: f32,
        height: f32,
        click_count: usize,
        m: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(local.x), f32::from(local.y));
        // 滚动条：抓滑块拖（#228），点轨道空白处翻页
        if btn == MouseButton::Left {
            if let Some(g) = self.scrollbar_geom(width, height) {
                if x >= g.x {
                    if y >= g.slider_top && y < g.slider_top + g.slider_h {
                        self.slider_drag = Some(y - g.slider_top);
                    } else {
                        let up = y < g.slider_top;
                        self.term.lock().scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
                    }
                    self.scroll_activity.ping(self.now_ms());
                    cx.notify();
                    return;
                }
            }
        }
        // ⌘+点击打开链接
        if btn == MouseButton::Left && is_cmd(m) {
            self.update_hover(local, m);
            if let Some(h) = self.hover.clone() {
                self.open_link(&h.target, cx);
                return;
            }
        }
        let (col, row) = self.cell_at(local);
        // 程序接管了鼠标（claude 全屏界面、vim……）：点击、双击都交给程序
        if self.mouse_reporting(m) {
            let b = match btn {
                MouseButton::Left => MouseBtn::Left,
                MouseButton::Middle => MouseBtn::Middle,
                MouseButton::Right => MouseBtn::Right,
                _ => return,
            };
            self.mouse_btn_down = Some(b);
            self.last_report_cell = Some((col, row));
            self.report(b, true, false, col, row, m);
            cx.notify();
            return;
        }
        if btn != MouseButton::Left {
            return;
        }
        let p = self.grid_point(col, row);
        if click_count == 2 {
            self.select_word(col, row, p, local);
        } else {
            let ty = if click_count >= 3 { SelectionType::Lines } else { SelectionType::Simple };
            self.term.lock().selection = Some(Selection::new(ty, p, self.side_at(local)));
        }
        self.selecting = true;
        cx.notify();
    }

    pub(crate) fn mouse_move(
        &mut self,
        local: Point<Pixels>,
        width: f32,
        height: f32,
        pressed: Option<MouseButton>,
        m: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(local.x), f32::from(local.y));
        self.last_mouse = Some(local);
        let mut dirty = false;
        // 右边缘 14px：滚动条显形、可直接抓（#206）
        let in_gutter = x >= 0.0 && y >= 0.0 && y <= height && scrollbar::is_in_scrollbar_gutter(x, width);
        if in_gutter != self.gutter_hover {
            self.gutter_hover = in_gutter;
            dirty = true;
        }
        let slider_hover = self
            .scrollbar_geom(width, height)
            .map(|g| in_gutter && y >= g.slider_top && y < g.slider_top + g.slider_h)
            .unwrap_or(false);
        if slider_hover != self.slider_hover {
            self.slider_hover = slider_hover;
            dirty = true;
        }
        // 拖滑块
        if let (Some(grab), Some(MouseButton::Left)) = (self.slider_drag, pressed) {
            if let Some(g) = self.scrollbar_geom(width, height) {
                let line = scrollbar::drag_to_line(0.0, g.track_h, g.slider_h, grab, y, g.history);
                let mut term = self.term.lock();
                let target = (g.history - line.min(g.history)) as i32;
                let cur = term.grid().display_offset() as i32;
                if target != cur {
                    term.scroll_display(Scroll::Delta(target - cur));
                }
            }
            self.scroll_activity.ping(self.now_ms());
            cx.notify();
            return;
        }
        // ⌘ 悬停链接
        let before = self.hover.clone();
        if is_cmd(m) {
            self.update_hover(local, m);
        } else {
            self.hover = None;
        }
        if self.hover != before {
            dirty = true;
        }
        // 鼠标上报：拖动（1002）/ 任意移动（1003）
        let (col, row) = self.cell_at(local);
        let mode = self.mode();
        if self.mouse_reporting(m) && Some((col, row)) != self.last_report_cell {
            let report_drag = mode.contains(TermMode::MOUSE_DRAG) && self.mouse_btn_down.is_some();
            let report_motion = mode.contains(TermMode::MOUSE_MOTION);
            if report_drag || report_motion {
                self.last_report_cell = Some((col, row));
                match self.mouse_btn_down {
                    Some(b) => self.report(b, true, true, col, row, m),
                    // 没按键的移动：按键码 3 + 32
                    None => {
                        let sgr = mode.contains(TermMode::SGR_MOUSE);
                        let code = 35 + 4 * m.shift as u32 + 8 * m.alt as u32 + 16 * m.control as u32;
                        let bytes = if sgr {
                            Some(format!("\x1b[<{code};{};{}M", col + 1, row + 1).into_bytes())
                        } else if col <= 222 && row <= 222 {
                            Some(vec![0x1b, b'[', b'M', 32 + code as u8, 33 + col as u8, 33 + row as u8])
                        } else {
                            None
                        };
                        if let Some(b) = bytes {
                            self.write_pty(b);
                        }
                    }
                }
            }
        }
        // 选区
        if self.selecting && pressed == Some(MouseButton::Left) {
            let p = self.grid_point(col, row);
            let side = self.side_at(local);
            if let Some(sel) = self.term.lock().selection.as_mut() {
                sel.update(p, side);
            }
            dirty = true;
        }
        if dirty {
            cx.notify();
        }
    }

    pub(crate) fn mouse_up(&mut self, btn: MouseButton, local: Point<Pixels>, m: &Modifiers, cx: &mut Context<Self>) {
        if btn == MouseButton::Left && self.slider_drag.take().is_some() {
            cx.notify();
            return;
        }
        if let Some(b) = self.mouse_btn_down.take() {
            let (col, row) = self.cell_at(local);
            self.report(b, false, false, col, row, m);
            self.last_report_cell = None;
            return;
        }
        if btn == MouseButton::Left && self.selecting {
            self.selecting = false;
            let mut term = self.term.lock();
            if term.selection.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
                term.selection = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn modifiers_changed(&mut self, m: &Modifiers, cx: &mut Context<Self>) {
        let before = self.hover.clone();
        match (is_cmd(m), self.last_mouse) {
            (true, Some(local)) => self.update_hover(local, m),
            _ => self.hover = None,
        }
        if self.hover != before {
            cx.notify();
        }
    }

    // ───────────────────────── 滚轮 ─────────────────────────

    pub(crate) fn scroll_wheel(&mut self, ev: &ScrollWheelEvent, local: Point<Pixels>, cx: &mut Context<Self>) {
        let now = self.now_ms();
        self.scroll_activity.ping(now);
        let (lines, dy) = match ev.delta {
            ScrollDelta::Pixels(p) => {
                self.discrete_pending = 0.0;
                let (n, rest) = wheel::precise_pixels_to_rows(self.precise_pending, f32::from(p.y), f32::from(self.line_h), 1.0);
                self.precise_pending = rest;
                (n, f32::from(p.y))
            }
            ScrollDelta::Lines(l) => {
                // 离散滚轮：清掉触控板的余数（同 TS 版）
                self.precise_pending = 0.0;
                let (n, rest) = wheel::discrete_lines_to_rows(self.discrete_pending, l.y);
                self.discrete_pending = rest;
                (n, l.y)
            }
        };
        if dy == 0.0 {
            self.precise_pending = 0.0;
        }
        if lines == 0 {
            cx.notify();
            return;
        }
        let mode = self.mode();
        if self.mouse_reporting(&ev.modifiers) {
            // 程序自己要鼠标事件：滚轮上报给它
            let (col, row) = self.cell_at(local);
            let btn = if lines > 0 { MouseBtn::WheelUp } else { MouseBtn::WheelDown };
            for _ in 0..lines.unsigned_abs().min(100) {
                self.report(btn, true, false, col, row, &ev.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) {
            // 全屏程序（less、vim……）：滚轮翻成上下方向键（xterm 同样的行为）
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                let key = if lines > 0 { "up" } else { "down" };
                let bytes = key_to_bytes(key, Mods::default(), mode.contains(TermMode::APP_CURSOR)).unwrap_or_default();
                let mut all = Vec::new();
                for _ in 0..lines.unsigned_abs().min(100) {
                    all.extend_from_slice(&bytes);
                }
                self.write_pty(all);
            }
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }

    // ───────────────────────── 链接 ─────────────────────────

    /// 从这一行往上、往下沿折行把整条逻辑行摸出来（各最多 50 行）
    fn logical_rows(term: &Term<Listener>, line: Line) -> Vec<Row> {
        let top = term.topmost_line();
        let bottom = term.bottommost_line();
        let cols = term.columns();
        let wrapped = |l: Line| term.grid()[l][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let mut start = line;
        for _ in 0..MAX_WRAP_ROWS {
            if start <= top || !wrapped(start - 1) {
                break;
            }
            start = start - 1;
        }
        let mut end = line;
        for _ in 0..MAX_WRAP_ROWS {
            if end >= bottom || !wrapped(end) {
                break;
            }
            end = end + 1;
        }
        let mut rows = Vec::new();
        let mut l = start;
        while l <= end {
            let cells = (0..cols)
                .map(|c| {
                    let cell = &term.grid()[l][Column(c)];
                    if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                        return Cell::new("", 0);
                    }
                    let mut s = cell.c.to_string();
                    if let Some(zw) = cell.zerowidth() {
                        s.extend(zw.iter());
                    }
                    Cell::new(s, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 })
                })
                .collect();
            rows.push(Row { y: l.0, cells });
            l = l + 1;
        }
        rows
    }

    /// ⌘ 悬停：OSC 8 优先，其次网址 / 路径（候选路径要查磁盘，缓存 5 秒）
    fn update_hover(&mut self, local: Point<Pixels>, _m: &Modifiers) {
        let (col, row) = self.cell_at(local);
        let cwd = self.cwd();
        let now = self.now_ms();
        let term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let line = Line(row as i32 - offset);
        let rows_n = term.screen_lines();
        let to_view = |l: i32| -> Option<usize> {
            let r = l + offset;
            (r >= 0 && (r as usize) < rows_n).then_some(r as usize)
        };
        // OSC 8：同一个超链接 id 的连续格子
        if let Some(h) = term.grid()[line][Column(col)].hyperlink() {
            let target = match osc8_target(h.uri()) {
                Some(Osc8Target::Url(u)) => Some(LinkTarget::Url(u)),
                Some(Osc8Target::File(p)) => Some(LinkTarget::Path(p)),
                None => None,
            };
            drop(term);
            self.hover = target.map(|target| HoverLink { segments: self.osc8_segments(row, &h), target });
            return;
        }
        let rows = Self::logical_rows(&term, line);
        drop(term);
        let mut offset_cells = 0;
        let mut hit = None;
        for r in &rows {
            if r.y == line.0 {
                let mut h = offset_cells + col;
                if col > 0 && r.cells.get(col).map(|c| c.width == 0).unwrap_or(false) {
                    h -= 1;
                }
                hit = Some(h);
                break;
            }
            offset_cells += r.cells.len();
        }
        let Some(hit) = hit else {
            self.hover = None;
            return;
        };
        let found = row_links(&rows).into_iter().find(|l| hit >= l.start_cell && hit < l.start_cell + l.cells);
        let Some(l) = found else {
            self.hover = None;
            return;
        };
        let target = match l.kind {
            LinkKind::Url => LinkTarget::Url(l.text.clone()),
            LinkKind::Path => {
                let (path, _, _) = links::parse_path_text(&l.text);
                let abs = links::resolve_path(&path, &cwd);
                if l.verify {
                    let ok = self.exist_cache.existing(std::slice::from_ref(&abs), now, makit_core::paths::paths_exist);
                    if !ok.first().copied().unwrap_or(false) {
                        self.hover = None;
                        return;
                    }
                }
                LinkTarget::Path(abs)
            }
        };
        // 链接覆盖的格子 → 视口里的行段（跨折行）
        let cols = self.size.cols as usize;
        let mut segments = Vec::new();
        let start_y = rows[0].y;
        let (s, e) = (l.start_cell, l.start_cell + l.cells - 1);
        for (i, r) in rows.iter().enumerate() {
            let (rs, re) = (i * cols, i * cols + cols - 1);
            if e < rs || s > re {
                continue;
            }
            if let Some(vr) = to_view(start_y + i as i32) {
                segments.push((vr, s.max(rs) - rs, e.min(re) - rs));
            }
            let _ = r;
        }
        self.hover = Some(HoverLink { segments, target });
    }

    fn osc8_segments(&self, row: usize, h: &alacritty_terminal::term::cell::Hyperlink) -> Vec<(usize, usize, usize)> {
        let term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let cols = term.columns();
        let mut segs = Vec::new();
        for r in 0..term.screen_lines() {
            let line = Line(r as i32 - offset);
            let mut run: Option<(usize, usize)> = None;
            for c in 0..cols {
                let same = term.grid()[line][Column(c)].hyperlink().map(|x| x.id() == h.id()).unwrap_or(false);
                match (&mut run, same) {
                    (Some((_, e)), true) => *e = c,
                    (None, true) => run = Some((c, c)),
                    (Some((s, e)), false) => {
                        segs.push((r, *s, *e));
                        run = None;
                    }
                    (None, false) => {}
                }
            }
            if let Some((s, e)) = run {
                segs.push((r, s, e));
            }
        }
        let _ = row;
        segs
    }

    fn open_link(&mut self, target: &LinkTarget, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Url(u) => cx.open_url(u),
            LinkTarget::Path(p) => {
                // 文件用默认程序打开，目录进 Finder（open_path 会展开 ~，路径不存在就报错）
                let p = p.clone();
                std::thread::spawn(move || {
                    if let Err(e) = makit_core::paths::open_path(p, false) {
                        log::warn!(target: "终端", "打开链接失败：{e}");
                    }
                });
            }
        }
    }

    // ───────────────────────── 双击选词 ─────────────────────────

    /// ① 确定的链接整选（可跨行，路径不带 `:行:列`）→ ② 中日韩文字按系统分词选一个词 → ③ 对标终端 分隔符
    fn select_word(&mut self, col: usize, row: usize, p: AlacPoint, local: Point<Pixels>) {
        let side = self.side_at(local);
        let mut term = self.term.lock();
        let rows = Self::logical_rows(&term, p.line);
        let cols = term.columns();
        let span = |start_col: usize, start_line: i32, len: usize| {
            let start = AlacPoint::new(Line(start_line), Column(start_col));
            let flat_end = start_col + len.max(1) - 1;
            let end = AlacPoint::new(Line(start_line + (flat_end / cols) as i32), Column(flat_end % cols));
            let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
            sel.update(end, Side::Right);
            sel
        };
        if let Some((c, l, len)) = link_selection_at(&rows, col, p.line.0) {
            term.selection = Some(span(c, l, len));
            return;
        }
        if let Some(here) = rows.iter().find(|r| r.y == p.line.0) {
            if let Some((c, len)) = cjk_word_range(&here.cells, col, &segment::segment_words) {
                term.selection = Some(span(c, p.line.0, len));
                return;
            }
        }
        let _ = row;
        term.selection = Some(Selection::new(SelectionType::Semantic, p, side));
    }
}

// ── 输入法：中文上屏走这里，拼音组字（marked text）画在光标处 ──
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_ref().map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        cx.notify();
    }

    /// 上屏：只发这一次最终文本（不变量 9）
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.write_user_cx(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }

    /// 组字中间态：只画，不发
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = if new_text.is_empty() { None } else { Some(new_text.to_string()) };
        cx.notify();
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element_bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        // 候选框贴着光标
        let term = self.term.lock();
        let c = term.grid().cursor.point;
        let offset = term.grid().display_offset() as i32;
        let row = (c.line.0 + offset).max(0) as f32 - self.row_shift() as f32;
        Some(Bounds::new(
            point(element_bounds.origin.x + self.cell_w * c.column.0 as f32, element_bounds.origin.y + self.line_h * row),
            size(self.cell_w, self.line_h),
        ))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

/// 终端右键菜单要不要弹、弹哪几项（同 Tauri App.tsx paneMenuItems 的 terminal 分支）：
/// 只对选中的文字做事；没选中不弹（弹个空框更糟）。选中内容在右键那一刻取走（菜单一开焦点就离开终端，之后可能被清掉）
pub fn terminal_menu(selection: Option<&str>) -> Option<(&'static str, &'static str)> {
    // 空字符串不算选中（xterm 拿不到选区时也是这个值）；纯空白是用户真选的，照样能复制
    selection.filter(|s| !s.is_empty()).map(|_| ("复制", "搜索选中内容"))
}

#[cfg(test)]
mod menu_tests {
    use super::terminal_menu;

    #[test]
    fn right_click_menu_only_with_selection() {
        assert_eq!(terminal_menu(None), None);
        assert_eq!(terminal_menu(Some("")), None, "空选区不弹");
        assert_eq!(terminal_menu(Some("   ")), Some(("复制", "搜索选中内容")), "空白也是用户选的内容，照样可以复制");
        assert_eq!(terminal_menu(Some("error: foo")), Some(("复制", "搜索选中内容")));
    }
}
