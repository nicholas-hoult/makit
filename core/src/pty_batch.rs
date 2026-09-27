//! PTY 输出合并（#222）。
//!
//! macOS 的 PTY 在程序逐行写的时候，读端一次只拿到几十字节：`seq 1 300000`（2.3MB）
//! 实测平均每次 24–36 字节、7.8 万次读。以前每读一次就 emit 一个 Tauri 事件，7.8 万个
//! 事件把 WebView 主线程挤满，9 秒里只画出 3 帧 —— 用户看到的「开始输出卡一下」其实是
//! 整段输出期间界面冻住。
//!
//! 策略（节流，带前沿）：
//! - 空闲后来的第一块**立刻**发 —— 打字回显不加任何延迟
//! - 之后连续输出时，距上次发送不足 `interval` 就先攒着，到点一起发
//! - 攒够 `max_bytes` 立刻发，不等到点（控制单个事件大小）
//! - 只发完整的 UTF-8 字符，被切断的字节留到下一次（中文 3 字节可能跨两次读）

use std::time::{Duration, Instant};

pub const EMIT_INTERVAL: Duration = Duration::from_millis(8);
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
/// 在途事件上限（#229）：× MAX_BATCH_BYTES 就是最坏积压量，1MB 以内 xterm 半秒处理完
pub const MAX_INFLIGHT: usize = 4;
/// 这么久收不到确认就放行，防止前端卡死 / 丢消息时终端永远不出字
pub const ACK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
pub enum Next {
    /// 现在就发
    EmitNow,
    /// 等到这个时刻再发（期间继续收）
    WaitUntil(Instant),
    /// 没有可发的完整字符，等下一块数据
    Idle,
}

pub struct Batcher {
    interval: Duration,
    max_bytes: usize,
    last_emit: Option<Instant>,
    pending: Vec<u8>,
}

impl Batcher {
    pub fn new(interval: Duration, max_bytes: usize) -> Self {
        Self { interval, max_bytes, last_emit: None, pending: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// 已经收到的字节里，完整 UTF-8 字符的长度
    fn emittable(&self) -> usize {
        match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) => e.valid_up_to(),
        }
    }

    pub fn next(&self, now: Instant) -> Next {
        let ready = self.emittable();
        if ready == 0 {
            return Next::Idle;
        }
        if ready >= self.max_bytes {
            return Next::EmitNow;
        }
        match self.last_emit {
            Some(last) if now < last + self.interval => Next::WaitUntil(last + self.interval),
            _ => Next::EmitNow,
        }
    }

    /// 取出能发的部分（完整字符），记下发送时刻；被切断的尾巴留着
    pub fn take(&mut self, now: Instant) -> String {
        let ready = self.emittable();
        if ready == 0 {
            return String::new();
        }
        let rest = self.pending.split_off(ready);
        let bytes = std::mem::replace(&mut self.pending, rest);
        self.last_emit = Some(now);
        // emittable 已经验证过这段是完整的 UTF-8
        String::from_utf8(bytes).unwrap_or_default()
    }

    /// 结束时把剩下的全部交出去（残缺字节按替换字符处理，不丢）
    pub fn finish(&mut self) -> String {
        let bytes = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// 已发给前端、前端还没确认处理完的事件数（#229 背压）。
///
/// 以前发送端不管前端有没有消费：`yes` 刷屏时事件在 WebView 主线程上无限积压，
/// 30 秒内存涨到 3.3GB，Ctrl-C 的 pty_write 排队 124 秒。现在在途事件到上限就停发；
/// 发送端一停，读→发之间的有界通道很快就满，读线程阻塞，PTY 缓冲区满后程序自己被卡住。
/// 前端在 `terminal.write` 回调里 `pty_ack`。长时间收不到确认（前端卡死 / 丢消息）就放行，
/// 宁可多发一点也不能把终端永远卡死。
pub struct Inflight {
    count: usize,
    max: usize,
    timeout: Duration,
    stalled_since: Option<Instant>,
}

impl Inflight {
    pub fn new(max: usize, timeout: Duration) -> Self {
        Self { count: 0, max, timeout, stalled_since: None }
    }
    /// 现在能不能再发一个
    pub fn can_emit(&mut self, now: Instant) -> bool {
        if self.count < self.max {
            self.stalled_since = None;
            return true;
        }
        let since = *self.stalled_since.get_or_insert(now);
        if now.duration_since(since) >= self.timeout {
            // 等太久了：多半是前端卡死或确认丢了，清零放行
            self.count = 0;
            self.stalled_since = None;
            return true;
        }
        false
    }
    pub fn on_emit(&mut self) {
        self.count += 1;
    }
    pub fn on_ack(&mut self, n: usize) {
        self.count = self.count.saturating_sub(n);
    }
}

/// 发不发、什么时候发出了错，界面上的样子是：打字有延迟（前沿没做好）、刷屏又冻住
/// （合并没生效）、中文变乱码或丢字（UTF-8 切断处理错）。
#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn first_chunk_after_idle_goes_out_immediately() {
        let t0 = Instant::now();
        let mut b = Batcher::new(ms(8), 1 << 20);
        assert_eq!(b.next(t0), Next::Idle, "没数据时不发");
        b.push(b"a");
        assert_eq!(b.next(t0), Next::EmitNow, "空闲后的第一块（比如打字回显）立刻发");
        assert_eq!(b.take(t0), "a");
        assert_eq!(b.next(t0), Next::Idle);
    }

