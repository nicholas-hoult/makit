//! 脉冲（呼吸）动画的统一时钟。
//!
//! 为什么不用 GPUI 的 `with_animation(.. .repeat())`：无限循环的动画每一帧都请求下一帧，窗口按显示器刷新率
//! （60Hz）一直重绘，而每帧要把整个窗口（含终端的全部字形）重新提交一遍。实测（Intel Mac、release、窗口可见）：
//! 只有 1 个 busy 会话的侧栏就占 **26% CPU**（没有 busy 会话时 0.5%），还连带推高 WindowServer，整台机器发卡。
//!
//! 这里改成「按时间取值 + 低频刷新」：
//! - 取值：`opacity(now_ms, period_ms, min)`，平滑三角波，时间按 `STEP_MS` 量化（每秒 5 档，视觉上仍是呼吸）
//! - 刷新：脉冲点渲染时调 `want_ticks()` 声明「我在屏幕上」；全局定时器每 `STEP_MS` 看一眼，有人声明才刷新窗口。
//!   没有任何脉冲点可见时**什么都不做**，CPU 归零。
//!
//! 为什么单独测：取值错了在界面上是「呼吸卡住不动 / 闪得刺眼」；时钟错了是「CPU 又上去了」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::App;

/// 刷新间隔 / 取值量化粒度（毫秒）。200ms = 每秒 5 次重绘，约为原来 60Hz 的 1/12
pub const STEP_MS: u64 = 200;

static WANT: AtomicBool = AtomicBool::new(false);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// 进程内的单调毫秒数（脉冲相位的时间基）
pub fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 透明度：在 1.0 和 `min` 之间，一个周期 `period_ms` 走一个来回（0 → 1.0，半周期 → `min`），
/// 时间先量化到 `STEP_MS` 再取值，所以同一档内多次调用结果相同
pub fn opacity(now_ms: u64, period_ms: u64, min: f32) -> f32 {
    let t = now_ms / STEP_MS * STEP_MS; // 量化到档
    let phase = (t % period_ms.max(1)) as f32 / period_ms.max(1) as f32; // 0..1
    let tri = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 }; // 三角波 0→1→0
    let eased = tri * tri * (3.0 - 2.0 * tri); // smoothstep：两端缓、中间快（近似 CSS ease-in-out）
    1.0 - (1.0 - min) * eased
}

/// 脉冲点在渲染时调：声明「我现在在屏幕上，需要下一次刷新」
pub fn want_ticks() {
    WANT.store(true, Ordering::Relaxed);
}

/// 定时器每档调一次：上一档以来有没有人声明过需要刷新（读完清零）
pub fn take_want() -> bool {
    WANT.swap(false, Ordering::Relaxed)
}

/// 启动全局脉冲时钟（应用启动时调一次）
pub fn start(cx: &mut App) {
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(Duration::from_millis(STEP_MS)).await;
        if take_want() {
            let _ = cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_swings_between_one_and_min_over_one_period() {
        assert_eq!(opacity(0, 2000, 0.4), 1.0, "周期起点不透明");
        let mid = opacity(1000, 2000, 0.4);
        assert!((mid - 0.4).abs() < 1e-4, "半周期最淡：{mid}");
        assert!((opacity(2000, 2000, 0.4) - 1.0).abs() < 1e-4, "走完一周期回到起点");
        for t in (0..4000).step_by(50) {
            let o = opacity(t, 2000, 0.4);
            assert!((0.4 - 1e-4..=1.0 + 1e-4).contains(&o), "t={t} 超出范围：{o}");
        }
    }

    #[test]
    fn values_are_stepped_not_continuous() {
        // 同一档（STEP_MS）内不变 —— 不然重绘频率降下来了取值还在抖，没意义
        assert_eq!(opacity(400, 2000, 0.4), opacity(400 + STEP_MS - 1, 2000, 0.4));
        assert_ne!(opacity(400, 2000, 0.4), opacity(400 + STEP_MS, 2000, 0.4), "换一档就变");
    }

    #[test]
    fn breathing_is_symmetric_around_half_period() {
        for k in 1..5u64 {
            let (a, b) = (opacity(k * STEP_MS, 2000, 0.3), opacity(2000 - k * STEP_MS, 2000, 0.3));
            assert!((a - b).abs() < 1e-4, "档 {k}：前半周期 {a} 和后半周期 {b} 应对称");
        }
    }

    #[test]
    fn shorter_periods_still_work() {
        // 标签点 1.2s 周期：半周期 600ms 正好落在档上
        assert!((opacity(600, 1200, 0.3) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn nobody_wanting_ticks_means_no_refresh() {
        // 全局标志，测试之间会互相影响，所以放在一个用例里按顺序验
        let _ = take_want();
        assert!(!take_want(), "没人声明：不刷新");
        want_ticks();
        assert!(take_want(), "有人声明：刷新一次");
        assert!(!take_want(), "读完清零：不声明就不再刷新");
    }
}
