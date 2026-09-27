//! 终端滚动条的纯逻辑（#188 / #206 / #210 / #228，TS 版 `src/terminalScrollbar.ts`、`terminalJumpLatest.ts`）。
//!
//! - 只在**人在翻**的时候显形（滚轮落在终端上、视口离开底部、指针在右边缘 14px 里），停手 900ms 淡出；
//!   贴底时新输出带来的「滚动」不算，否则 Claude 一刷屏条子就一直闪。
//! - 显隐只管透明度，不管命中：有可滚内容就始终能抓住拖；alternate buffer / 没有回滚时不画、不命中。
//! - 滑块长度照 xterm（VS Code ScrollbarState）算，再封顶到轨道 1/4、最小 20px；位置按比例重映射。
//! - 拖滑块用同一套几何换算（#228），滑块一直贴着指针。
//!
//! 为什么单独测：`is_user_scroll` 错了条子闪；`Activity` 的边沿错了是「闪烁 / 永远不藏 / 藏了回不来」；
//! `cap_slider` 错了是「滚到底了条子还没到底」；`drag_to_line` 错了是拖着拖着滑块离开了指针。

/// 滚动条（也是命中区）宽度：xterm 6.1 `DEFAULT_SCROLL_BAR_WIDTH`
pub const SCROLLBAR_W: f32 = 14.0;
/// 右边缘这么宽的一条里就让滚动条显形（#206）
pub const GUTTER_PX: f32 = 14.0;
/// xterm（VS Code ScrollbarState）自己的滑块最小长度，封顶时不能比它还短
pub const MIN_SLIDER: f32 = 20.0;
/// 滑块最长占轨道的比例（#188）
pub const MAX_SLIDER_RATIO: f32 = 0.25;
/// 停手多久开始淡出
pub const IDLE_MS: f64 = 900.0;
/// 淡出过渡时长（App.css `.xterm-scrollbar.xterm-visible { transition: opacity 300ms linear }`）
pub const FADE_MS: f64 = 300.0;

/// 视口不在底部 = 有人在看回滚；贴底的滚动是新输出把视口往下带。
/// alacritty 里 `viewport_y < base_y` 等价于 `display_offset > 0`
pub fn is_user_scroll(viewport_y: usize, base_y: usize) -> bool {
    viewport_y < base_y
}

/// 「↓ 回到最新」按钮（#205）：视口离开底部就显示；贴底（含贴底时有新输出）和没有回滚历史都不显示
pub fn should_show_jump_latest(viewport_y: usize, base_y: usize) -> bool {
    is_user_scroll(viewport_y, base_y)
}

/// 指针在不在滚动条那一条里。`x` 是指针横坐标，`right` 是终端的右边缘
pub fn is_in_scrollbar_gutter(x: f32, right: f32) -> bool {
    x > right - GUTTER_PX && x <= right
}

/// xterm 原本会画的滑块（VS Code `ScrollbarState`）：长度 = max(20, floor(可见/总 × 轨道))，
/// 顶端 = 已滚行数 × (轨道 - 长度) / (总 - 可见)。`top_line` 是视口第一行在缓冲区里的行号（0 = 最顶）
pub fn xterm_slider(track: f32, visible_rows: usize, total_rows: usize, top_line: usize) -> (f32, f32) {
    if track <= 0.0 || total_rows == 0 {
        return (track.max(0.0), 0.0);
    }
    if total_rows <= visible_rows {
        return (track, 0.0);
    }
    let size = (MIN_SLIDER.max((visible_rows as f32 * track / total_rows as f32).floor())).round().min(track);
    let ratio = (track - size) / (total_rows - visible_rows) as f32;
    let top = (top_line.min(total_rows - visible_rows) as f32 * ratio).round();
    (size, top)
}

/// 把 xterm 算出的滑块（长度 size、顶端 top）封顶到轨道的 max_ratio，并按比例重映射位置：
/// xterm 的 top ∈ [0, track - size]，截短后映射到 [0, track - capped]，顶贴顶、底贴底。
pub fn cap_slider(track: f32, size: f32, top: f32, max_ratio: f32) -> (f32, f32) {
    if track <= 0.0 {
        return (size, top);
    }
    let cap = MIN_SLIDER.max((track * max_ratio).floor());
    if size <= cap {
        return (size, top);
    }
    let range = track - size;
    let ratio = if range > 0.0 { top / range } else { 0.0 };
    // JS Math.round：.5 向正无穷取整
    (cap, (ratio * (track - cap) + 0.5).floor())
}

