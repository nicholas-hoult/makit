//! 本机会话接口的开关和写入桥（#299）：设置里的「允许本机其他程序读取会话」和「允许从手机回话和批准」。
//! 两个都默认关；开关即时生效，退出时关掉并清理 socket。
//!
//! 接口本身在 `makit_core::link`（协议、安全判定）；这里管三件事：
//! 1. 什么时候开、什么时候关（[`decide`]）；
//! 2. 要碰界面的请求怎么回到 GPUI 主线程：socket 线程把 [`MainRequest`] 发进通道，主线程处理后回执（[`attach_window`]）。
//!    包括写终端、取屏幕快照、新开会话标签、续接会话；
//! 3. 会话列表和对话直接用核心的 `CoreBackend`，不经主线程。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use gpui::{App, Entity, WindowHandle};
use makit_core::link::{Backend, CoreBackend, Cursor, Handle, Screen, SessionInfo, TranscriptPage};

use crate::state::{AppEvent, AppState};

/// 偏好和实际状态对比后该做什么。
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Keep,
}

/// 纯判断：想开而没开 → 开；不想开而开着 → 关；其余不动。
/// 为什么单独测：开关错一次，要么用户关了却还在监听（以为安全其实没关），要么开了却没起来（别的程序连不上）。
pub fn decide(enabled: bool, running: bool) -> Action {
    match (enabled, running) {
        (true, false) => Action::Start,
        (false, true) => Action::Stop,
        _ => Action::Keep,
    }
}

/// 要主线程做的事。
#[derive(Debug, PartialEq)]
pub enum MainOp {
    /// 往会话的终端写字节
    Write { session_id: String, bytes: Vec<u8> },
    /// 会话所在终端的屏幕快照
    Screen { session_id: String },
    /// 在目录里开新标签启动工具（已经过 `plan_open` 检查）
    Open { cwd: String, tool: String },
    /// 续接已结束的会话
    Resume { session_id: String },
}

/// 主线程的回执。
#[derive(Debug, PartialEq)]
pub enum MainReply {
    Done,
    Screen(Screen),
}

/// 一次请求：socket 线程发给主线程，`reply` 收回执。
pub struct MainRequest {
    pub op: MainOp,
    pub reply: std_mpsc::Sender<Result<MainReply, String>>,
}

/// 等主线程回执的上限。主线程在忙（比如正在大量输出）时也不该让手机一直转圈。
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);

/// 接口用的后端：读取交给核心，写入经通道交给主线程。
pub struct NativeBackend {
    core: CoreBackend,
    write_on: Arc<AtomicBool>,
    tx: UnboundedSender<MainRequest>,
    timeout: Duration,
}

impl NativeBackend {
    pub fn new(write_on: Arc<AtomicBool>, tx: UnboundedSender<MainRequest>) -> Self {
        Self { core: CoreBackend::new(), write_on, tx, timeout: WRITE_TIMEOUT }
    }

    fn ask(&self, op: MainOp) -> Result<MainReply, String> {
        let (reply, wait) = std_mpsc::channel();
        self.tx.unbounded_send(MainRequest { op, reply }).map_err(|_| "makit is shutting down".to_string())?;
        wait.recv_timeout(self.timeout).map_err(|_| "makit did not respond".to_string())?
    }

    fn ask_done(&self, op: MainOp) -> Result<(), String> {
        match self.ask(op)? {
            MainReply::Done => Ok(()),
            MainReply::Screen(_) => Err("unexpected reply".into()),
        }
    }
}

impl Backend for NativeBackend {
    fn sessions(&self, limit: usize) -> Result<Vec<SessionInfo>, String> {
        self.core.sessions(limit)
    }

    fn transcript(&self, session_id: &str, cursor: Cursor) -> Result<TranscriptPage, String> {
        self.core.transcript(session_id, cursor)
    }

    fn write_enabled(&self) -> bool {
        self.write_on.load(Ordering::SeqCst)
    }

    fn session_state(&self, session_id: &str) -> Option<(String, String)> {
        self.core.session_state(session_id)
    }

    fn write(&self, session_id: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.ask_done(MainOp::Write { session_id: session_id.to_string(), bytes })
    }

    fn screen(&self, session_id: &str) -> Result<Screen, String> {
        match self.ask(MainOp::Screen { session_id: session_id.to_string() })? {
            MainReply::Screen(s) => Ok(s),
            MainReply::Done => Err("unexpected reply".into()),
        }
    }

