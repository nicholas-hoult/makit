//! PTY output coalescing (#222).
//!
//! On macOS, when a program writes line by line, the PTY read side gets only a few dozen bytes at a time: for `seq 1 300000` (2.3MB)
//! the measured average was 24-36 bytes per read, 78,000 reads. It used to emit one Tauri event per read, and 78,000
//! events flooded the WebView main thread: only 3 frames were drawn in 9 seconds. What the user saw as "a hitch when output starts" was really
//! the UI being frozen for the whole duration of the output.
//!
//! Strategy (throttling with a leading edge):
//! - The first chunk after idle is sent **immediately**: typing echo gets no added delay
//! - During continuous output afterwards, if less than `interval` has passed since the last send, accumulate and send everything when due
//! - Once `max_bytes` is accumulated, send at once without waiting for the deadline (bounds the size of a single event)
//! - Send only complete UTF-8 characters; a cut-off byte sequence is kept for the next time (a 3-byte Chinese character may span two reads)

use std::time::{Duration, Instant};

pub const EMIT_INTERVAL: Duration = Duration::from_millis(8);
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
/// In-flight event limit (#229): times MAX_BATCH_BYTES this is the worst-case backlog; under 1MB xterm handles it within half a second
pub const MAX_INFLIGHT: usize = 4;
/// If no acknowledgement arrives for this long, let sending proceed, so the terminal never stays blank forever when the frontend hangs / loses messages
pub const ACK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
pub enum Next {
    /// Send now
    EmitNow,
    /// Wait until this moment before sending (keep receiving meanwhile)
    WaitUntil(Instant),
    /// Nothing complete to send; wait for the next chunk of data
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

    /// Length of the complete UTF-8 characters within the bytes received so far
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

    /// Take out the part that can be sent (complete characters) and record the send time; a cut-off tail is kept
    pub fn take(&mut self, now: Instant) -> String {
        let ready = self.emittable();
        if ready == 0 {
            return String::new();
        }
        let rest = self.pending.split_off(ready);
        let bytes = std::mem::replace(&mut self.pending, rest);
        self.last_emit = Some(now);
        // emittable has already verified that this segment is complete UTF-8
        String::from_utf8(bytes).unwrap_or_default()
    }

    /// At the end, hand over everything remaining (an incomplete byte sequence is handled as a replacement character, not dropped)
    pub fn finish(&mut self) -> String {
        let bytes = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Number of events sent to the frontend that the frontend has not yet acknowledged as processed (#229 backpressure).
///
/// The sender used to ignore whether the frontend was consuming: while `yes` flooded the screen, events piled up without limit on the WebView main thread,
/// memory grew to 3.3GB in 30 seconds, and Ctrl-C's pty_write queued for 124 seconds. Now sending stops once in-flight events reach the limit;
/// once the sender stops, the bounded channel between read and send fills up quickly, the read thread blocks, and after the PTY buffer fills the program itself is stalled.
/// The frontend calls `pty_ack` in the `terminal.write` callback. If no acknowledgement arrives for a long time (frontend hung / messages lost), sending proceeds:
/// better to over-send a little than to hang the terminal forever.
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
    /// Whether another one can be sent now
    pub fn can_emit(&mut self, now: Instant) -> bool {
        if self.count < self.max {
            self.stalled_since = None;
            return true;
        }
        let since = *self.stalled_since.get_or_insert(now);
        if now.duration_since(since) >= self.timeout {
            // waited too long: most likely the frontend hung or an acknowledgement was lost; reset to zero and proceed
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

/// What goes wrong on screen if whether / when to send is wrong: typing has latency (leading edge broken), flooding freezes again
/// (coalescing ineffective), Chinese text turns to garbage or drops characters (UTF-8 split handled wrong).
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
        let zh = "中".as_bytes(); // 3 bytes
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
