//! 单行输入框（带输入法）。GPUI 0.2 没有现成的输入控件，这里照 gpui 自带的 `examples/input.rs`
//! 改写（Apache-2.0）：`EntityInputHandler` 接系统输入法（中文拼音的 marked text 画下划线），
//! 编辑键走 `actions::overlays::Input*`（context `TextInput`，在快捷键总表里）。
//!
//! 和示例的差别：光标按字符（char）而不是字素簇移动（不多引一个依赖；中文一字一 char，够用）；
//! 颜色取主题；内容变了发 `TextInputEvent::Changed`，父视图订阅它做实时搜索。
//!
//! 用法：
//! ```ignore
//! let input = cx.new(|cx| TextInput::new("搜索 session...", cx));
//! cx.subscribe(&input, |this, input, _: &TextInputEvent, cx| { let q = input.read(cx).text().to_string(); … }).detach();
//! // 渲染：父元素定字号 / 行高，input 自己撑满宽度
//! div().text_size(px(15.)).child(input.clone())
//! ```
//! Enter / Esc / ↑↓ 不在这里处理：没绑在 `TextInput` 上，会冒泡给外层浮层（context `Overlay`）。

use std::ops::Range;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};

use crate::actions::overlays as act;
use crate::theme::ActiveTheme;

#[derive(Clone, Debug)]
pub enum TextInputEvent {
    /// 内容变了（打字、粘贴、删除、`set_text`）
    Changed,
}

pub struct TextInput {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// 整体替换内容，光标放到末尾（点搜索历史填入、清空……）
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected = self.content.len()..self.content.len();
        self.reversed = false;
        self.marked = None;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    pub fn set_placeholder(&mut self, p: impl Into<SharedString>) {
        self.placeholder = p.into();
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    // ---- 编辑键 ----

    fn left(&mut self, _: &act::InputLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(prev_boundary(&self.content, self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx)
        }
    }

    fn right(&mut self, _: &act::InputRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(next_boundary(&self.content, self.selected.end), cx);
        } else {
            self.move_to(self.selected.end, cx)
        }
    }

    fn select_left(&mut self, _: &act::InputSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(prev_boundary(&self.content, self.cursor()), cx);
    }

