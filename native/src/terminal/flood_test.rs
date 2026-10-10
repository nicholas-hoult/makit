//! 输出背压的回归测试（Tauri 版 #229：没有背压时 `yes` 刷屏内存无限涨、Ctrl-C 排队 2 分钟）。
//!
//! GPUI 版的读线程是 alacritty 的 EventLoop：读多少解析多少（解析在同一个线程里、持 Term 锁），解析跟不上就不再读，
//! 内核 PTY 缓冲一满 `yes` 就阻塞在 write 上。所以理论上天然有背压 —— 这里用真 PTY + 真 `yes` 量一遍，
//! 不靠推理：刷 3 秒，看进程内存涨了多少、Ctrl-C 之后多久子进程退出。
//! 界面线程不参与（事件通道由一个线程照常抽干），量的是「解析 + 缓冲」这条链本身。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacEvent, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::unbounded;

use super::{Listener, TermSize};

fn rss_kb(pid: u32) -> u64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

#[test]
fn yes_flood_has_backpressure_and_ctrl_c_is_fast() {
    let (tx, mut rx) = unbounded();
    let term = Arc::new(FairMutex::new(Term::new(
        Config { scrolling_history: super::size::SCROLLBACK, ..Config::default() },
        &TermSize { cols: 120, rows: 40 },
        Listener(tx.clone()),
    )));
    // 事件通道由一个线程抽干（界面线程的角色）；记下子进程退出的时刻和 Wakeup 次数
    let exited_at: Arc<Mutex<Option<Instant>>> = Arc::default();
    let wakeups = Arc::new(Mutex::new(0u64));
    let (e2, w2) = (exited_at.clone(), wakeups.clone());
    let drain = std::thread::spawn(move || loop {
        match rx.try_recv() {
            Ok(AlacEvent::Wakeup) => *w2.lock().unwrap() += 1,
            Ok(AlacEvent::ChildExit(_) | AlacEvent::Exit) => {
                e2.lock().unwrap().get_or_insert(Instant::now());
            }
            Ok(_) => {}
            Err(e) if e.is_closed() => break,
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
        if e2.lock().unwrap().is_some() {
            break;
        }
    });
    // exec yes：yes 自己就是会话首进程 / 前台进程组，^C 经行规程变成 SIGINT 直接打到它
    let options = tty::Options {
        shell: Some(tty::Shell::new("/bin/sh".into(), vec!["-c".into(), "exec yes".into()])),
        working_directory: None,
        drain_on_exit: false,
        env: Default::default(),
    };
    let ws = WindowSize { num_lines: 40, num_cols: 120, cell_width: 8, cell_height: 16 };
    let pty = tty::new(&options, ws, 0).expect("PTY");
    let pid = pty.child().id();
    let el = EventLoop::new(term.clone(), Listener(tx), pty, false, false).expect("event loop");
    let notifier = Notifier(el.channel());
    let _io = el.spawn();

    let me = std::process::id();
    std::thread::sleep(Duration::from_millis(500));
    let rss0 = rss_kb(me);
    std::thread::sleep(Duration::from_secs(3));
    let rss1 = rss_kb(me);
    let sent = Instant::now();
    notifier.notify(&b"\x03"[..]);
    // 子进程退出的时刻（debug 构建里解析慢，内核缓冲里剩下的那点输出要先解析完，退出事件才轮得到）
    let deadline = Instant::now() + Duration::from_secs(10);
    while exited_at.lock().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    let latency = exited_at.lock().unwrap().map(|t| t.duration_since(sent));
    let _ = notifier.0.send(Msg::Shutdown);
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    let _ = drain.join();
    let grew_mb = (rss1 as f64 - rss0 as f64) / 1024.0;
    eprintln!(
        "[背压] yes 刷 3 秒：进程 RSS {:.1}MB → {:.1}MB（涨 {grew_mb:.1}MB），Wakeup {} 次；Ctrl-C → 子进程退出 {:?}",
        rss0 as f64 / 1024.0,
        rss1 as f64 / 1024.0,
        wakeups.lock().unwrap(),
        latency
    );
    let latency = latency.expect("Ctrl-C 之后 10 秒内子进程没退出 —— 输入被输出饿死了（#229）");
    assert!(latency < Duration::from_secs(1), "Ctrl-C 到退出 {latency:?}，应远小于 1 秒");
    assert!(grew_mb < 64.0, "刷 3 秒内存涨了 {grew_mb:.1}MB —— 没有背压");
}
