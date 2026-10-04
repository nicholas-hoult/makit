//! 终端的绘制元素：prepaint 时先按量出来的区域改尺寸（同帧生效，不会有空白帧，#203），再锁一下 Term、
//! 把可见区域拍成背景块 + 文字段 + 选区 / 搜索高亮 / 链接下划线 / 光标 / 滚动条，paint 时画出来，顺带挂鼠标监听。
//!
//! 视觉数值照 Tauri 版（App.css / TerminalManager.ts / xterm 6.1 默认）：
//! - 字体：对标 对标产品（见 `fonts.rs`）——JetBrains Mono 13px，中日韩按系统语言固定 PingFang SC / TC / Hiragino Sans
//! - 格子：宽 = floor(字宽 × 缩放) / 缩放，高 = ceil((ascent + descent) × 缩放) / 缩放（xterm 的 device 像素取整，
//!   列数才和 Tauri 版同窗口一致）
//! - 选区：主题 `--selection-bg`（未聚焦 `--selection-bg-inactive`），画在文字下面
//! - 光标：块状、fg 色、闪烁 600ms，块里的字用背景色；未聚焦画空心框；程序可以改成竖线 / 下划线（1px）
//! - 滚动条：14px 宽、方角，滑块 fg × 0.2（悬停 0.4、拖动 0.5），长度封顶 1/4 轨道、最小 20，停手 900ms 后 300ms 淡出
//! - 最低对比度 4.5，按每个格子的实际背景（含搜索高亮底）算，dim 减半再 50% 混进背景

use crate::ts;
use gpui::{
    fill, outline, point, px, relative, size, App, BorderStyle, Bounds, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, Font, FontFallbacks, FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior,
    Hsla, InspectorElementId, IntoElement, LayoutId, ModifiersChangedEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};

use std::sync::OnceLock;

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};

use super::contrast::cell_fg;
use super::palette::TermColors;
use super::scrollbar::SCROLLBAR_W;
use super::{fonts, TerminalView, FONT_STACK};

