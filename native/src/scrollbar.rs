//! 通用细滚动条：给浮层（命令面板、设置、通知抽屉、会话详情）用，长相和时序照 Tauri 版全局那套
//! （App.css 的 `::-webkit-scrollbar` 规范 + scrollActivity.ts）：
//! - 命中区 9px、看着 5px 的圆角细条（上下各留 3px），最短 34px，颜色从 fg 混出来（滚动时 30%，指针放上去 50%）
//! - **只在滚动时显形**，停手 900ms 后淡出；拖着的时候一直亮
//! - 内容没溢出就什么都不画
//!
//! 几何（`thumb_geometry` / `scroll_for_thumb`）直接用侧栏那套（`sidebar::tree`，已有测试）。
//! 侧栏和终端有各自的实现（滚动来源不同：虚拟列表 / 终端网格），这里服务的是两类来源：
//! `ScrollHandle`（普通 overflow-scroll 的 div）和 gpui 的 `ListState`（详情面板的变高列表）。
//!
//! 用法：视图里存一个 `Entity<Scrollbar>`（`Scrollbar::handle(&scroll)` / `Scrollbar::list(&state)`），
//! 渲染时把它作为**滚动容器的同级、绝对定位**的子元素：
//! `div().relative().child(滚动容器).child(self.scrollbar.clone())`。滚动条自己 absolute 贴右边、上下撑满，
//! 不占布局宽度、也不挡下面的内容（只有滑块本身接鼠标）。
//!
//! 为什么不需要滚动事件：滚轮会让窗口重绘，滚动条每次渲染都读一次滚动位置，跟上次不一样就是「刚滚过」。

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, Context, DragMoveEvent, Entity, EntityId, IntoElement, ListState, MouseButton, Render, ScrollHandle, Task, Window,
};

use crate::overlays::style::mix_alpha;
use crate::sidebar::tree::{scroll_for_thumb, thumb_geometry, SCROLLBAR_W};
use crate::theme::ActiveTheme;

/// 停手多久后淡出（scrollActivity.ts 的 idleMs）
pub const FADE_MS: u64 = 900;
/// 滚动位置变化超过这个量才算「在滚」（浮点抖动不算）
const SCROLLED_EPS: f32 = 0.5;

/// 滚动位置从哪读、往哪写
pub enum ScrollSource {
    Handle(ScrollHandle),
    List(ListState),
}

/// 内容总高、视口高、当前滚动量（都是正数，逻辑像素）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub total: f32,
    pub viewport: f32,
    pub top: f32,
}

impl ScrollSource {
    pub fn metrics(&self) -> Metrics {
        match self {
            ScrollSource::Handle(h) => {
                let viewport = f32::from(h.bounds().size.height);
                Metrics { total: viewport + f32::from(h.max_offset().height), viewport, top: -f32::from(h.offset().y) }
            }
            ScrollSource::List(l) => {
                let viewport = f32::from(l.viewport_bounds().size.height);
                Metrics { total: viewport + f32::from(l.max_offset_for_scrollbar().height), viewport, top: -f32::from(l.scroll_px_offset_for_scrollbar().y) }
            }
        }
    }

    fn set_top(&self, y: f32) {
        match self {
            ScrollSource::Handle(h) => {
                let o = h.offset();
                h.set_offset(point(o.x, px(-y)));
            }
            ScrollSource::List(l) => l.set_offset_from_scrollbar(point(px(0.0), px(-y))),
        }
    }

    fn drag_started(&self) {
        if let ScrollSource::List(l) = self {
            l.scrollbar_drag_started();
        }
    }

    fn drag_ended(&self) {
        if let ScrollSource::List(l) = self {
            l.scrollbar_drag_ended();
        }
    }
}

/// 拖动时鼠标位置 → 滑块顶（相对轨道）。`grab` 是按下时鼠标在滑块里的纵向偏移
pub fn thumb_top_for_pointer(pointer_y: f32, track_top: f32, grab: f32) -> f32 {
    pointer_y - track_top - grab
}

/// 这次渲染读到的滚动量和上次比，算不算「刚滚过」
pub fn scrolled(prev_top: f32, top: f32) -> bool {
    (top - prev_top).abs() > SCROLLED_EPS
}

