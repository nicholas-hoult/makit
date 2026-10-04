//! 侧栏搜索框：单行文本输入（照 gpui 自带的 `examples/input.rs` 改的最小实现：光标、选区、
//! 鼠标点选 / 拖选、⌘A / ⌘C / ⌘X / ⌘V、输入法 marked text）。
//!
//! 样式照 `.tree-search`：底色 --bg、1px --border 边框（聚焦时 --accent）、圆角 4、内边距 4 24 4 8、
//! 12px 字；占位符「搜索… (⌘⇧F)」用 --fg-muted。右侧有输入时出现 × 清除按钮（`.tree-search-clear`）。
//!
//! 键位在 actions/mod.rs 的快捷键表里（context = `SidebarSearch`）。↓ 发 `SearchEvent::ToList`，
//! 由侧栏把焦点移到列表并选中第一行（D7）。
//!
//! 注：D 浮层包（⌘K）以后如果做了通用输入框，这个可以换成那个；键位 context 故意起了侧栏专用的名字，
//! 免得两边的快捷键表行撞车。

use crate::ts;
use std::ops::Range;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};

use crate::actions::sidebar as act;
use crate::theme::ActiveTheme;

pub enum SearchEvent {
    /// 内容变了（每次都发，侧栏据此重新过滤）
    Changed(String),
    /// 在搜索框里按 ↓：进入列表
    ToList,
}

