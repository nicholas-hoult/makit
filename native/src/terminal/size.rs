//! 终端尺寸：几何、行列分离节流、PTY 尺寸记账（#111 / #156 / #203）。
//! 对应 TS 版 `src/resizePlan.ts` + `src/ptySize.ts`，测试向量照搬 `scripts/test-resize-plan.ts`、`test-pty-size.ts`。
//!
//! 不变量（对齐清单 1 / 2 / 4 / 5 / 7）：
//! - **尺寸只有一份账**：`PtyLedger` 里的尺寸必须恒等于终端网格当前的 cols×rows；判定和记账是同一个动作；
//!   发送失败退账，且只在账上还是自己那一笔时才退；spawn 成功按 spawn 尺寸记账。（#156）
//! - **下限两边同夹**：网格和 PTY 同时夹到至少 20×5，绝不能只抬 PTY 一边。（#156）
//! - **不白扣滚动条宽度**：列数 = floor(可用宽 / 格宽)，overlay 滚动条不占列。（#111）
//! - **行列分离**：行数每帧跟；列数在缓冲区 ≥ 200 行时按 33ms 节流跟，停手补最后一次，松手立即做。（#203）

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

pub const fn sz(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

/// 缓冲区小于这个行数时重排很便宜，行列都立即调（VS Code 同值）
pub const SMALL_BUFFER_LINES: usize = 200;
/// 列数跟随的节奏：拖动中每这么久重排一次（对标终端 是 25ms，这里取一帧多一点，实测跟得动）
pub const COLS_FOLLOW_MS: f64 = 33.0;
/// 回滚行数（#203：改列数要把整段回滚按新宽度重排，耗时和回滚行数成正比；加大要重新测量）
pub const SCROLLBACK: usize = 2000;
/// 极窄 pane 的下限。低于这个尺寸的 TUI 会彻底错乱，不如给它一个能用的最小值
pub const MIN_COLS: u16 = 20;
pub const MIN_ROWS: u16 = 5;
/// 不可见守卫（#156）：宽 < 50 或高 < 20 时跳过所有 fit，否则 fit 成极小尺寸再被夹到 20
pub const VISIBLE_MIN_W: f32 = 50.0;
pub const VISIBLE_MIN_H: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub rows_now: bool,
    pub cols_now: bool,
    pub cols_later: bool,
}

/// 这一次改尺寸做什么。行数变了总是立即生效（便宜）；列数变了，小缓冲区或 `immediate`
/// （一次性的改动：松手、缩放字号、最大化、切标签）立即，否则延后。
pub fn plan_resize(current: Size, next: Size, buffer_lines: usize, immediate: bool) -> Plan {
    let rows_changed = next.rows != current.rows;
    let cols_changed = next.cols != current.cols;
    let cols_now = cols_changed && (immediate || buffer_lines < SMALL_BUFFER_LINES);
    Plan { rows_now: rows_changed, cols_now, cols_later: cols_changed && !cols_now }
}

/// 容器能放下多少行列（#111）。公式同 `FitAddon.proposeDimensions()`，只少减一项：不扣滚动条宽度
/// （我们的滚动条是 overlay，不占布局宽度）。下限 2×1（上游 FitAddon 的下限）
pub fn propose_geometry(width: f32, height: f32, cell_w: f32, cell_h: f32) -> Size {
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return sz(2, 1);
    }
    let cols = (width.max(0.0) / cell_w).floor().clamp(2.0, u16::MAX as f32) as u16;
    let rows = (height.max(0.0) / cell_h).floor().clamp(1.0, u16::MAX as f32) as u16;
    sz(cols, rows)
}

/// 把 fit 出来的尺寸夹到下限。调用方必须把网格和 PTY 一起用夹完的这个数
pub fn clamp_size(s: Size) -> Size {
    sz(s.cols.max(MIN_COLS), s.rows.max(MIN_ROWS))
}

/// 这块区域够不够大（不可见 / 祖先隐藏时量出来接近 0，跳过 fit）
pub fn is_visible_area(width: f32, height: f32) -> bool {
    width >= VISIBLE_MIN_W && height >= VISIBLE_MIN_H
}