    #[test]
    fn burst_within_interval_is_held_until_the_tick() {
        let t0 = Instant::now();
        let mut b = Batcher::new(ms(8), 1 << 20);
        b.push(b"1\n");
        b.take(t0);
        b.push(b"2\n");
        b.push(b"3\n");
        assert_eq!(b.next(t0 + ms(3)), Next::WaitUntil(t0 + ms(8)), "上次发送 8ms 内的先攒着");
        assert_eq!(b.next(t0 + ms(8)), Next::EmitNow, "到点一起发");
        assert_eq!(b.take(t0 + ms(8)), "2\n3\n");
    }

    #[test]
    fn full_batch_goes_out_without_waiting() {
        let t0 = Instant::now();
        let mut b = Batcher::new(ms(8), 4);
        b.push(b"x");
        b.take(t0);
        b.push(b"abcd");
        assert_eq!(b.next(t0 + ms(1)), Next::EmitNow, "攒够上限就发，不等到点");
    }

    #[test]
    fn split_utf8_char_waits_for_its_tail() {
        let t0 = Instant::now();
        let mut b = Batcher::new(ms(8), 1 << 20);
        let zh = "中".as_bytes(); // 3 字节
        b.push(&zh[..2]);
        assert_eq!(b.next(t0), Next::Idle, "只有半个字，没有可发的");
        b.push(&zh[2..]);
        assert_eq!(b.next(t0), Next::EmitNow);
        assert_eq!(b.take(t0), "中");
        b.push("好a".as_bytes());
        b.push(&"文".as_bytes()[..1]);
        assert_eq!(b.take(t0 + ms(8)), "好a", "完整的先发，切断的尾巴留下");
        b.push(&"文".as_bytes()[1..]);
        assert_eq!(b.take(t0 + ms(16)), "文");
    }

    #[test]
    fn inflight_blocks_at_limit_and_ack_releases() {
        let t0 = Instant::now();
        let mut f = Inflight::new(2, ms(3000));
        assert!(f.can_emit(t0));
        f.on_emit();
        assert!(f.can_emit(t0));
        f.on_emit();
        assert!(!f.can_emit(t0), "在途 2 个到上限，停发");
        f.on_ack(1);
        assert!(f.can_emit(t0), "前端确认一个，放行");
    }

    #[test]
    fn inflight_releases_after_timeout_without_ack() {
        let t0 = Instant::now();
        let mut f = Inflight::new(1, ms(3000));
        f.on_emit();
        assert!(!f.can_emit(t0));
        assert!(!f.can_emit(t0 + ms(2999)), "超时前继续等");
        assert!(f.can_emit(t0 + ms(3000)), "3 秒收不到确认就放行，不能把终端永远卡死");
        f.on_emit();
        assert!(!f.can_emit(t0 + ms(3001)), "放行后重新计数，不是从此不限");
    }

    #[test]
    fn extra_acks_do_not_underflow() {
        let t0 = Instant::now();
        let mut f = Inflight::new(1, ms(3000));
        f.on_ack(5);
        assert!(f.can_emit(t0));
        f.on_emit();
        assert!(!f.can_emit(t0), "多余的确认不能攒成「额度」");
    }

    #[test]
    fn finish_hands_over_everything() {
        let mut b = Batcher::new(ms(8), 1 << 20);
        b.push(b"end");
        b.push(&"中".as_bytes()[..1]);
        assert_eq!(b.finish(), "end\u{FFFD}", "进程退出时残缺字节不丢，用替换字符");
        assert_eq!(b.finish(), "");
    }
}
