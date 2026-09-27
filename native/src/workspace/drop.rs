//! 拖放的纯逻辑：落点四区、落点浮层、标签插入线、拖文件插路径。
//!
//! 照搬 `WorkspaceView.tsx` 的 `computeDropZone` / `DropOverlay`、`ContainerView.tsx` 的插入线、
//! `src/paneDrop.ts`（接不接这次拖拽）、`src/dropPaths.ts`（路径加引号）。测试向量来自
//! `scripts/test-pane-drop.ts`、`scripts/test-drop-paths.ts`。
//!
//! 为什么单独测：落点判错 = 松手分到了另一边；插入线画错 = 松手落到别的位置；
//! 路径引号错 = 往用户命令行里插进一个会被 shell 拆开 / 展开的东西（#175 那种「浮层清不掉」也在这里防）。
//!
//! GPUI 里拖拽载荷是 Rust 类型（`TabDrag` / `SessionDrag` / `ExternalPaths`），不是 MIME 字符串，
//! 所以 paneDrop.ts 里「MIME 字面量只许出现一处」那条天然成立；「白名单」语义保留在 `DragKind` 上。
//! Finder 拖进来的文件 GPUI 直接给 `PathBuf`，不用再解析 `text/uri-list`，所以 `fileUrlToPath` /
//! `parseDroppedPaths` 没有移植（没有调用方）。

use super::model::{Dir, Side};

/// 拖进 pane 的东西是什么
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    /// 工作区里的标签（标签条重排 / 跨 pane 移动 / 拖到边上分屏）
    Tab,
    /// 侧栏的会话行（拖到边上分屏）
    Session,
    /// Finder 拖进来的文件（插路径，不分屏）
    Files,
    /// 别的（选中的文本、链接……）
    Other,
}

/// pane 的落点浮层：只有会分屏的两种拖拽才画（白名单；文件插的是一段文本，画「松手就分屏」是句谎话，#175）
pub fn accepts_pane_drop(kind: DragKind) -> bool {
    matches!(kind, DragKind::Tab | DragKind::Session)
}

/// 标签条的「插到这里」指示线：只有标签拖拽算（标签条的 drop 只认它）
pub fn is_tab_drag(kind: DragKind) -> bool {
    kind == DragKind::Tab
}

/// 落在 pane 的哪一侧：离哪条边最近就分到哪边。`(lx, ly)` 是鼠标相对 pane 左上角的坐标。
/// 平手时的优先级 左 > 右 > 上 > 下（同 TS 的 `if` 顺序）
pub fn drop_zone(width: f32, height: f32, lx: f32, ly: f32) -> (Dir, Side) {
    let (fx, fy) = (lx / width, ly / height);
    let (dl, dr, dt, db) = (fx, 1.0 - fx, fy, 1.0 - fy);
    let min = dl.min(dr).min(dt).min(db);
    if min == dl {
        (Dir::V, Side::Before)
    } else if min == dr {
        (Dir::V, Side::After)
    } else if min == dt {
        (Dir::H, Side::Before)
    } else {
        (Dir::H, Side::After)
    }
}

/// 落点浮层占 pane 的哪一半，返回 (x, y, w, h) 的比例（0–1）
pub fn overlay_fraction(dir: Dir, side: Side) -> (f32, f32, f32, f32) {
    match (dir, side) {
        (Dir::V, Side::Before) => (0.0, 0.0, 0.5, 1.0),
        (Dir::V, Side::After) => (0.5, 0.0, 0.5, 1.0),
        (Dir::H, Side::Before) => (0.0, 0.0, 1.0, 0.5),
        (Dir::H, Side::After) => (0.0, 0.5, 1.0, 0.5),
    }
}

/// 标签拖到第 `idx` 个标签上：鼠标在它左半边就插到它前面（idx），右半边插到它后面（idx + 1）
pub fn tab_insert_index(idx: usize, mouse_x: f32, tab_left: f32, tab_width: f32) -> usize {
    if mouse_x < tab_left + tab_width / 2.0 {
        idx
    } else {
        idx + 1
    }
}

/// 第 `idx` 个标签要不要画插入线：(前面一根, 后面一根)。
/// 同 ContainerView：`dropIdx == idx` 画在它前面；插到末尾（`dropIdx == len`）只在最后一个标签后面画
pub fn insert_marker(drop_idx: Option<usize>, idx: usize, len: usize) -> (bool, bool) {
    match drop_idx {
        None => (false, false),
        Some(d) => (d == idx, d == idx + 1 && idx + 1 == len),
    }
}

/// shell 里有特殊含义的字符（黑名单：非 ASCII 在 shell 里是普通字符，中文路径不该被套引号）
fn is_shell_special(c: char) -> bool {
    c.is_whitespace() || "|&;<>()$`\\\"'*?[]{}~!#".contains(c)
}

