//! 终端的绘制元素（从 mod.rs 挪出来，#226）：prepaint 时锁一下 Term、把可见区域拍成背景块 + 文字段，paint 时画出来，
//! 顺带挂鼠标 / 滚轮 / 输入法监听。

use gpui::{
    fill, font, point, px, relative, size, App, Bounds, Element, ElementId, ElementInputHandler, Entity, Font, FontStyle,
    FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window,
};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;

use super::grid::grid_size;
use super::{dim, hsla, palette, resolve, TerminalView, FONT_FAMILY, FONT_SIZE, LINE_HEIGHT};

// ── 绘制 ──

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
    lines: Vec<TextLine>,
    cursor: Option<(Bounds<Pixels>, bool)>,
    marked: Option<(Point<Pixels>, ShapedLine)>,
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
        self.fg == fg
            && self.bold == bold
            && self.italic == italic
            && self.underline == underline
            && self.strike == strike
            && self.wide == wide
    }
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

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
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
        let font_size = px(FONT_SIZE);
        let base_font = font(FONT_FAMILY);
        let ts = window.text_system().clone();
        let font_id = ts.resolve_font(&base_font);
        let cell_w = ts.advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(px(8.0));
        let line_h = px(LINE_HEIGHT);
        let (cols, rows) = grid_size(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            f32::from(cell_w),
            f32::from(line_h),
        );
        self.view.update(cx, |v, _| v.resize(cols, rows, cell_w, line_h));

        let view = self.view.read(cx);
        let focused = view.focus.is_focused(window);
        let marked_text = view.marked_text.clone();
        let term = view.term.clone();
        let term = term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let origin = bounds.origin;
        let cell_rect = |col: usize, row: usize, n: usize| {
            Bounds::new(
                point(origin.x + cell_w * col as f32, origin.y + line_h * row as f32),
                size(cell_w * n as f32, line_h),
            )
        };

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
            let row = (cell.point.line.0 + offset) as usize;
            let col = cell.point.column.0;
            if row >= rows as usize {
                continue;
            }
            let flags = cell.flags;
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let bg_rgb = resolve(bg, colors);
            if bg_rgb != palette::BG || flags.contains(Flags::INVERSE) {
                match &mut bg_run {
                    Some((r, c, n, color)) if *r == row && *c + *n == col && *color == bg_rgb => *n += 1,
                    _ => {
                        flush_bg(&mut bg_run, &mut rects);
                        bg_run = Some((row, col, 1, bg_rgb));
                    }
                }
            }
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN) {
                continue;
            }
            let wide = flags.contains(Flags::WIDE_CHAR);
            let mut fg_rgb = resolve(fg, colors);
            if flags.contains(Flags::DIM) {
                fg_rgb = dim(fg_rgb);
            }
            let bold = flags.contains(Flags::BOLD);
            let italic = flags.contains(Flags::ITALIC);
            let underline = flags.intersects(Flags::ALL_UNDERLINES);
            let strike = flags.contains(Flags::STRIKEOUT);
            let runs = &mut runs_by_row[row];
            let width = if wide { 2 } else { 1 };
            let extend = match runs.last() {
                Some(r) => r.col + r.cells == col && r.same_style(fg_rgb, bold, italic, underline, strike, wide),
                None => false,
            };
            if cell.c == ' ' && !extend {
                continue; // 空格不开新段
            }
            if extend {
                let r = runs.last_mut().unwrap();
                r.text.push(cell.c);
                r.cells += width;
            } else {
                runs.push(Run {
                    col,
                    cells: width,
                    wide,
                    text: cell.c.to_string(),
                    fg: fg_rgb,
                    bold,
                    italic,
                    underline,
                    strike,
                });
            }
            if let Some(zw) = cell.zerowidth() {
                let r = runs.last_mut().unwrap();
                r.text.extend(zw.iter());
            }
        }
        flush_bg(&mut bg_run, &mut rects);

        // 选区
        if let Some(sel) = content.selection {
            let start_row = sel.start.line.0 + offset;
            let end_row = sel.end.line.0 + offset;
            for row in start_row.max(0)..=end_row.min(rows as i32 - 1) {
                let from = if row == start_row { sel.start.column.0 } else { 0 };
                let to = if row == end_row { sel.end.column.0 } else { cols as usize - 1 };
                if to >= from {
                    let mut c: Hsla = hsla(palette::SELECTION);
                    c.a = 0.85;
                    rects.push((cell_rect(from, row as usize, to - from + 1), c));
                }
            }
        }

        // 光标
        let cur = content.cursor;
        let cursor = if cur.shape != CursorShape::Hidden {
            let row = cur.point.line.0 + offset;
            if row >= 0 && (row as u16) < rows {
                let wide = term.grid()[cur.point].flags.contains(Flags::WIDE_CHAR);
                let mut b = cell_rect(cur.point.column.0, row as usize, if wide { 2 } else { 1 });
                match cur.shape {
                    CursorShape::Beam => b.size.width = px(2.0),
                    CursorShape::Underline => {
                        b.origin.y += line_h - px(2.0);
                        b.size.height = px(2.0);
                    }
                    _ => {}
                }
                Some((b, focused && cur.shape != CursorShape::HollowBlock))
            } else {
                None
            }
        } else {
            None
        };
        // 回看时右侧画一条细滚动条（只在离开底部时出现，回到底部就消失）
        if offset > 0 {
            let history = term.grid().history_size() as f32;
            let total = history + rows as f32;
            let h = bounds.size.height * (rows as f32 / total).max(0.05);
            let top = (bounds.size.height - h) * ((history - offset as f32) / history.max(1.0));
            let mut color = hsla(palette::FG);
            color.a = 0.35;
            rects.push((
                Bounds::new(point(origin.x + bounds.size.width - px(6.0), origin.y + top), size(px(4.0), h)),
                color,
            ));
        }
        let cursor_origin = cursor.map(|(b, _)| point(b.origin.x, origin.y + line_h * ((cur.point.line.0 + offset).max(0) as f32)));
        drop(term);

        // 文字：逐段排版（shape 有缓存，同样的文字下一帧不再排）
        let mut lines = Vec::new();
        for (row, runs) in runs_by_row.into_iter().enumerate() {
            for mut r in runs {
                let trimmed = r.text.trim_end_matches(' ').len();
                r.text.truncate(trimmed);
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
                    underline: r.underline.then(|| UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
                    strikethrough: r.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
                };
                let force = if r.wide { cell_w * 2.0 } else { cell_w };
                let shaped = ts.shape_line(SharedString::from(r.text), font_size, &[run], Some(force));
                lines.push(TextLine { origin: point(origin.x + cell_w * r.col as f32, origin.y + line_h * row as f32), line: shaped });
            }
        }

        let marked = match (marked_text, cursor_origin) {
            (Some(t), Some(o)) => {
                let color = hsla(palette::FG);
                let run = TextRun {
                    len: t.len(),
                    font: base_font.clone(),
                    color,
                    background_color: Some(hsla(palette::BG)),
                    underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
                    strikethrough: None,
                };
                Some((o, ts.shape_line(SharedString::from(t), font_size, &[run], None)))
            }
            _ => None,
        };

        Frame { hitbox, rects, lines, cursor, marked }
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
        window.set_cursor_style(gpui::CursorStyle::IBeam, &frame.hitbox);

        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (b, c) in &frame.rects {
                window.paint_quad(fill(*b, *c));
            }
            for l in &frame.lines {
                let _ = l.line.paint(l.origin, px(LINE_HEIGHT), window, cx);
            }
            if let Some((b, solid)) = frame.cursor {
                let mut c = hsla(palette::CURSOR);
                if solid {
                    c.a = 0.6;
                    window.paint_quad(fill(b, c));
                } else {
                    window.paint_quad(gpui::outline(b, c, gpui::BorderStyle::Solid));
                }
            }
            if let Some((o, line)) = &frame.marked {
                window.paint_quad(fill(Bounds::new(*o, size(line.width, px(LINE_HEIGHT))), hsla(palette::BG)));
                let _ = line.paint(*o, px(LINE_HEIGHT), window, cx);
            }
        });

        // 鼠标：选区、滚轮
        let view = self.view.clone();
        let hitbox = frame.hitbox.clone();
        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != gpui::DispatchPhase::Bubble || ev.button != MouseButton::Left || !hitbox.is_hovered(window) {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| {
                    v.focus.focus(window);
                    v.mouse_down(local, ev.click_count, cx)
                });
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &MouseMoveEvent, phase, _window, cx| {
                if phase != gpui::DispatchPhase::Bubble || ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| v.mouse_drag(local, cx));
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &MouseUpEvent, phase, _window, cx| {
                if phase != gpui::DispatchPhase::Bubble || ev.button != MouseButton::Left {
                    return;
                }
                view.update(cx, |v, cx| {
                    if v.selecting {
                        v.mouse_up(cx)
                    }
                });
            }
        });
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase != gpui::DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let local = ev.position - bounds.origin;
                view.update(cx, |v, cx| v.scroll_wheel(ev, local, cx));
            }
        });

        self.view.update(cx, |v, _| v.on_painted());
    }
}
