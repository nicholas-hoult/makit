//! 字号缩放 + ⌘F 搜索计数的纯逻辑（TS 版 `src/fontZoom.ts`、`src/searchCount.ts`）。
//!
//! 字号：只作用于当前 pane、不持久化（和 iTerm2 一样，刻意的选择）；改完走同一条 fit 路径同步 PTY（#156，
//! 不许另开 resize 路径）。按键判定（⌘= / ⌘⇧= / ⌘- / ⌘0，不带 ⌃⌥）由快捷键表管，见 actions/mod.rs。
//!
//! 搜索计数：`index` 从 0 起（显示要 +1）；超过高亮上限时 index 为 -1 → 只报总数；count 为 0 显示「无结果」
//! （这条必须排在 index < 0 之前）；搜索词为空不显示。
//!
//! 为什么单独测：边界值（撞上下限返回原值 = 调用方跳过 fit；0/17 这种差一错误）在界面上不显眼，但数字是错的。

pub const DEFAULT_FONT_SIZE: f32 = 13.0;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 32.0;
/// 搜索高亮上限（xterm search addon 的 highlightLimit）：超过它 index 为 -1
pub const HIGHLIGHT_LIMIT: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// 当前字号 + 动作 → 新字号（夹到 8–32）。返回值等于 cur 就是撞到边界了，调用方据此跳过 fit
pub fn next_font_size(cur: f32, a: Zoom) -> f32 {
    let raw = match a {
        Zoom::In => cur + 1.0,
        Zoom::Out => cur - 1.0,
        Zoom::Reset => DEFAULT_FONT_SIZE,
    };
    raw.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// 「3/17」。`progress` = (index, count)，还没收到过结果传 None
pub fn format_search_count(term: &str, progress: Option<(i64, usize)>) -> Option<String> {
    if term.trim().is_empty() {
        return None;
    }
    let (index, count) = progress?;
    if count == 0 {
        return Some("无结果".into());
    }
    if index < 0 || index as usize >= count {
        return Some(format!("{count} 个结果"));
    }
    Some(format!("{}/{count}", index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size() {
        assert_eq!((DEFAULT_FONT_SIZE, MIN_FONT_SIZE, MAX_FONT_SIZE), (13.0, 8.0, 32.0));
        assert_eq!(next_font_size(13.0, Zoom::In), 14.0);
        assert_eq!(next_font_size(13.0, Zoom::Out), 12.0);
        assert_eq!(next_font_size(8.0, Zoom::Out), 8.0, "撞下限停在 8（等于入参 → 跳过 fit）");
        assert_eq!(next_font_size(32.0, Zoom::In), 32.0, "撞上限停在 32");
        assert_eq!(next_font_size(20.0, Zoom::Reset), 13.0);
        assert_eq!(next_font_size(13.0, Zoom::Reset), 13.0, "已经是 13 时 reset 还是 13（等于入参）");
    }

    #[test]
    fn search_count() {
        let s = |t: &str, p| format_search_count(t, p);
        assert_eq!(s("foo", Some((0, 17))).as_deref(), Some("1/17"), "第一个匹配显示 1/17");
        assert_eq!(s("foo", Some((4, 17))).as_deref(), Some("5/17"));
        assert_eq!(s("foo", Some((16, 17))).as_deref(), Some("17/17"));
        assert_eq!(s("foo", Some((0, 1))).as_deref(), Some("1/1"));
        assert_eq!(s("zzz", Some((-1, 0))).as_deref(), Some("无结果"), "没有匹配显示无结果（排在 index<0 之前）");
        assert_eq!(s("foo", Some((-1, 1500))).as_deref(), Some("1500 个结果"), "超过高亮上限只报总数");
        assert_eq!(s("foo", Some((20, 17))).as_deref(), Some("17 个结果"), "越界也只报总数");
        assert_eq!(s("", Some((-1, 0))), None, "搜索词为空 → 不显示");
        assert_eq!(s("  ", Some((-1, 0))), None, "只有空白 → 不显示");
        assert_eq!(s("foo", None), None, "还没收到任何结果 → 不显示");
    }
}