pub struct SearchInput {
    focus: FocusHandle,
    content: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl EventEmitter<SearchEvent> for SearchInput {}

impl SearchInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: SharedString::default(),
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

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// 换内容（清空 / ⌘L 清除过滤），光标放末尾
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.content.as_ref() == text {
            return;
        }
        self.content = text.to_string().into();
        self.selected = self.content.len()..self.content.len();
        self.marked = None;
        cx.emit(SearchEvent::Changed(self.content.to_string()));
        cx.notify();
    }

    /// ⌘⇧F：聚焦并全选已有内容
    pub fn focus_and_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        self.selected = 0..self.content.len();
        self.reversed = false;
        cx.notify();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(SearchEvent::Changed(self.content.to_string()));
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    // 按字符（不按字素簇）移动：搜索框里组合 emoji 的边界不值得为它加一个依赖
    fn prev_boundary(&self, offset: usize) -> usize {
        self.content[..offset].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..].chars().next().map(|c| offset + c.len_utf8()).unwrap_or(self.content.len())
    }

    fn left(&mut self, _: &act::SearchLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.prev_boundary(self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx);
        }
    }

    fn right(&mut self, _: &act::SearchRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.next_boundary(self.selected.end), cx);
        } else {
            self.move_to(self.selected.end, cx);
        }
    }

    fn select_left(&mut self, _: &act::SearchSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.prev_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &act::SearchSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &act::SearchSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &act::SearchHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &act::SearchEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &act::SearchBackspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.prev_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &act::SearchDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &act::SearchPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &act::SearchCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &act::SearchCut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn to_list(&mut self, _: &act::SearchToList, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SearchEvent::ToList);
    }

    fn index_for_mouse(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if self.content.is_empty() || position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        self.selecting = true;
        if ev.modifiers.shift {
            self.select_to(self.index_for_mouse(ev.position), cx);
        } else {
            self.move_to(self.index_for_mouse(ev.position), cx);
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

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let (mut u8o, mut u16o) = (0, 0);
        for ch in self.content.chars() {
            if u16o >= offset {
                break;
            }
            u16o += ch.len_utf16();
            u8o += ch.len_utf8();
        }
        u8o
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let (mut u16o, mut u8o) = (0, 0);
        for ch in self.content.chars() {
            if u8o >= offset {
                break;
            }
            u8o += ch.len_utf8();
            u16o += ch.len_utf16();
        }
        u16o
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

impl EntityInputHandler for SearchInput {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = self.range_from_utf16(&range);
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

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let r = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = (self.content[..r.start].to_owned() + text + &self.content[r.end..]).into();
        self.selected = r.start + text.len()..r.start + text.len();
        self.reversed = false;
        self.marked = None;
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = (self.content[..r.start].to_owned() + text + &self.content[r.end..]).into();
        self.marked = (!text.is_empty()).then(|| r.start..r.start + text.len());
        self.selected = new_selected
            .as_ref()
            .map(|n| self.range_from_utf16(n))
            .map(|n| n.start + r.start..n.end + r.start)
            .unwrap_or_else(|| r.start + text.len()..r.start + text.len());
        // 输入法组字过程中不发 Changed：拼音字母不该拿去过滤（组完字 replace_text_in_range 再发）
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let r = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(r.start), bounds.top()),
            point(bounds.left() + line.x_for_index(r.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let local = self.last_bounds?.localize(&p)?;
        let line = self.last_layout.as_ref()?;
        let i = line.index_for_x(p.x - local.x)?;
        Some(self.offset_to_utf16(i))
    }
}

/// 真正画字、光标、选区的元素（同 gpui 示例的 TextElement）
struct TextElement {
    input: Entity<SearchInput>,
    placeholder: SharedString,
}

struct Prepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
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

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        let theme = cx.theme().clone();
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected = input.selected.clone();
        let cursor = input.cursor();
        let style = window.text_style();
        let (text, color) = if content.is_empty() { (self.placeholder.clone(), theme.fg_muted) } else { (content, theme.fg) };
        let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let runs = match input.marked.as_ref() {
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
        let placeholder = input.content.is_empty();
        let (selection, cursor) = if selected.is_empty() || placeholder {
            let x = if placeholder { px(0.) } else { line.x_for_index(cursor) };
            (None, Some(fill(Bounds::new(point(bounds.left() + x, bounds.top()), size(px(1.), bounds.size.height)), theme.fg)))
        } else {
            let mut sel = theme.accent;
            sel.a = 0.35;
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(selected.start), bounds.top()),
                        point(bounds.left() + line.x_for_index(selected.end), bounds.bottom()),
                    ),
                    sel,
                )),
                None,
            )
        };
        Prepaint { line: Some(line), cursor, selection }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pp: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(sel) = pp.selection.take() {
            window.paint_quad(sel);
        }
        let Some(line) = pp.line.take() else { return };
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if focus.is_focused(window) {
            if let Some(c) = pp.cursor.take() {
                window.paint_quad(c);
            }
        }
        let placeholder = self.input.read(cx).content.is_empty();
        self.input.update(cx, |input, _| {
            // 占位符的排版不能拿来算点击位置
            input.last_layout = (!placeholder).then_some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Focusable for SearchInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// 搜索框高度：12px 字（行盒 14）+ 上下 4 + 边框 1×2（WebKit 里 `.tree-search` 实测 24）
pub const SEARCH_H: f32 = 24.0;

impl Render for SearchInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus.is_focused(window);
        let has_text = !self.content.is_empty();
        div()
            .id("tree-search")
            .key_context("SidebarSearch")
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(SEARCH_H))
            .flex()
            .items_center()
            .pl(px(8.))
            .pr(px(24.))
            .bg(theme.bg)
            .border_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .rounded(px(4.))
            .text_size(px(12.))
            .line_height(px(14.))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::to_list))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(div().flex_1().min_w_0().overflow_hidden().child(TextElement { input: cx.entity(), placeholder: ts!("sidebar.search_placeholder").into() }))
            .when(has_text, |d| {
                // `.tree-search-clear`：right 4、14px、padding 0 4、line-height 1，hover 变 --fg
                d.child(
                    div()
                        .id("tree-search-clear")
                        .absolute()
                        .right(px(4.))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .px(px(4.))
                        .text_size(px(14.))
                        .line_height(px(14.))
                        .text_color(theme.fg_muted)
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.text_color(theme.fg))
                        .child("×")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, _, cx| this.set_text("", cx))),
                )
            })
    }
}