    fn open(&self, cwd: &str, tool: &str) -> Result<(), String> {
        self.ask_done(MainOp::Open { cwd: cwd.to_string(), tool: tool.to_string() })
    }

    fn resume(&self, session_id: &str) -> Result<(), String> {
        self.ask_done(MainOp::Resume { session_id: session_id.to_string() })
    }
}

/// 启动时按偏好决定开不开，并跟着偏好变化即时开关；退出时关掉。在 `AppState::init` 之后调。
/// 返回写入请求的接收端，窗口建好后交给 [`attach_window`]。
pub fn init(state: &Entity<AppState>, cx: &mut App) -> UnboundedReceiver<MainRequest> {
    let (tx, rx) = unbounded::<MainRequest>();
    let write_on = Arc::new(AtomicBool::new(false));
    let running: Arc<Mutex<Option<Handle>>> = Arc::new(Mutex::new(None));
    let sync = {
        let running = running.clone();
        let write_on = write_on.clone();
        move |enabled: bool, write: bool| {
            // 只读开关关着时写入一定不可用；接口停了也就没人能调
            write_on.store(enabled && write, Ordering::SeqCst);
            let mut slot = running.lock().expect("link switch lock");
            match decide(enabled, slot.is_some()) {
                Action::Start => *slot = makit_core::link::start(Arc::new(NativeBackend::new(write_on.clone(), tx.clone()))),
                Action::Stop => {
                    if let Some(h) = slot.take() {
                        h.stop();
                    }
                }
                Action::Keep => {}
            }
        }
    };
    {
        let p = &state.read(cx).prefs;
        sync(p.link_enabled, p.link_write_enabled);
    }
    let on_prefs = sync.clone();
    cx.subscribe(state, move |state, ev: &AppEvent, cx| {
        if matches!(ev, AppEvent::PrefsChanged) {
            let p = &state.read(cx).prefs;
            on_prefs(p.link_enabled, p.link_write_enabled);
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        if let Some(h) = running.lock().expect("link switch lock").take() {
            h.stop();
        }
        async {}
    })
    .detach();
    rx
}

/// 窗口建好后接上主线程请求：写终端、取屏幕、开新会话、续接会话，把结果回给 socket 线程。
pub fn attach_window(handle: WindowHandle<crate::app::Root>, mut rx: UnboundedReceiver<MainRequest>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(req) = rx.next().await {
            let op = req.op;
            let result = handle
                .update(cx, move |root, _, cx| run(root, op, cx))
                .unwrap_or_else(|_| Err("makit window is closed".into()));
            // 发请求的一方可能已经超时走了，回不去也无所谓
            let _ = req.reply.send(result);
        }
    })
    .detach();
}

