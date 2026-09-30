//! 一次性取证（#203）：alacritty 改列数时的重排耗时随回滚行数怎么长。
//!
//! 背景：SCROLLBACK=2000 这个值是在 **xterm.js** 上测出来的（回滚 5000 行约 21ms/次、
//! 2000 行约 8ms，见 src/resizePlan.ts），GPUI 版照搬了常量但没在 alacritty 上重测过
//! —— size.rs 的注释自己写着「加大要重新测量」。这里就测那一下。
//!
//! 跑：cargo run --release --manifest-path native/Cargo.toml --example reflow_cost

use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::term::{test::TermSize, Config, Term};
use alacritty_terminal::vte::ansi::Processor;

#[derive(Clone)]
struct Noop;
impl EventListener for Noop {
    fn send_event(&self, _: Event) {}
}

fn rss_kb() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

fn main() {
    // 拖动时列数在这两个值之间来回，和真实拖宽度一致
    const WIDE: u16 = 120;
    const NARROW: u16 = 80;
    const ROWS: u16 = 40;

    println!("回滚行数 | 单次重排 ms | 单个终端占用 MB | 20 面板合计 MB");
    println!("---|---|---|---");
    for &lines in &[200usize, 1000, 2000, 5000, 10000, 20000] {
        let before = rss_kb();
        let mut term = Term::new(
            Config { scrolling_history: lines, ..Config::default() },
            &TermSize::new(WIDE as usize, ROWS as usize),
            Noop,
        );
        // 填满回滚：每行写满到接近 WIDE，这样换到 NARROW 一定要折行（重排最贵的情况）
        let mut parser: Processor<alacritty_terminal::vte::ansi::StdSyncHandler> = Processor::new();
        let filler: String = (0..(WIDE as usize - 10)).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        for i in 0..(lines + ROWS as usize) {
            let line = format!("{i:06} {filler}\r\n");
            for b in line.as_bytes() {
                parser.advance(&mut term, &[*b]);
            }
        }
        let mem_mb = (rss_kb().saturating_sub(before)) as f64 / 1024.0;

        // 量六次 resize 取均值。窄→宽、宽→窄都要折，来回各算
        let mut total = std::time::Duration::ZERO;
        let n = 6;
        for i in 0..n {
            let cols = if i % 2 == 0 { NARROW } else { WIDE };
            let t = Instant::now();
            term.resize(TermSize::new(cols as usize, ROWS as usize));
            total += t.elapsed();
        }
        let avg_ms = total.as_secs_f64() * 1000.0 / n as f64;
        println!("{lines} | {avg_ms:.2} | {mem_mb:.1} | {:.0}", mem_mb * 20.0);
        drop(term);
    }
}