/// 列数跟随的节流（`createColsFollower` 的纯状态机版）：首次立即执行、之后每 33ms 一次，
/// 停手后补最后一次；`flush`（松手）立即执行并取消待办。时钟由调用方传（毫秒）
#[derive(Clone, Debug)]
pub struct ColsFollower {
    ms: f64,
    last_run: f64,
    /// 排着的「补最后一次」在什么时候到点
    pending_at: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Follow {
    /// 现在就执行
    RunNow,
    /// 已经排着（或刚排上）一次，到 at 时调 `fire`
    Scheduled { at: f64 },
}

impl ColsFollower {
    pub fn new(ms: f64) -> Self {
        Self { ms, last_run: f64::NEG_INFINITY, pending_at: None }
    }
    pub fn request(&mut self, now: f64) -> Follow {
        if now - self.last_run >= self.ms {
            self.pending_at = None;
            self.last_run = now;
            return Follow::RunNow;
        }
        // 已经排着「补最后一次」就不重排
        let at = *self.pending_at.get_or_insert(self.last_run + self.ms);
        Follow::Scheduled { at }
    }
    /// 定时器到点：真的有待办才返回 true（调用方据此执行）
    pub fn fire(&mut self, now: f64) -> bool {
        match self.pending_at {
            Some(at) if now >= at => {
                self.pending_at = None;
                self.last_run = now;
                true
            }
            _ => false,
        }
    }
    /// 松手：有待办就立即执行（返回 true）并取消待办
    pub fn flush(&mut self, now: f64) -> bool {
        if self.pending_at.take().is_none() {
            return false;
        }
        self.last_run = now;
        true
    }
    pub fn pending(&self) -> bool {
        self.pending_at.is_some()
    }
}

/// 这次要不要真的发，以及发之前 PTY 知道的是什么（失败时用它回滚）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub send: bool,
    pub prev: Option<Size>,
}

/// PTY 尺寸记账本 —— **一个 pane 只有一份**（每个终端视图持有一个，终端销毁账就没了 = forgetPane）
#[derive(Default, Clone, Debug)]
pub struct PtyLedger {
    known: Option<Size>,
}