pub(super) struct TerminalElement {
    pub(super) view: Entity<TerminalView>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct TextLine {
    origin: Point<Pixels>,
    line: ShapedLine,
}

pub struct Frame {
    hitbox: Hitbox,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    /// 画在文字上面的：搜索描边
    outlines: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<TextLine>,
    /// 光标：框 + 颜色 + 是否实心；实心块光标里的那个字（背景色重画）
    cursor: Option<(Bounds<Pixels>, Hsla, bool)>,
    cursor_text: Option<TextLine>,
    marked: Option<(Point<Pixels>, ShapedLine)>,
    scrollbar: Option<(Bounds<Pixels>, Hsla)>,
    line_h: Pixels,
    cursor_style: CursorStyle,
    animating: bool,
}

/// 一段同样式、同宽度的连续字符
struct Run {
    col: usize,
    cells: usize,
    wide: bool,
    text: String,
    fg: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl Run {
    fn same_style(&self, fg: u32, bold: bool, italic: bool, underline: bool, strike: bool, wide: bool) -> bool {
        self.fg == fg && self.bold == bold && self.italic == italic && self.underline == underline && self.strike == strike && self.wide == wide
    }
}

fn hsla(v: u32) -> Hsla {
    gpui::rgb(v).into()
}

fn rgb_of(c: alacritty_terminal::vte::ansi::Rgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// 单元格颜色 → 实际颜色：程序用 OSC 改过的颜色优先，否则走主题调色板。返回 (颜色, 是否 dim 系列)
fn resolve(color: AnsiColor, overrides: &Colors, c: &TermColors, bold: bool) -> (u32, bool) {
    match color {
        AnsiColor::Spec(rgb) => (rgb_of(rgb), false),
        AnsiColor::Indexed(i) => {
            // 粗体的 0–7 画成亮色（xterm drawBoldTextInBrightColors 默认开）
            let i = if bold && i < 8 { i + 8 } else { i };
            (overrides[i as usize].map(rgb_of).unwrap_or_else(|| c.indexed(i)), false)
        }
        AnsiColor::Named(n) => {
            let mut idx = n as usize;
            if bold && idx < 8 {
                idx += 8;
            }
            if let Some(o) = overrides[idx] {
                return (rgb_of(o), false);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => (c.fg, false),
                NamedColor::Background => (c.bg, false),
                NamedColor::Cursor => (c.cursor, false),
                NamedColor::DimForeground => (c.fg, true),
                _ if idx < 16 => (c.ansi[idx], false),
                // Dim 系列：DimBlack 起对应 ANSI 0..7
                _ => (c.ansi[(idx - NamedColor::DimBlack as usize).min(7)], true),
            }
        }
    }
}

static PICKED: OnceLock<SharedString> = OnceLock::new();

/// 实际用上的等宽字体（调试导出用）
pub(super) fn picked_family() -> Option<SharedString> {
    PICKED.get().cloned()
}

/// 字体栈里第一个能加载的（结果缓存：进程里只挑一次）
fn font_family(window: &Window) -> SharedString {
    PICKED
        .get_or_init(|| {
            let ts = window.text_system();
            for f in FONT_STACK {
                // resolve_font 找不到时会悄悄换成默认字体栈里的字体：拿回来的 family 对得上才算加载到了
                let id = ts.resolve_font(&gpui::font(*f));
                if ts.get_font_for_id(id).map(|got| got.family.as_ref() == *f).unwrap_or(false) {
                    return SharedString::from(*f);
                }
            }
            SharedString::from("Menlo")
        })
        .clone()
}

static CJK: OnceLock<Vec<String>> = OnceLock::new();

fn cjk_names() -> &'static Vec<String> {
    CJK.get_or_init(|| fonts::cjk_fallbacks(&fonts::system_preferred_languages()).into_iter().map(String::from).collect())
}

/// 界面里别处要用等宽字体（阅读视图）：主字体 + 和终端一样的 CJK 回退。
/// 只写 `font_family("Menlo")` 时中文走系统自己挑的回退，会挑到别的字体、字形对不上（#231）
pub(crate) fn text_font(window: &Window) -> Font {
    let mut f = gpui::font(font_family(window));
    f.fallbacks = Some(FontFallbacks::from_fonts(cjk_names().clone()));
    f
}

/// 当前字号下的终端字体和格子尺寸（对标终端 的设备像素四舍五入，见 `fonts::cell_size`）
pub(super) fn metrics(window: &Window, font_size: f32) -> (Font, Pixels, Pixels) {
    let mut f = gpui::font(font_family(window));
    f.fallbacks = Some(FontFallbacks::from_fonts(cjk_names().clone()));
    let ts = window.text_system();
    let id = ts.resolve_font(&f);
    let fs = px(font_size);
    let scale = window.scale_factor().max(1.0);
    let adv = ts.advance(id, fs, 'W').map(|s| f32::from(s.width)).unwrap_or(font_size * 0.6);
    let asc = f32::from(ts.ascent(id, fs));
    let desc = f32::from(ts.descent(id, fs)).abs();
    // line_gap：GPUI 不公开；JetBrains Mono 的 hhea lineGap 就是 0
    let (cell_w, line_h) = fonts::cell_size(adv, asc, desc, 0.0, scale);
    (f, px(cell_w), px(line_h))
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let font_size = self.view.read(cx).font_size;
        let (base_font, cell_w, line_h) = metrics(window, font_size);
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        // 先改尺寸（同帧生效），再拍内容
        self.view.update(cx, |v, cx| v.layout(w, h, cell_w, line_h, cx));
        self.view.update(cx, |v, _| v.update_active_region());

        let now_ms = self.view.read(cx).now_ms();
        let scroll_geom = self.view.read(cx).scrollbar_geom(w, h);
        let view = self.view.read(cx);
        let focused = view.focus.is_focused(window);
        let marked_text = view.marked_text.clone();
        let colors = view.colors.clone();
        let hover = view.hover.clone();
        let blink_on = view.blink_on;
        let (rows, cols) = (view.size.rows as usize, view.size.cols as usize);
        let search_hits: Vec<_> = view.search.matches.clone();
        let search_current = view.search.current;
        let gutter_hover = view.gutter_hover;
        let slider_hover = view.slider_hover;
        let dragging = view.slider_drag.is_some();
        let mouse_mode_cursor = view.term.lock().mode().intersects(TermMode::MOUSE_MODE);
        let activity = view.scroll_activity;
        let term_arc = view.term.clone();
        // 可重排视图：只画活动区 start..=end，并把它平移到元素顶部（终端视图恒为 Whole，下面一切照旧）
        let region = match view.active_show {
            super::active_region::Show::Region(r) if view.active_only => Some(r),
            _ => None,
        };
        let in_region = |row: usize| region.map_or(true, |r| row >= r.start && row <= r.end);
        let ts = window.text_system().clone();
        let fs = px(font_size);

        let term = term_arc.lock();
        let content = term.renderable_content();
        let overrides = content.colors;
        let offset = content.display_offset as i32;
        let origin = match region {
            Some(r) => point(bounds.origin.x, bounds.origin.y - line_h * r.start as f32),
            None => bounds.origin,
        };
        let cell_rect = |col: usize, row: usize, n: usize| {
            Bounds::new(point(origin.x + cell_w * col as f32, origin.y + line_h * row as f32), size(cell_w * n as f32, line_h))
        };

        // ── 搜索命中 → 每格的背景覆盖（对比度按它算）+ 描边 ──
        let mut bg_override: Vec<Option<u32>> = vec![None; rows * cols];
        let mut outlines: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
        let view_top = -offset;
        let view_bottom = view_top + rows as i32 - 1;
        for (i, m) in search_hits.iter().enumerate() {
            let (s, e) = (*m.start(), *m.end());
            if e.line.0 < view_top || s.line.0 > view_bottom {
                continue;
            }
            let active = Some(i) == search_current;
            let (bg, border) =
                if active { (colors.active_match_bg, colors.active_match_border) } else { (colors.match_bg, colors.match_border) };
            for line in s.line.0.max(view_top)..=e.line.0.min(view_bottom) {
                let row = (line + offset) as usize;
                let from = if line == s.line.0 { s.column.0 } else { 0 };
                let to = if line == e.line.0 { e.column.0 } else { cols - 1 };
                for c in from..=to.min(cols - 1) {
                    bg_override[row * cols + c] = Some(bg);
                }
                outlines.push((cell_rect(from, row, to.min(cols - 1) + 1 - from), hsla(border)));
            }
        }
        // ── ⌘ 悬停的链接：下划线 ──
        let mut link_mask = vec![false; rows * cols];
        if let Some(h) = &hover {
            for &(r, a, b) in &h.segments {
                for c in a..=b.min(cols.saturating_sub(1)) {
                    if r < rows {
                        link_mask[r * cols + c] = true;
                    }
                }
            }
        }

        let mut rects: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
        let mut runs_by_row: Vec<Vec<Run>> = (0..rows).map(|_| Vec::new()).collect();
        // 背景块：同一行相邻同色合并
        let mut bg_run: Option<(usize, usize, usize, u32)> = None; // row, col, n, color
        let flush_bg = |r: &mut Option<(usize, usize, usize, u32)>, rects: &mut Vec<(Bounds<Pixels>, Hsla)>| {
            if let Some((row, col, n, c)) = r.take() {
                rects.push((cell_rect(col, row, n), hsla(c)));
            }
        };

        for cell in content.display_iter {
            let row_i = cell.point.line.0 + offset;
            let col = cell.point.column.0;
            if row_i < 0 || row_i as usize >= rows || col >= cols || !in_region(row_i as usize) {
                continue;
            }
            let row = row_i as usize;
            let flags = cell.flags;
            let bold = flags.contains(Flags::BOLD);
            let (mut fg, mut fg_dim) = resolve(cell.fg, overrides, &colors, bold);
            let (mut bg, _) = resolve(cell.bg, overrides, &colors, false);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                fg_dim = false;
            }
            if let Some(o) = bg_override[row * cols + col] {
                bg = o;
            }
            if bg != colors.bg {
                match &mut bg_run {
                    Some((r, c, n, color)) if *r == row && *c + *n == col && *color == bg => *n += 1,
                    _ => {
                        flush_bg(&mut bg_run, &mut rects);
                        bg_run = Some((row, col, 1, bg));
                    }
                }
            }
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN) {
                continue;
            }
            let wide = flags.contains(Flags::WIDE_CHAR);
            let dim = fg_dim || flags.contains(Flags::DIM);
            let fg_rgb = cell_fg(bg, fg, dim, cell.c);
            let italic = flags.contains(Flags::ITALIC);
            let underline = flags.intersects(Flags::ALL_UNDERLINES) || link_mask[row * cols + col];
            let strike = flags.contains(Flags::STRIKEOUT);
            let runs = &mut runs_by_row[row];
            let width = if wide { 2 } else { 1 };
            let extend = match runs.last() {
                Some(r) => r.col + r.cells == col && r.same_style(fg_rgb, bold, italic, underline, strike, wide),
                None => false,
            };
            if cell.c == ' ' && !extend && !underline {
                continue; // 空格不开新段
            }
            if extend {
                let r = runs.last_mut().unwrap();
                r.text.push(cell.c);
                r.cells += width;
            } else {
                runs.push(Run { col, cells: width, wide, text: cell.c.to_string(), fg: fg_rgb, bold, italic, underline, strike });
            }
            if let Some(zw) = cell.zerowidth() {
                let r = runs.last_mut().unwrap();
                r.text.extend(zw.iter());
            }
        }
        flush_bg(&mut bg_run, &mut rects);