fn run(root: &mut crate::app::Root, op: MainOp, cx: &mut gpui::Context<crate::app::Root>) -> Result<MainReply, String> {
    let ws = root.workspace_view();
    match op {
        MainOp::Write { session_id, bytes } => ws.update(cx, |w, cx| w.write_to_session(&session_id, bytes, cx)).map(|_| MainReply::Done),
        MainOp::Screen { session_id } => ws.update(cx, |w, cx| w.screen_of_session(&session_id, cx)).map(MainReply::Screen),
        MainOp::Open { cwd, tool } => {
            let state = AppState::global(cx);
            state.update(cx, |s, cx| {
                s.workspace.open_new_session(&cwd, makit_core::ai_provider::tool_arg(&tool));
                s.workspace_changed(cx);
            });
            crate::overlays::show_toast(crate::ts!("link.toast.remote_input"), crate::overlays::toast::REMOTE_INPUT, cx);
            Ok(MainReply::Done)
        }
        MainOp::Resume { session_id } => {
            if AppState::global(cx).read(cx).session(&session_id).is_none() {
                return Err("session not found".into());
            }
            crate::overlays::session_ops::open_session(&session_id, cx);
            Ok(MainReply::Done)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_starts_stops_or_keeps() {
        assert_eq!(decide(true, false), Action::Start);
        assert_eq!(decide(false, true), Action::Stop);
        assert_eq!(decide(true, true), Action::Keep);
        assert_eq!(decide(false, false), Action::Keep);
    }

    #[test]
    fn link_is_off_by_default_and_old_state_files_load_with_it_off() {
        let d = crate::persist::NativeState::default();
        assert!(!d.link_enabled);
        assert!(!d.link_write_enabled);
        // 旧版本的状态文件没有这两项：按关处理，不能因为缺字段而读失败
        let old: crate::persist::NativeState = serde_json::from_str(r#"{"version":1,"reflow_view":true}"#).unwrap();
        assert!(!old.link_enabled && !old.link_write_enabled);
        assert!(old.reflow_view);
        // 只开了读取的旧状态：写入仍是关
        let read_only: crate::persist::NativeState = serde_json::from_str(r#"{"version":1,"link_enabled":true}"#).unwrap();
        assert!(read_only.link_enabled && !read_only.link_write_enabled);
    }

    // 为什么单独测：写入要穿过线程边界。回执丢了，手机上是一直转圈；回执错了，是「已发送」但终端里什么都没有。
    fn backend(timeout_ms: u64) -> (NativeBackend, UnboundedReceiver<MainRequest>) {
        let (tx, rx) = unbounded();
        let mut b = NativeBackend::new(Arc::new(AtomicBool::new(true)), tx);
        b.timeout = Duration::from_millis(timeout_ms);
        (b, rx)
    }

    #[test]
    fn write_waits_for_the_main_thread_and_returns_its_answer() {
        let (b, mut rx) = backend(2000);
        let main = std::thread::spawn(move || {
            let req = futures::executor::block_on(rx.next()).expect("a request");
            assert_eq!(req.op, MainOp::Write { session_id: "s1".into(), bytes: b"\r".to_vec() });
            req.reply.send(Ok(MainReply::Done)).unwrap();
            let req = futures::executor::block_on(rx.next()).expect("a second request");
            req.reply.send(Err("session not open in makit".into())).unwrap();
        });
        assert_eq!(b.write("s1", b"\r".to_vec()), Ok(()));
        assert_eq!(b.write("s2", b"x".to_vec()), Err("session not open in makit".into()));
        main.join().unwrap();
    }

    #[test]
    fn write_fails_fast_when_nobody_receives_and_times_out_when_nobody_answers() {
        let (b, rx) = backend(50);
        // 主线程没回执：超时，不能一直挂着
        assert_eq!(b.write("s1", vec![1]), Err("makit did not respond".into()));
        // 接收端没了（makit 在退出）：立刻失败
        drop(rx);
        assert_eq!(b.write("s1", vec![1]), Err("makit is shutting down".into()));
    }

    #[test]
    fn write_flag_follows_the_switch() {
        let (tx, _rx) = unbounded();
        let on = Arc::new(AtomicBool::new(false));
        let b = NativeBackend::new(on.clone(), tx);
        assert!(!b.write_enabled());
        on.store(true, Ordering::SeqCst);
        assert!(b.write_enabled());
    }

    #[test]
    fn screen_open_and_resume_reach_the_main_thread_with_their_arguments() {
        let (b, mut rx) = backend(2000);
        let screen = Screen { cols: 80, rows: 2, cursor_x: 1, cursor_y: 0, lines: vec!["hi".into(), String::new()] };
        let expected = screen.clone();
        let main = std::thread::spawn(move || {
            let next = |rx: &mut UnboundedReceiver<MainRequest>| futures::executor::block_on(rx.next()).expect("a request");
            let req = next(&mut rx);
            assert_eq!(req.op, MainOp::Screen { session_id: "s1".into() });
            req.reply.send(Ok(MainReply::Screen(screen))).unwrap();
            let req = next(&mut rx);
            assert_eq!(req.op, MainOp::Open { cwd: "/Users/me/p".into(), tool: "codex".into() });
            req.reply.send(Ok(MainReply::Done)).unwrap();
            let req = next(&mut rx);
            assert_eq!(req.op, MainOp::Resume { session_id: "s9".into() });
            req.reply.send(Err("session not found".into())).unwrap();
            // A reply of the wrong kind is an error, not a silent success
            let req = next(&mut rx);
            req.reply.send(Ok(MainReply::Done)).unwrap();
        });
        assert_eq!(b.screen("s1"), Ok(expected));
        assert_eq!(b.open("/Users/me/p", "codex"), Ok(()));
        assert_eq!(b.resume("s9"), Err("session not found".into()));
        assert_eq!(b.screen("s1"), Err("unexpected reply".into()));
        main.join().unwrap();
    }
}
