//! 本机会话接口的开关和写入桥（#299）：设置里的「允许本机其他程序读取会话」和「允许从手机回话和批准」。
//! 两个都默认关；开关即时生效，退出时关掉并清理 socket。
//!
//! 接口本身在 `makit_core::link`（协议、安全判定）；这里管三件事：
//! 1. 什么时候开、什么时候关（[`decide`]）；
//! 2. 写入怎么回到 GPUI 主线程：socket 线程把 [`WriteRequest`] 发进通道，主线程按会话找到终端、写 PTY、回执（[`attach_window`]）；
//! 3. 读取直接用核心的 `CoreBackend`，不经主线程。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use gpui::{App, Entity, WindowHandle};
use makit_core::link::{Backend, CoreBackend, Cursor, Handle, SessionInfo, TranscriptPage};

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

/// 一次写入请求：socket 线程发给主线程，`reply` 收回执。
pub struct WriteRequest {
    pub session_id: String,
    pub bytes: Vec<u8>,
    pub reply: std_mpsc::Sender<Result<(), String>>,
}

/// 等主线程回执的上限。主线程在忙（比如正在大量输出）时也不该让手机一直转圈。
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);

/// 接口用的后端：读取交给核心，写入经通道交给主线程。
pub struct NativeBackend {
    core: CoreBackend,
    write_on: Arc<AtomicBool>,
    tx: UnboundedSender<WriteRequest>,
    timeout: Duration,
}

impl NativeBackend {
    pub fn new(write_on: Arc<AtomicBool>, tx: UnboundedSender<WriteRequest>) -> Self {
        Self { core: CoreBackend::new(), write_on, tx, timeout: WRITE_TIMEOUT }
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
        let (reply, wait) = std_mpsc::channel();
        self.tx.unbounded_send(WriteRequest { session_id: session_id.to_string(), bytes, reply }).map_err(|_| "makit is shutting down".to_string())?;
        wait.recv_timeout(self.timeout).map_err(|_| "makit did not respond".to_string())?
    }
}

/// 启动时按偏好决定开不开，并跟着偏好变化即时开关；退出时关掉。在 `AppState::init` 之后调。
/// 返回写入请求的接收端，窗口建好后交给 [`attach_window`]。
pub fn init(state: &Entity<AppState>, cx: &mut App) -> UnboundedReceiver<WriteRequest> {
    let (tx, rx) = unbounded::<WriteRequest>();
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

/// 窗口建好后接上写入：主线程收到请求，找到绑着该会话的终端写进去，把结果回给 socket 线程。
pub fn attach_window(handle: WindowHandle<crate::app::Root>, mut rx: UnboundedReceiver<WriteRequest>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(req) = rx.next().await {
            let result = handle
                .update(cx, |root, _, cx| root.workspace_view().update(cx, |ws, cx| ws.write_to_session(&req.session_id, req.bytes.clone(), cx)))
                .unwrap_or_else(|_| Err("makit window is closed".into()));
            // 发请求的一方可能已经超时走了，回不去也无所谓
            let _ = req.reply.send(result);
        }
    })
    .detach();
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
    fn backend(timeout_ms: u64) -> (NativeBackend, UnboundedReceiver<WriteRequest>) {
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
            assert_eq!((req.session_id.as_str(), req.bytes.as_slice()), ("s1", &b"\r"[..]));
            req.reply.send(Ok(())).unwrap();
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
}