        // 选区（半透明，文字下面）
        if let Some(sel) = content.selection {
            let color = if focused { colors.selection } else { colors.selection_inactive };
            let start_row = sel.start.line.0 + offset;
            let end_row = sel.end.line.0 + offset;
            for row in start_row.max(0)..=end_row.min(rows as i32 - 1) {
                if !in_region(row as usize) {
                    continue;
                }
                let (from, to) = if sel.is_block {
                    (sel.start.column.0.min(sel.end.column.0), sel.start.column.0.max(sel.end.column.0))
                } else {
                    (if row == start_row { sel.start.column.0 } else { 0 }, if row == end_row { sel.end.column.0 } else { cols - 1 })
                };
                if to >= from {
                    rects.push((cell_rect(from, row as usize, to.min(cols - 1) - from + 1), color));
                }
            }
        }

        // 光标
        let cur = content.cursor;
        let cursor_row = cur.point.line.0 + offset;
        let mut cursor = None;
        let mut cursor_text = None;
        if cur.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < rows && cur.point.column.0 < cols && in_region(cursor_row as usize) {
            let row = cursor_row as usize;
            let ccell = &term.grid()[cur.point];
            let wide = ccell.flags.contains(Flags::WIDE_CHAR);
            let mut b = cell_rect(cur.point.column.0, row, if wide { 2 } else { 1 });
            let ccolor = overrides[NamedColor::Cursor as usize].map(rgb_of).unwrap_or(colors.cursor);
            if !focused {
                // 未聚焦：空心框（xterm cursorInactiveStyle 默认 outline）
                cursor = Some((b, hsla(ccolor), false));
            } else if blink_on {
                match cur.shape {
                    CursorShape::Beam => {
                        b.size.width = px(1.0);
                        cursor = Some((b, hsla(ccolor), true));
                    }
                    CursorShape::Underline => {
                        b.origin.y += line_h - px(1.0);
                        b.size.height = px(1.0);
                        cursor = Some((b, hsla(ccolor), true));
                    }
                    CursorShape::HollowBlock => cursor = Some((b, hsla(ccolor), false)),
                    _ => {
                        cursor = Some((b, hsla(ccolor), true));
                        // 块光标里的字用背景色重画（xterm cursorAccent 默认 = 背景）
                        if ccell.c != ' ' && !ccell.flags.contains(Flags::HIDDEN) {
                            let mut text = ccell.c.to_string();
                            if let Some(zw) = ccell.zerowidth() {
                                text.extend(zw.iter());
                            }
                            let run = TextRun {
                                len: text.len(),
                                font: base_font.clone(),
                                color: hsla(colors.bg),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let force = if wide { cell_w * 2.0 } else { cell_w };
                            let shaped = ts.shape_line(SharedString::from(text), fs, &[run], Some(force));
                            cursor_text = Some(TextLine { origin: b.origin, line: shaped });
                        }
                    }
                }
            }
        }
        let cursor_origin = (cursor_row >= 0)
            .then(|| point(origin.x + cell_w * cur.point.column.0 as f32, origin.y + line_h * cursor_row.max(0) as f32));
        drop(term);

        // 滚动条（有可滚内容才画；显隐只管透明度）
        let mut animating = false;
        // 活动区模式不显示回滚区，也就没有滚动条（滚动归外面的复合视图）
        let scroll_geom = if region.is_some() { None } else { scroll_geom };
        let scrollbar = scroll_geom.and_then(|g| {
            animating = activity.animating(now_ms);
            let op = if gutter_hover || dragging { 1.0 } else { activity.opacity(now_ms) };
            if op <= 0.0 {
                return None;
            }
            let a = if dragging { 0.5 } else if slider_hover { 0.4 } else { 0.2 };
            let mut c = hsla(colors.fg);
            c.a = a * op;
            Some((
                Bounds::new(point(origin.x + px(g.x), origin.y + px(g.slider_top)), size(px(SCROLLBAR_W), px(g.slider_h))),
                c,
            ))
        });

        // 文字：逐段排版（shape 有缓存，同样的文字下一帧不再排）
        let mut lines = Vec::new();
        for (row, runs) in runs_by_row.into_iter().enumerate() {
            for mut r in runs {
                if !r.underline {
                    let trimmed = r.text.trim_end_matches(' ').len();
                    r.text.truncate(trimmed);
                }
                if r.text.is_empty() {
                    continue;
                }
                let mut f: Font = base_font.clone();
                if r.bold {
                    f.weight = FontWeight::BOLD;
                }
                if r.italic {
                    f.style = FontStyle::Italic;
                }
                let color = hsla(r.fg);
                let run = TextRun {
                    len: r.text.len(),
                    font: f,
                    color,
                    background_color: None,
                    underline: r.underline.then_some(UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
                    strikethrough: r.strike.then_some(StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
                };
                let force = if r.wide { cell_w * 2.0 } else { cell_w };
                let shaped = ts.shape_line(SharedString::from(r.text), fs, &[run], Some(force));
                lines.push(TextLine { origin: point(origin.x + cell_w * r.col as f32, origin.y + line_h * row as f32), line: shaped });
            }
        }

        let marked = match (marked_text, cursor_origin) {
            (Some(t), Some(o)) => {
                let color = hsla(colors.fg);
                let run = TextRun {
                    len: t.len(),
                    font: base_font.clone(),
                    color,
                    background_color: Some(hsla(colors.bg)),
                    underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
                    strikethrough: None,
                };
                Some((o, ts.shape_line(SharedString::from(t), fs, &[run], None)))
            }
            _ => None,
        };

        let cursor_style = if gutter_hover || dragging {
            CursorStyle::Arrow
        } else if hover.is_some() {
            CursorStyle::PointingHand
        } else if mouse_mode_cursor {
            CursorStyle::Arrow
        } else {
            CursorStyle::IBeam
        };

        // 自检导出（MAKIT_NATIVE_DUMP）：这一帧实际画了网格里的哪几行 —— 验证可重排视图「只画活动区」
        if std::env::var_os("MAKIT_NATIVE_DUMP").is_some() {
            let top = f32::from(origin.y);
            let mut drawn: Vec<usize> = lines.iter().map(|l| ((f32::from(l.origin.y) - top) / f32::from(line_h)).round() as usize).collect();
            drawn.sort_unstable();
            drawn.dedup();
            let y0 = lines.iter().map(|l| f32::from(l.origin.y) - f32::from(bounds.origin.y)).fold(f32::INFINITY, f32::min);
            self.view.update(cx, |v, _| v.dbg_drawn = format!("drawn_rows={drawn:?} first_line_y={:.1}", if y0.is_finite() { y0 } else { -1.0 }));
        }

        Frame { hitbox, rects, outlines, lines, cursor, cursor_text, marked, scrollbar, line_h, cursor_style, animating }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        window.set_cursor_style(frame.cursor_style, &frame.hitbox);
        let line_h = frame.line_h;

        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (b, c) in &frame.rects {
                window.paint_quad(fill(*b, *c));
            }
            for l in &frame.lines {
                let _ = l.line.paint(l.origin, line_h, window, cx);
            }
            for (b, c) in &frame.outlines {
                window.paint_quad(outline(*b, *c, BorderStyle::Solid));
            }
            if let Some((b, c, solid)) = frame.cursor {
                if solid {
                    window.paint_quad(fill(b, c));
                } else {
                    window.paint_quad(outline(b, c, BorderStyle::Solid));
                }
            }
            if let Some(t) = &frame.cursor_text {
                let _ = t.line.paint(t.origin, line_h, window, cx);
            }
            if let Some((o, line)) = &frame.marked {
                let _ = line.paint(*o, line_h, window, cx);
            }
            if let Some((b, c)) = frame.scrollbar {
                window.paint_quad(fill(b, c));
            }
        });
        if frame.animating {
            // 滚动条还在显形 / 淡出：继续要帧
            window.request_animation_frame();
        }

        // 鼠标：选区、链接、滚动条、鼠标上报、滚轮
        let view = self.view.clone();
        let hitbox = frame.hitbox.clone();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let local = ev.position - bounds.origin;
                // 右键：程序接管鼠标时转给它（vim 的右键菜单等）；否则只对已选中的文字弹菜单（同 Tauri，
                // 没选中不弹 —— 空框更糟）。选中内容在右键这一刻取走：菜单一开、焦点离开终端，之后可能被清掉
                if ev.button == gpui::MouseButton::Right {
                    let (reporting, selection) = view.update(cx, |v, _| (v.mouse_reporting(&ev.modifiers), v.selection_text()));
                    if !reporting {
                        if super::input::terminal_menu(selection.as_deref()).is_some() {
                            let text = selection.unwrap();
                            let v2 = view.clone();
                            crate::overlays::show_context_menu(ev.position, window, cx, move |_| {
                                vec![
                                    crate::overlays::MenuItem::action(ts!("common.copy"), {
                                        let t = text.clone();
                                        move |_, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(t.clone()));
                                            crate::overlays::show_toast(ts!("common.copied"), crate::overlays::toast::COPY_OK, cx);
                                        }
                                    }),
                                    crate::overlays::MenuItem::action(ts!("terminal.menu.search_selection"), {
                                        let (t, v2) = (text.clone(), v2.clone());
                                        move |window, cx| {
                                            v2.update(cx, |v, _| window.focus(&v.focus));
                                            crate::overlays::find_in_terminal(Some(t.clone()), window, cx);
                                        }
                                    }),
                                ]
                            });
                        }
                        return;
                    }
                }
                view.update(cx, |v, cx| {
                    window.focus(&v.focus);
                    v.mouse_down(ev.button, local, w, h, ev.click_count, &ev.modifiers, cx)
                });
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| v.mouse_move(local, w, h, ev.pressed_button, &ev.modifiers, cx));
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| v.mouse_up(ev.button, local, &ev.modifiers, cx));
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| v.scroll_wheel(ev, local, cx));
            }
        });
        window.on_modifiers_changed({
            let view = view.clone();
            move |ev: &ModifiersChangedEvent, _window, cx| {
                view.update(cx, |v, cx| v.modifiers_changed(&ev.modifiers, cx));
            }
        });

        self.view.update(cx, |v, _| {
            // 视口离开了底部（键盘翻页 / 拖滑块 / 滚轮）= 人在翻：滚动条显形；贴底时新输出不算
            let off = v.term.lock().grid().display_offset();
            if off != v.last_display_offset {
                v.last_display_offset = off;
                if off > 0 {
                    let now = v.now_ms();
                    v.scroll_activity.ping(now);
                }
            }
            v.on_painted()
        });
    }
}