/// 拖动滑块时，指针位置 → 视口第一行该是缓冲区第几行（#228）。滑块顶可移动范围 [0, 轨道 - 滑块] 线性映射到
/// [0, base_y]。`grab_offset`：按下时指针在滑块内的位置，拖动中不变，滑块才会贴着指针而不是把顶端跳到指针处。
pub fn drag_to_line(track_top: f32, track_h: f32, slider: f32, grab_offset: f32, pointer_y: f32, base_y: usize) -> usize {
    let range = track_h - slider;
    if range <= 0.0 || base_y == 0 {
        return 0;
    }
    let pos = (pointer_y - track_top - grab_offset).clamp(0.0, range);
    ((pos / range) * base_y as f32 + 0.5).floor() as usize
}

/// 边沿触发的活动计时：ping → 显形；停手 IDLE_MS 后淡出（FADE_MS 线性）。时间由调用方传（毫秒）
#[derive(Default, Clone, Copy, Debug)]
pub struct Activity {
    last_ping: Option<f64>,
}

impl Activity {
    /// 返回 true = 这一下是从隐藏变成显形（边沿）
    pub fn ping(&mut self, now: f64) -> bool {
        let edge = !self.active(now);
        self.last_ping = Some(now);
        edge
    }
    /// 还在「显形」期（不含淡出）
    pub fn active(&self, now: f64) -> bool {
        self.last_ping.map(|t| now - t < IDLE_MS).unwrap_or(false)
    }
    /// 当前透明度：显形期 1，淡出期线性降到 0
    pub fn opacity(&self, now: f64) -> f32 {
        let Some(t) = self.last_ping else { return 0.0 };
        let since = now - t - IDLE_MS;
        if since <= 0.0 {
            1.0
        } else if since >= FADE_MS {
            0.0
        } else {
            (1.0 - since / FADE_MS) as f32
        }
    }
    /// 什么时候需要下一帧（淡出还没完就要继续画）
    pub fn animating(&self, now: f64) -> bool {
        self.last_ping.map(|t| now - t < IDLE_MS + FADE_MS).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_scroll_and_jump_latest() {
        assert!(!is_user_scroll(120, 120), "贴底（输出跟随）不算");
        assert!(is_user_scroll(80, 120), "离开底部算");
        assert!(!is_user_scroll(0, 0), "没有回滚历史不算");
        assert!(is_user_scroll(0, 500), "翻到最顶算");
        // scripts/test-jump-latest.ts
        assert!(!should_show_jump_latest(120, 120), "贴底：不显示");
        assert!(should_show_jump_latest(119, 120), "往回翻了一行：显示");
        assert!(should_show_jump_latest(0, 500), "翻到最顶：显示");
        assert!(!should_show_jump_latest(0, 0), "没有回滚历史：不显示");
        assert!(!should_show_jump_latest(200, 200), "贴底时来了新输出（两者一起涨）：不显示");
    }

    #[test]
    fn activity_edges() {
        let mut a = Activity::default();
        assert!(!a.active(0.0));
        assert!(a.ping(0.0), "第一次 ping 立刻 show");
        assert!(!a.ping(100.0), "连续 ping 不重复 show");
        assert!(!a.ping(200.0));
        assert!(a.active(1099.0), "最后一次 ping 后不到 900ms 不 hide");
        assert!(!a.active(1100.0), "满 900ms 后 hide");
        assert!((a.opacity(1100.0) - 1.0).abs() < 1e-3, "刚开始淡出时还是不透明");
        assert!((a.opacity(1250.0) - 0.5).abs() < 1e-3, "淡出一半");
        assert!(a.animating(1250.0));
        assert_eq!(a.opacity(1400.0), 0.0, "淡出完");
        assert!(!a.animating(5000.0));
        assert!(a.ping(6000.0), "hide 之后再 ping 能重新 show");
        assert_eq!(a.opacity(6000.0), 1.0);
    }

    #[test]
    fn cap_slider_vectors() {
        // 轨道 400，上限 1/4 = 100，最小 20（向量照搬 scripts/test-terminal-scrollbar.ts）
        let q = MAX_SLIDER_RATIO;
        assert_eq!(cap_slider(400.0, 60.0, 170.0, q), (60.0, 170.0), "本来就短于上限：原样");
        assert_eq!(cap_slider(400.0, 100.0, 150.0, q), (100.0, 150.0), "等于上限：原样");
        assert_eq!(cap_slider(400.0, 320.0, 0.0, q), (100.0, 0.0), "超上限、在顶：截到 100 贴顶");
        assert_eq!(cap_slider(400.0, 320.0, 80.0, q), (100.0, 300.0), "超上限、在底：截到 100 贴底");
        assert_eq!(cap_slider(400.0, 320.0, 40.0, q), (100.0, 150.0), "超上限、居中：按比例居中");
        assert_eq!(cap_slider(400.0, 400.0, 0.0, q), (100.0, 0.0), "整条轨道（不可滚）：截短但贴顶，不除零");
        assert_eq!(cap_slider(60.0, 50.0, 5.0, q), (20.0, 20.0), "轨道很短：上限低于最小值时取最小值 20");
        assert_eq!(cap_slider(0.0, 0.0, 0.0, q), (0.0, 0.0), "轨道为 0（未布局）：原样");
        assert_eq!(cap_slider(400.0, 320.0, 80.0, 0.5), (200.0, 200.0), "自定义上限 1/2");
    }

    #[test]
    fn xterm_slider_geometry() {
        // 可见 40 行、总 4040 行、轨道 400：长度 floor(40/4040×400)=3 → 夹到最小 20
        assert_eq!(xterm_slider(400.0, 40, 4040, 0), (20.0, 0.0), "在最顶");
        assert_eq!(xterm_slider(400.0, 40, 4040, 4000), (20.0, 380.0), "在最底");
        assert_eq!(xterm_slider(400.0, 40, 80, 20), (200.0, 100.0), "一半历史、居中");
        assert_eq!(xterm_slider(400.0, 40, 40, 0), (400.0, 0.0), "没有历史：整条");
    }

    #[test]
    fn gutter() {
        assert!(is_in_scrollbar_gutter(800.0, 800.0), "正好在右边缘：在");
        assert!(is_in_scrollbar_gutter(800.0 - 13.0, 800.0), "离右边缘 13px：在");
        assert!(!is_in_scrollbar_gutter(800.0 - 14.0, 800.0), "离右边缘 14px：不在（刚好在界外）");
        assert!(!is_in_scrollbar_gutter(400.0, 800.0), "终端中间：不在");
        assert!(!is_in_scrollbar_gutter(820.0, 800.0), "指针跑到终端右侧之外：不在");
    }

    #[test]
    fn drag_follows_pointer() {
        // 轨道顶 100、高 400，滑块封顶后 100 高，可移动 300；base_y 3000
        let d = |grab: f32, y: f32| drag_to_line(100.0, 400.0, 100.0, grab, y, 3000);
        assert_eq!(d(50.0, 100.0 + 150.0 + 50.0), 1500, "按下不动：还在原来那行");
        assert_eq!(d(50.0, 900.0), 3000, "拖到轨道最下面之外：夹到最后一行");
        assert_eq!(d(50.0, 0.0), 0, "拖到轨道上面之外：夹到第 0 行");
        assert_eq!(drag_to_line(100.0, 400.0, 100.0, 10.0, 300.0, 0), 0, "没有可滚内容：恒为 0");
        assert_eq!(drag_to_line(100.0, 400.0, 400.0, 10.0, 300.0, 3000), 0, "滑块和轨道一样长：0，不除以零");
        // 滑块贴着指针：拖到任意位置后，按这行重新算出来的（封顶后的）滑块顶 = 指针 - 偏移，误差不超过 1px
        let grab = 30.0;
        for top in (100..=400).step_by(7) {
            let line = drag_to_line(100.0, 400.0, 100.0, grab, top as f32 + grab, 3000);
            let (size, t) = cap_slider(400.0, 160.0, line as f32 / 3000.0 * (400.0 - 160.0), MAX_SLIDER_RATIO);
            assert_eq!(size, 100.0);
            assert!((100.0 + t - top as f32).abs() <= 1.0, "滑块顶 {} 应贴着 {}", 100.0 + t, top);
        }
    }
}