/// 该不该因为这次位置变化让滚动条显形。内容刚变（实时追加、折叠展开）时位置会跟着变，那不是用户在滚，
/// 显形了就是每来一批新内容滚动条闪一下（#231「查看对话还是会刷一下」）
pub fn wakes(scrolled: bool, content_just_changed: bool) -> bool {
    scrolled && !content_just_changed
}

/// 内容变化之后多久内不因位置变化显形（够布局跟上；用户在这段时间里的滚动仍会在下一次位置变化时显形）
const QUIET_MS: u64 = 250;

/// 拖动滑块的标记，带上滚动条自己的实体 id（同屏多个滚动条时只响应自己那个）
#[derive(Clone, Copy)]
struct ScrollbarDrag(EntityId);

/// 拖动时跟着鼠标的东西：什么都不画
struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct Scrollbar {
    source: ScrollSource,
    last_top: f32,
    active: bool,
    fade: Option<Task<()>>,
    /// 拖动时鼠标在滑块里的纵向偏移
    grab: f32,
    dragging: bool,
    /// 轨道离容器顶的距离：滚动区上面还垫着 padding 时（详情面板 pt 14）轨道要跟滚动区的视口对齐，
    /// 不然滑块位置会整体偏一个 padding
    inset_top: f32,
    /// 内容刚变过（调用方 `content_changed()`）：到这个时刻之前，位置变化不算「在滚」
    quiet_until: Option<std::time::Instant>,
}

impl Scrollbar {
    pub fn new(source: ScrollSource) -> Self {
        Self { source, last_top: 0.0, active: false, fade: None, grab: 0.0, dragging: false, inset_top: 0.0, quiet_until: None }
    }

    /// 内容刚变了（实时追加 / 折叠展开）：接下来一小会儿位置变化不让滚动条显形
    pub fn content_changed(&mut self) {
        self.quiet_until = Some(std::time::Instant::now() + Duration::from_millis(QUIET_MS));
    }

    pub fn with_inset_top(mut self, px: f32) -> Self {
        self.inset_top = px;
        self
    }

    pub fn handle(h: &ScrollHandle, cx: &mut gpui::App) -> Entity<Self> {
        cx.new(|_| Self::new(ScrollSource::Handle(h.clone())))
    }

    pub fn list(l: &ListState, cx: &mut gpui::App) -> Entity<Self> {
        cx.new(|_| Self::new(ScrollSource::List(l.clone())))
    }

    /// 鼠标拖到 `pointer_y`（窗口坐标）时，把内容滚到对应位置。`track_top` 是轨道顶的窗口坐标
    fn drag_to(&mut self, pointer_y: f32, track_top: f32) {
        let m = self.source.metrics();
        let thumb_top = thumb_top_for_pointer(pointer_y, track_top, self.grab);
        self.source.set_top(scroll_for_thumb(m.total, m.viewport, thumb_top));
        self.last_top = self.source.metrics().top;
    }

    /// 自检用：不经过鼠标事件（gpui 没有对外的注入接口），直接走「按住滑块 grab 处 → 拖到 pointer_y」这条换算
    pub fn debug_drag(&mut self, grab: f32, pointer_y: f32, track_top: f32, cx: &mut Context<Self>) {
        self.grab = grab;
        self.dragging = true;
        self.source.drag_started();
        self.drag_to(pointer_y, track_top);
        cx.notify();
    }

    /// 自检用：(显形中, 拖动中)
    pub fn debug_state(&self) -> (bool, bool) {
        (self.active, self.dragging)
    }

    /// 换一个滚动来源（详情面板切排序时列表会重建）
    pub fn set_source(&mut self, source: ScrollSource) {
        self.source = source;
        self.last_top = self.source.metrics().top;
    }

    pub fn debug_metrics(&self) -> Metrics {
        self.source.metrics()
    }

    /// 刚滚过：显形，并把「900ms 后淡出」重新计时（旧的计时随 Task 一起丢掉）
    fn ping(&mut self, cx: &mut Context<Self>) {
        self.active = true;
        self.fade = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(FADE_MS)).await;
            let _ = this.update(cx, |this, cx| {
                this.active = false;
                cx.notify();
            });
        }));
    }
}

