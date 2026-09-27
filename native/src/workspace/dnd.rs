//! 拖拽载荷 + 拖动时跟着鼠标的那块「影子」（`.drag-ghost`）。
//!
//! GPUI 的拖放按**载荷类型**匹配（`on_drag(T)` ↔ `on_drop::<T>` / `on_drag_move::<T>`），相当于 TS 版的 MIME：
//! - `TabDrag`：工作区标签（= `CONTAINER_TAB_MIME`）
//! - `SessionDrag`：侧栏会话行（= `PANE_SPEC_MIME`）。**B 侧栏包发起拖拽时用它**：
//!   `.on_drag(SessionDrag { spec, title }, |d, _, _, cx| workspace::dnd::ghost(&d.title, cx))`
//! - Finder 拖进来的文件是 GPUI 自带的 `ExternalPaths`
//!
//! 只有这三种会被工作区接住（白名单天然成立，见 drop.rs 的 `DragKind`）。

use gpui::{div, prelude::*, px, App, BoxShadow, Context, Entity, FontWeight, SharedString, Window, point};

use super::model::TabSpec;
use crate::theme::ActiveTheme;

/// 拖一个工作区标签
#[derive(Clone, Debug)]
pub struct TabDrag {
    pub container_id: String,
    pub tab_id: String,
    pub title: String,
}

/// 从侧栏拖一个会话进工作区
#[derive(Clone, Debug)]
pub struct SessionDrag {
    pub spec: TabSpec,
    pub title: String,
}

/// 拖动影子：accent 底的小药丸（App.css `.drag-ghost`）
pub struct DragGhost {
    title: SharedString,
}

pub fn ghost(title: &str, cx: &mut App) -> Entity<DragGhost> {
    cx.new(|_| DragGhost { title: SharedString::from(title.to_string()) })
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px(px(10.0))
            .py(px(4.0))
            .bg(t.accent)
            .text_color(t.accent_fg)
            .rounded(px(4.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .max_w(px(280.0))
            .truncate()
            .shadow(vec![BoxShadow { color: t.shadow, offset: point(px(0.0), px(4.0)), blur_radius: px(12.0), spread_radius: px(0.0) }])
            .child(self.title.clone())
    }
}