/// 按 POSIX 规则给路径加引号。单引号串里唯一需要处理的就是单引号本身
pub fn quote_for_shell(path: &str) -> String {
    if !path.chars().any(is_shell_special) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// 拼成要写进 PTY 的文本：各自判断引号、空格分隔、末尾补一个空格（Terminal.app / iTerm2 惯例）。
/// 空数组返回空串，调用方据此判断「这次拖拽没东西可插」
pub fn format_paths_for_terminal(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut s = paths.iter().map(|p| quote_for_shell(p)).collect::<Vec<_>>().join(" ");
    s.push(' ');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 接不接（test-pane-drop.ts）----
    #[test]
    fn only_tab_and_session_drags_show_the_pane_overlay() {
        assert!(accepts_pane_drop(DragKind::Tab), "标签拖拽被 pane 接受（拖到边缘分屏）");
        assert!(accepts_pane_drop(DragKind::Session), "会话拖拽被 pane 接受");
        assert!(!accepts_pane_drop(DragKind::Files), "Finder 拖文件不画浮层（#175）");
        assert!(!accepts_pane_drop(DragKind::Other), "拖文本 / 链接不画浮层");
    }

    #[test]
    fn only_tab_drags_show_the_insert_line() {
        assert!(is_tab_drag(DragKind::Tab));
        assert!(!is_tab_drag(DragKind::Session), "会话拖拽不画标签插入线（标签条不处理它）");
        assert!(!is_tab_drag(DragKind::Files));
    }

    // ---- 四区（computeDropZone）----
    #[test]
    fn drop_zone_picks_the_nearest_edge() {
        assert_eq!(drop_zone(100.0, 100.0, 5.0, 50.0), (Dir::V, Side::Before), "靠左 → 左右分、新的在左");
        assert_eq!(drop_zone(100.0, 100.0, 95.0, 50.0), (Dir::V, Side::After), "靠右");
        assert_eq!(drop_zone(100.0, 100.0, 50.0, 5.0), (Dir::H, Side::Before), "靠上");
        assert_eq!(drop_zone(100.0, 100.0, 50.0, 95.0), (Dir::H, Side::After), "靠下");
        // 按比例而不是按像素：宽 400 高 100 时 x=40（10%）比 y=20（20%）更靠边
        assert_eq!(drop_zone(400.0, 100.0, 40.0, 20.0), (Dir::V, Side::Before));
        // 正中间四边等距：左优先
        assert_eq!(drop_zone(100.0, 100.0, 50.0, 50.0), (Dir::V, Side::Before));
    }

    #[test]
    fn overlay_covers_the_half_that_will_be_the_new_pane() {
        assert_eq!(overlay_fraction(Dir::V, Side::Before), (0.0, 0.0, 0.5, 1.0));
        assert_eq!(overlay_fraction(Dir::V, Side::After), (0.5, 0.0, 0.5, 1.0));
        assert_eq!(overlay_fraction(Dir::H, Side::Before), (0.0, 0.0, 1.0, 0.5));
        assert_eq!(overlay_fraction(Dir::H, Side::After), (0.0, 0.5, 1.0, 0.5));
    }

    // ---- 标签插入线（ContainerView onDragOver + className）----
    #[test]
    fn tab_insert_index_by_half() {
        assert_eq!(tab_insert_index(2, 110.0, 100.0, 40.0), 2, "左半边 → 插到它前面");
        assert_eq!(tab_insert_index(2, 125.0, 100.0, 40.0), 3, "右半边 → 插到它后面");
        assert_eq!(tab_insert_index(0, 120.0, 100.0, 40.0), 1, "正中间算右半边（TS 用的是 <）");
    }

    #[test]
    fn insert_marker_draws_one_line_only() {
        assert_eq!(insert_marker(None, 0, 3), (false, false));
        assert_eq!(insert_marker(Some(1), 1, 3), (true, false), "插到第 1 个前面");
        assert_eq!(insert_marker(Some(1), 0, 3), (false, false), "idx+1 的线不画在前一个后面（避免同一位置两根线）");
        assert_eq!(insert_marker(Some(3), 2, 3), (false, true), "插到末尾：画在最后一个后面");
        assert_eq!(insert_marker(Some(3), 1, 3), (false, false));
    }

    // ---- 引号（test-drop-paths.ts）----
    #[test]
    fn quote_for_shell_vectors() {
        assert_eq!(quote_for_shell("/Users/me/a.txt"), "/Users/me/a.txt", "普通路径不加引号");
        assert_eq!(quote_for_shell("/Users/me/My File.txt"), "'/Users/me/My File.txt'", "带空格必须加引号");
        assert_eq!(quote_for_shell("/Users/me/文档.txt"), "/Users/me/文档.txt", "中文路径不加引号");
        assert_eq!(quote_for_shell("/Users/me/it's.txt"), "'/Users/me/it'\\''s.txt'", "单引号用 '\\'' 拼接");
        assert_eq!(quote_for_shell("/Users/me/a$b"), "'/Users/me/a$b'", "$ 要加引号");
        assert_eq!(quote_for_shell("/Users/me/a&b"), "'/Users/me/a&b'", "& 要加引号");
        assert_eq!(quote_for_shell("/Users/me/a!b"), "'/Users/me/a!b'", "! 要加引号（历史展开）");
        assert_eq!(quote_for_shell("/Users/me/a*b"), "'/Users/me/a*b'", "* 要加引号（glob）");
        assert_eq!(quote_for_shell("/Users/me/a-b_c.1,2@3=4"), "/Users/me/a-b_c.1,2@3=4", "常见字符不必加引号");
    }

    #[test]
    fn format_paths_vectors() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(format_paths_for_terminal(&v(&["/a.txt"])), "/a.txt ", "单个文件，带尾随空格");
        assert_eq!(format_paths_for_terminal(&v(&["/a.txt", "/b.txt"])), "/a.txt /b.txt ", "多个文件用空格分隔");
        assert_eq!(format_paths_for_terminal(&v(&["/a b.txt", "/c.txt"])), "'/a b.txt' /c.txt ", "各自判断引号");
        assert_eq!(format_paths_for_terminal(&[]), "", "空数组 → 空串");
    }
}