impl Render for Scrollbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 拖拽结束（松手 / 取消）：放开列表的「拖动中锁定总高」
        if self.dragging && !cx.has_active_drag() {
            self.dragging = false;
            self.source.drag_ended();
        }
        let m = self.source.metrics();
        if scrolled(self.last_top, m.top) {
            self.last_top = m.top;
            let quiet = self.quiet_until.is_some_and(|t| std::time::Instant::now() < t);
            if wakes(true, quiet) {
                self.ping(cx);
            }
        }
        let Some((top, h)) = thumb_geometry(m.total, m.viewport, m.top) else {
            // 没溢出：不画，也不占位
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let visible = self.active || self.dragging;
        let id = cx.entity_id();
        let this = cx.entity().downgrade();
        div()
            .absolute()
            .top(px(self.inset_top))
            .bottom_0()
            .right_0()
            .w(px(SCROLLBAR_W))
            // 拖动：整个轨道听 mouse move（不用管鼠标有没有离开滑块），按滑块被抓住的位置换算成滚动量
            .on_drag_move(cx.listener(move |this, ev: &DragMoveEvent<ScrollbarDrag>, window, cx| {
                if ev.drag(cx).0 != id {
                    return;
                }
                this.drag_to(f32::from(ev.event.position.y), f32::from(ev.bounds.top()));
                cx.notify();
                window.refresh();
            }))
            .child(
                div()
                    .id("scrollbar-thumb")
                    .absolute()
                    .right_0()
                    .top(px(top))
                    .w(px(SCROLLBAR_W))
                    .h(px(h))
                    .py(px(3.0))
                    .px(px(2.0))
                    .child(
                        div()
                            .size_full()
                            .rounded_full()
                            .when(visible, |d| d.bg(mix_alpha(theme.fg, 0.3)).hover(|s| s.bg(mix_alpha(theme.fg, 0.5))))
                            .when(self.dragging, |d| d.bg(mix_alpha(theme.fg, 0.5))),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_drag(ScrollbarDrag(id), move |_, grab, _, cx| {
                        let _ = this.update(cx, |s, _| {
                            s.grab = f32::from(grab.y);
                            s.dragging = true;
                            s.source.drag_started();
                        });
                        cx.new(|_| Ghost)
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 为什么要测：拖动的换算错了，界面上就是「拖滑块，内容跟着跳 / 滑块跑得比鼠标快」（#228 终端滚动条出过这个毛病）。
    /// 这里验的是几何和换算互为反函数：任何滚动量 → 滑块位置 → 鼠标位置 → 滚动量，回到原处
    #[test]
    fn dragging_maps_back_to_the_same_scroll_position() {
        let (total, viewport) = (3000.0, 500.0);
        for top in [0.0, 1.0, 750.0, 1499.0, 2500.0] {
            let (thumb_top, _) = thumb_geometry(total, viewport, top).unwrap();
            let (track_top, grab) = (120.0, 7.0);
            // 鼠标按在滑块里 grab 处，滑块在 thumb_top：此时鼠标的 y
            let pointer_y = track_top + thumb_top + grab;
            let back = scroll_for_thumb(total, viewport, thumb_top_for_pointer(pointer_y, track_top, grab));
            assert!((back - top).abs() < 0.01, "滚动量 {top} 拖一圈回来是 {back}");
        }
    }

    #[test]
    fn pointer_past_the_ends_clamps_to_the_ends() {
        let (total, viewport) = (3000.0, 500.0);
        assert_eq!(scroll_for_thumb(total, viewport, thumb_top_for_pointer(-500.0, 100.0, 5.0)), 0.0, "拖过顶端不越界");
        assert_eq!(scroll_for_thumb(total, viewport, thumb_top_for_pointer(9999.0, 100.0, 5.0)), 2500.0, "拖过底端停在底");
    }

    #[test]
    fn content_changes_do_not_wake_the_scrollbar() {
        assert!(wakes(true, false), "用户在滚：显形");
        assert!(!wakes(true, true), "刚追加了内容、位置跟着变：不显形");
        assert!(!wakes(false, false), "没动：不显形");
    }

    #[test]
    fn tiny_float_jitter_is_not_scrolling() {
        assert!(!scrolled(100.0, 100.3));
        assert!(!scrolled(100.0, 100.0));
        assert!(scrolled(100.0, 101.0));
        assert!(scrolled(100.0, 99.0), "往回滚也算");
    }

    #[test]
    fn nothing_to_draw_when_content_fits() {
        assert_eq!(thumb_geometry(400.0, 500.0, 0.0), None);
        assert_eq!(thumb_geometry(500.0, 500.0, 0.0), None, "刚好放下也不画");
    }
}