impl PtyLedger {
    /// 终端现在是 `size`，问「需要告诉 PTY 吗」。需要就顺手记账 —— 判定和记账是同一个动作
    pub fn claim(&mut self, size: Size) -> Claim {
        let prev = self.known;
        if prev == Some(size) {
            return Claim { send: false, prev };
        }
        self.known = Some(size);
        Claim { send: true, prev }
    }
    /// 没送到，把账退回去。**只有账上还是我记的那一笔时才退**（迟到的失败回执不许盖掉新的成功记账）
    pub fn revert(&mut self, sent: Size, prev: Option<Size>) {
        if self.known == Some(sent) {
            self.known = prev;
        }
    }
    /// PTY 现在以为自己多大。没告诉过它就是 None
    pub fn known(&self) -> Option<Size> {
        self.known
    }
    /// spawn 成功时按 spawn 尺寸记账（避免第一次 fit 白发一次 SIGWINCH）
    pub fn spawned(&mut self, size: Size) {
        self.known = Some(size);
    }
    pub fn forget(&mut self) {
        self.known = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_vectors() {
        let cur = sz(120, 40);
        let big = 5000;
        let p = |r, c, l| Plan { rows_now: r, cols_now: c, cols_later: l };
        assert_eq!(plan_resize(cur, sz(120, 40), big, false), p(false, false, false), "尺寸没变：什么都不做");
        assert_eq!(plan_resize(cur, sz(120, 30), big, false), p(true, false, false), "只改行数：立即");
        assert_eq!(plan_resize(cur, sz(90, 40), big, false), p(false, false, true), "只改列数（大缓冲区）：延后");
        assert_eq!(plan_resize(cur, sz(90, 30), big, false), p(true, false, true), "行列都改（大缓冲区）：行立即、列延后");
        assert_eq!(plan_resize(cur, sz(90, 30), SMALL_BUFFER_LINES - 1, false), p(true, true, false), "小缓冲区：列也立即");
        assert_eq!(plan_resize(cur, sz(90, 40), big, true), p(false, true, false), "显式立即：列也立即");
        assert_eq!(plan_resize(cur, sz(120, 40), big, true), p(false, false, false), "显式立即但尺寸没变：什么都不做");
    }

    /// 假时钟驱动：request 返回 Scheduled 时记下到点时间，advance 时到点就 fire
    struct Harness {
        f: ColsFollower,
        now: f64,
        timer: Option<f64>,
        runs: u32,
    }
    impl Harness {
        fn request(&mut self) {
            match self.f.request(self.now) {
                Follow::RunNow => self.runs += 1,
                Follow::Scheduled { at } => self.timer = Some(at),
            }
        }
        fn advance(&mut self, ms: f64) {
            self.now += ms;
            if let Some(at) = self.timer {
                if at <= self.now {
                    self.timer = None;
                    if self.f.fire(at) {
                        self.runs += 1;
                    }
                }
            }
        }
        fn flush(&mut self) {
            if self.f.flush(self.now) {
                self.runs += 1;
            }
            self.timer = None;
        }
    }

    #[test]
    fn cols_follower_cadence() {
        let mut h = Harness { f: ColsFollower::new(COLS_FOLLOW_MS), now: 0.0, timer: None, runs: 0 };
        h.request();
        assert_eq!(h.runs, 1, "第一次请求立即跟（起手不延迟）");
        h.advance(16.0);
        h.request();
        assert_eq!(h.runs, 1, "16ms 后再请求：还没到节奏，不跟");
        h.advance(17.0);
        h.request();
        assert_eq!(h.runs, 2, "满 33ms 再跟一次");
        let before = h.runs;
        for _ in 0..30 {
            h.advance(16.0);
            h.request();
        }
        let drag = h.runs - before;
        assert!((8..=15).contains(&drag), "连续拖动按节奏跟随：30 帧重排 {drag} 次");
        let before_idle = h.runs;
        h.advance(COLS_FOLLOW_MS);
        assert_eq!(h.runs, before_idle + 1, "停手后补最后一次（停在哪就折到哪）");
        h.advance(1000.0);
        assert_eq!(h.runs, before_idle + 1, "之后不再重复");
        h.request();
        assert_eq!(h.runs, before_idle + 2, "久未拖动后再拖，第一次立即跟");
        h.advance(5.0);
        h.request();
        h.flush();
        assert_eq!(h.runs, before_idle + 3, "松手 flush 立即兑现待办");
        h.advance(1000.0);
        assert_eq!(h.runs, before_idle + 3, "flush 后待办已取消，不会再跟一次");
        assert!(!h.f.flush(h.now), "没有待办时 flush 什么都不做");
    }

    #[test]
    fn geometry_does_not_reserve_scrollbar() {
        // #111：720px 容器、格宽 8：上游 FitAddon 扣 16px 得 87 列，应为 90 列
        assert_eq!(propose_geometry(720.0, 400.0, 8.0, 16.0), sz(90, 25));
        assert_eq!(propose_geometry(0.0, 0.0, 8.0, 16.0), sz(2, 1), "下限 2×1");
        assert_eq!(propose_geometry(800.0, 400.0, 0.0, 0.0), sz(2, 1), "字体还没量出来时不能除零");
    }

    /// 三条 fit 路径共用一份账（Tauri 版的 doFit / fit / fitAll；这里都走同一个 ledger）
    fn go(l: &mut PtyLedger, pty: &mut Option<Size>, s: Size, ok: bool) {
        let c = l.claim(s);
        if !c.send {
            return;
        }
        if ok {
            *pty = Some(s);
        } else {
            l.revert(s, c.prev);
        }
    }

    #[test]
    fn ledger_root_cause_sequences() {
        // 根因序列：拖分割线 → 切个 tab 回来 → 再拖回原位
        let (mut l, mut pty) = (PtyLedger::default(), None);
        go(&mut l, &mut pty, sz(80, 24), true);
        go(&mut l, &mut pty, sz(100, 30), true);
        go(&mut l, &mut pty, sz(80, 24), true);
        assert_eq!(pty, Some(sz(80, 24)), "同一序列下 PTY 跟上了终端");
        // 发送失败必须回滚
        let (mut l, mut pty) = (PtyLedger::default(), None);
        go(&mut l, &mut pty, sz(80, 24), true);
        go(&mut l, &mut pty, sz(60, 20), false);
        assert_eq!(l.known(), Some(sz(80, 24)), "失败后账退回去了");
        go(&mut l, &mut pty, sz(60, 20), true);
        assert_eq!(pty, Some(sz(60, 20)), "所以下一次同尺寸的 fit 仍然会发");
    }

    #[test]
    fn ledger_dedups_and_forgets() {
        let mut l = PtyLedger::default();
        assert!(l.claim(sz(80, 24)).send, "首次必发");
        assert!(!l.claim(sz(80, 24)).send, "同尺寸不重发");
        assert!(l.claim(sz(81, 24)).send, "只有 cols 变也要发");
        assert!(l.claim(sz(81, 25)).send, "只有 rows 变也要发");
        assert!(!l.claim(sz(81, 25)).send, "再问一次还是不发");
        l.forget();
        assert_eq!(l.known(), None);
        assert!(l.claim(sz(81, 25)).send, "forget 之后首次必发");
        let mut l = PtyLedger::default();
        l.spawned(sz(90, 30));
        assert!(!l.claim(sz(90, 30)).send, "spawn 时按 spawn 尺寸记过账，第一次 fit 不白发");
    }

    #[test]
    fn ledger_all_sequences_hold_invariant() {
        let sizes = [sz(80, 24), sz(100, 30)];
        let mut total = 0;
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    for fail_mid in [false, true] {
                        total += 1;
                        let (mut l, mut pty) = (PtyLedger::default(), None);
                        go(&mut l, &mut pty, sizes[i], true);
                        go(&mut l, &mut pty, sizes[j], !fail_mid);
                        go(&mut l, &mut pty, sizes[k], true);
                        assert_eq!(pty, Some(sizes[k]), "序列 {i}{j}{k} fail_mid={fail_mid}");
                    }
                }
            }
        }
        assert_eq!(total, 16);
    }

    #[test]
    fn late_failure_does_not_overwrite_newer_claim() {
        let mut l = PtyLedger::default();
        let a = l.claim(sz(100, 30)); // A 发 100，之后会失败
        l.claim(sz(80, 24)); // B 发 80，成功
        l.revert(sz(100, 30), a.prev); // A 的失败回执迟到
        assert_eq!(l.known(), Some(sz(80, 24)), "迟到的退账被忽略");
        assert!(!l.claim(sz(80, 24)).send, "所以同尺寸的下一次 fit 仍然被正确去重");
        let c = l.claim(sz(120, 40));
        l.revert(sz(120, 40), c.prev);
        assert_eq!(l.known(), Some(sz(80, 24)), "没人后发时退账生效");
    }

    #[test]
    fn clamp_is_min_20x5_and_idempotent() {
        assert_eq!(clamp_size(sz(80, 24)), sz(80, 24), "正常尺寸不动");
        assert_eq!(clamp_size(sz(7, 24)), sz(20, 24), "过窄抬到 20 列");
        assert_eq!(clamp_size(sz(80, 2)), sz(80, 5), "过矮抬到 5 行");
        assert_eq!(clamp_size(sz(20, 5)), sz(20, 5), "边界值本身不动");
        assert_eq!(clamp_size(clamp_size(sz(3, 1))), clamp_size(sz(3, 1)), "夹两次和夹一次一样");
    }

    #[test]
    fn visibility_guard() {
        assert!(!is_visible_area(0.0, 0.0), "display:none / 还没布局");
        assert!(!is_visible_area(49.0, 400.0));
        assert!(!is_visible_area(400.0, 19.0));
        assert!(is_visible_area(50.0, 20.0));
    }
}