    fn select_right(&mut self, _: &act::InputSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_boundary(&self.content, self.cursor()), cx);
    }

    fn select_all(&mut self, _: &act::InputSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    fn home(&mut self, _: &act::InputHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &act::InputEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &act::InputSelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &act::InputSelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &act::InputBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(prev_boundary(&self.content, self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &act::InputDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(next_boundary(&self.content, self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_to_start(&mut self, _: &act::InputDeleteToStart, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(0, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &act::InputPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // 单行：换行压成空格
            self.replace_text_in_range(None, &text.replace(['\n', '\r'], " "), window, cx);
        }
    }

    fn copy(&mut self, _: &act::InputCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &act::InputCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    // ---- 鼠标 ----

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.selecting = true;
        let at = self.index_for_mouse(ev.position);
        if ev.modifiers.shift {
            self.select_to(at, cx);
        } else if ev.click_count >= 2 {
            self.select_all_text(cx);
        } else {
            self.move_to(at, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_for_mouse(ev.position), cx);
        }
    }

    fn index_for_mouse(&self, pos: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if pos.y < bounds.top() {
            return 0;
        }
        if pos.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(pos.x - bounds.left())
    }

    // ---- 光标 / 选区 ----

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        cx.notify()
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset
        } else {
            self.selected.end = offset
        };
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_to_utf8(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf8_to_utf16(&self.content, offset)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

/// UTF-16 下标 → UTF-8 字节下标（系统输入法用 UTF-16 计数）
pub fn utf16_to_utf8(s: &str, offset: usize) -> usize {
    let (mut u8i, mut u16i) = (0, 0);
    for ch in s.chars() {
        if u16i >= offset {
            break;
        }
        u16i += ch.len_utf16();
        u8i += ch.len_utf8();
    }
    u8i
}

pub fn utf8_to_utf16(s: &str, offset: usize) -> usize {
    let (mut u16i, mut u8i) = (0, 0);
    for ch in s.chars() {
        if u8i >= offset {
            break;
        }
        u8i += ch.len_utf8();
        u16i += ch.len_utf16();
    }
    u16i
}

fn prev_boundary(s: &str, offset: usize) -> usize {
    s[..offset.min(s.len())].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
}

fn next_boundary(s: &str, offset: usize) -> usize {
    s.char_indices().map(|(i, _)| i).find(|&i| i > offset).unwrap_or(s.len())
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, r16: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = self.range_from_utf16(&r16);
        actual.replace(self.range_to_utf16(&r));
        Some(self.content[r].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, r16: Option<Range<usize>>, new_text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let r = r16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..r.start], new_text, &self.content[r.end..]);
        self.selected = r.start + new_text.len()..r.start + new_text.len();
        self.reversed = false;
        self.marked.take();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        r16: Option<Range<usize>>,
        new_text: &str,
        new_sel16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = r16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..r.start], new_text, &self.content[r.end..]);
        self.marked = if new_text.is_empty() { None } else { Some(r.start..r.start + new_text.len()) };
        self.selected = new_sel16
            .as_ref()
            .map(|s| utf16_to_utf8(new_text, s.start) + r.start..utf16_to_utf8(new_text, s.end) + r.start)
            .unwrap_or_else(|| r.start + new_text.len()..r.start + new_text.len());
        // 拼音组字过程中不发 Changed：候选字还没定，拿去搜索只会闪一堆无关结果
        cx.notify();
    }

    fn bounds_for_range(&mut self, r16: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let r = self.range_from_utf16(&r16);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(r.start), bounds.top()),
            point(bounds.left() + layout.x_for_index(r.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let local = self.last_bounds?.localize(&p)?;
        let layout = self.last_layout.as_ref()?;
        let idx = layout.index_for_x(p.x - local.x)?;
        Some(self.offset_to_utf16(idx))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct Prepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let theme = cx.theme().clone();
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color): (SharedString, _) = if input.content.is_empty() {
            (input.placeholder.clone(), theme.fg_subtle)
        } else {
            (input.content.clone().into(), style.color)
        };
        let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let runs = match input.marked.as_ref().filter(|_| !input.content.is_empty()) {
            Some(m) => vec![
                TextRun { len: m.start, ..run.clone() },
                TextRun { len: m.end - m.start, underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }), ..run.clone() },
                TextRun { len: text.len() - m.end, ..run },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(text, font_size, &runs, None);
        let (selection, cursor) = if input.content.is_empty() || input.selected.is_empty() {
            let x = if input.content.is_empty() { px(0.0) } else { line.x_for_index(input.cursor()) };
            (None, Some(fill(Bounds::new(point(bounds.left() + x, bounds.top()), size(px(1.5), bounds.bottom() - bounds.top())), theme.accent)))
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(input.selected.start), bounds.top()),
                        point(bounds.left() + line.x_for_index(input.selected.end), bounds.bottom()),
                    ),
                    theme.selection_bg,
                )),
                None,
            )
        };
        Prepaint { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(sel) = prepaint.selection.take() {
            window.paint_quad(sel)
        }
        let Some(line) = prepaint.line.take() else { return };
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if focus.is_focused(window) {
            if let Some(c) = prepaint.cursor.take() {
                window.paint_quad(c);
            }
        }
        let empty = self.input.read(cx).content.is_empty();
        self.input.update(cx, |input, _| {
            // 占位符的排版不能当成内容的排版（点击定位会算错）
            input.last_layout = if empty { None } else { Some(line) };
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("text-input")
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_roundtrip_with_chinese_and_emoji() {
        let s = "a中🐉b";
        // a=1/1, 中=3/1, 🐉=4/2, b=1/1
        assert_eq!(utf8_to_utf16(s, 0), 0);
        assert_eq!(utf8_to_utf16(s, 1), 1);
        assert_eq!(utf8_to_utf16(s, 4), 2);
        assert_eq!(utf8_to_utf16(s, 8), 4);
        assert_eq!(utf16_to_utf8(s, 4), 8);
        assert_eq!(utf16_to_utf8(s, 5), 9);
        assert_eq!(utf16_to_utf8(s, 99), s.len(), "越界按末尾");
    }

    #[test]
    fn boundaries_step_whole_chars() {
        let s = "中文";
        assert_eq!(next_boundary(s, 0), 3);
        assert_eq!(next_boundary(s, 3), 6);
        assert_eq!(next_boundary(s, 6), 6);
        assert_eq!(prev_boundary(s, 6), 3);
        assert_eq!(prev_boundary(s, 0), 0);
    }
}
