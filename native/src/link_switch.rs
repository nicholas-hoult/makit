//! 本机会话接口的开关和写入桥（#299）：设置里的「允许本机其他程序读取会话」和「允许从手机回话和批准」。
//! 两个都默认关；开关即时生效，退出时关掉并清理 socket。
//!
//! 接口本身在 `makit_core::link`（协议、安全判定）；这里管三件事：
//! 1. 什么时候开、什么时候关（[`decide`]）；
//! 2. 要碰界面的请求怎么回到 GPUI 主线程：socket 线程把 [`MainRequest`] 发进通道，主线程处理后回执（[`attach_window`]）。
//!    包括写终端、取屏幕快照、新开会话标签、续接会话；
//! 3. 会话列表和对话直接用核心的 `CoreBackend`，不经主线程。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use gpui::{App, Entity, WindowHandle};
use makit_core::link::{AfterRestart, Backend, CoreBackend, Cursor, Handle, OpenError, Screen, SessionInfo, TranscriptPage, UpdateStatus};

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
    /// 在目录里开新标签启动工具（已经过 `plan_open` / `validate_open_text` 检查），`text` 是首条消息；`trust`/
    /// `permission_mode` 见 #D-2168（只对 claude 生效，调用前 `handle_line` 已经把关过 tool）
    Open { cwd: String, tool: String, text: Option<String>, trust: bool, permission_mode: Option<String> },
    /// 续接已结束的会话；标签复用策略是调用方的（#D-2138/#D-2140）：`allow_new_tab` 为假且没有标签可复用时不新开，返回结构化拒绝；
    /// `permission_mode` 见 #D-2168（只在真的新开标签时才用得上，复用已有标签时静默忽略）
    Resume { session_id: String, allow_new_tab: bool, permission_mode: Option<String> },
    /// 停止会话的进程、标签保留（同侍栏「停止会话」，#D-2299）；运行中/等待批准中默认拒绝，`force:true` 才执行
    Stop { session_id: String, force: bool },
    /// 真正关掉这个会话的标签（同侍栏「关闭」，#D-2300）；拒绝规则同 `Stop`
    Close { session_id: String, force: bool },
    /// 本机会话接口的 `update check`（#D-2122）：当前已知的更新状态，不是强制联网重查
    UpdateCheck,
    /// 本机会话接口的 `update install`：装当前已发现的新版本并重启；`after_restart` 是重启后要接上的那个会话
    UpdateInstall { after_restart: Option<AfterRestart> },
}

/// 主线程的回执。
#[derive(Debug, PartialEq)]
pub enum MainReply {
    Done,
    Screen(Screen),
    UpdateStatus(UpdateStatus),
    /// `resume` succeeded: which branch it actually took (#D-2138).
    Resumed(makit_core::link::ResumeAction),
}

/// 一次请求：socket 线程发给主线程，`reply` 收回执。错误用 `OpenError`（不是普通 `String`）：`open`/`resume`
/// 被容量上限拦住时要带上结构化的 `capacity_limit`（#D-2099），`Write`/`Screen` 走同一个通道，错误原样包成
/// `OpenError::Other`，靠 `From<OpenError> for String` 在各自的 `Backend` 方法里自动降级回普通消息
pub struct MainRequest {
    pub op: MainOp,
    pub reply: std_mpsc::Sender<Result<MainReply, OpenError>>,
}

/// 等主线程回执的上限。主线程在忙（比如正在大量输出）时也不该让手机一直转圈。
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);

/// 接口用的后端：读取交给核心，写入经通道交给主线程。
pub struct NativeBackend {
    core: CoreBackend,
    write_on: Arc<AtomicBool>,
    tx: UnboundedSender<MainRequest>,
    timeout: Duration,
    /// 当前每个会话标签的状态（"open"/"stopped"，没有标签就不在这个 map 里，#D-2143 的 `has_tab`、#D-2300 的
    /// `tab_state` 都从这读）；主线程跟着 `WorkspaceChanged` 更新，这里只读，不过主线程
    tab_states: Arc<Mutex<HashMap<String, &'static str>>>,
}

impl NativeBackend {
    pub fn new(write_on: Arc<AtomicBool>, tx: UnboundedSender<MainRequest>, tab_states: Arc<Mutex<HashMap<String, &'static str>>>) -> Self {
        Self { core: CoreBackend::new(), write_on, tx, timeout: WRITE_TIMEOUT, tab_states }
    }

    fn ask(&self, op: MainOp) -> Result<MainReply, OpenError> {
        let (reply, wait) = std_mpsc::channel();
        self.tx.unbounded_send(MainRequest { op, reply }).map_err(|_| OpenError::from("makit is shutting down"))?;
        wait.recv_timeout(self.timeout).map_err(|_| OpenError::from("makit did not respond"))?
    }

    fn ask_done(&self, op: MainOp) -> Result<(), OpenError> {
        match self.ask(op)? {
            MainReply::Done => Ok(()),
            MainReply::Screen(_) | MainReply::UpdateStatus(_) | MainReply::Resumed(_) => Err("unexpected reply".into()),
        }
    }
}

impl Backend for NativeBackend {
    fn sessions(&self, limit: usize) -> Result<Vec<SessionInfo>, String> {
        let mut list = self.core.sessions(limit)?;
        let states = self.tab_states.lock().unwrap();
        for s in &mut list {
            let state = states.get(&s.session_id).copied();
            s.has_tab = state.is_some();
            s.tab_state = state.unwrap_or("none").to_string();
        }
        Ok(list)
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

    /// #D-2188: must not inherit the default (`self.sessions(MAX_LIMIT)`, which `CoreBackend::sessions` filters
    /// to non-archived and caps) -- `resume` on an archived-but-still-on-disk session needs this to say it
    /// exists, same as `CoreBackend::session_exists` itself already correctly does by looking the file up on
    /// disk directly.
    fn session_exists(&self, session_id: &str) -> bool {
        self.core.session_exists(session_id)
    }

    fn write(&self, session_id: &str, bytes: Vec<u8>) -> Result<(), String> {
        Ok(self.ask_done(MainOp::Write { session_id: session_id.to_string(), bytes })?)
    }

    fn screen(&self, session_id: &str) -> Result<Screen, String> {
        match self.ask(MainOp::Screen { session_id: session_id.to_string() })? {
            MainReply::Screen(s) => Ok(s),
            MainReply::Done | MainReply::UpdateStatus(_) | MainReply::Resumed(_) => Err("unexpected reply".into()),
        }
    }

    fn open(&self, cwd: &str, tool: &str, text: Option<&str>, trust: bool, permission_mode: Option<&str>) -> Result<(), OpenError> {
        self.ask_done(MainOp::Open { cwd: cwd.to_string(), tool: tool.to_string(), text: text.map(String::from), trust, permission_mode: permission_mode.map(String::from) })
    }

    fn resume(&self, session_id: &str, allow_new_tab: bool, permission_mode: Option<&str>) -> Result<makit_core::link::ResumeAction, OpenError> {
        match self.ask(MainOp::Resume { session_id: session_id.to_string(), allow_new_tab, permission_mode: permission_mode.map(String::from) })? {
            MainReply::Resumed(action) => Ok(action),
            MainReply::Done | MainReply::Screen(_) | MainReply::UpdateStatus(_) => Err("unexpected reply".into()),
        }
    }

    fn stop(&self, session_id: &str, force: bool) -> Result<(), OpenError> {
        self.ask_done(MainOp::Stop { session_id: session_id.to_string(), force })
    }

    fn close(&self, session_id: &str, force: bool) -> Result<(), OpenError> {
        self.ask_done(MainOp::Close { session_id: session_id.to_string(), force })
    }

    fn update_check(&self) -> Result<UpdateStatus, String> {
        match self.ask(MainOp::UpdateCheck)? {
            MainReply::UpdateStatus(s) => Ok(s),
            MainReply::Done | MainReply::Screen(_) | MainReply::Resumed(_) => Err("unexpected reply".into()),
        }
    }

    fn update_install(&self, after_restart: Option<AfterRestart>) -> Result<(), String> {
        Ok(self.ask_done(MainOp::UpdateInstall { after_restart })?)
    }

    fn archive(&self, session_id: &str) -> Result<(), String> {
        self.core.archive(session_id)
    }
}

/// 启动时按偏好决定开不开，并跟着偏好变化即时开关；退出时关掉。在 `AppState::init` 之后调。
/// 返回写入请求的接收端，窗口建好后交给 [`attach_window`]。
pub fn init(state: &Entity<AppState>, cx: &mut App) -> UnboundedReceiver<MainRequest> {
    let (tx, rx) = unbounded::<MainRequest>();
    let write_on = Arc::new(AtomicBool::new(false));
    let running: Arc<Mutex<Option<Handle>>> = Arc::new(Mutex::new(None));
    let tab_states: Arc<Mutex<HashMap<String, &'static str>>> = Arc::new(Mutex::new(crate::state::binding::tab_states(&state.read(cx).workspace.state.root)));
    let sync = {
        let running = running.clone();
        let write_on = write_on.clone();
        let tab_states = tab_states.clone();
        move |enabled: bool, write: bool| {
            // 只读开关关着时写入一定不可用；接口停了也就没人能调
            write_on.store(enabled && write, Ordering::SeqCst);
            let mut slot = running.lock().expect("link switch lock");
            match decide(enabled, slot.is_some()) {
                Action::Start => *slot = makit_core::link::start(Arc::new(NativeBackend::new(write_on.clone(), tx.clone(), tab_states.clone()))),
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
    cx.subscribe(state, move |state, ev: &AppEvent, cx| match ev {
        AppEvent::PrefsChanged => {
            let p = &state.read(cx).prefs;
            on_prefs(p.link_enabled, p.link_write_enabled);
        }
        // `sessions`' `has_tab` / `tab_state` (#D-2143/#D-2300) read this without hopping to the main thread --
        // keep it current whenever the workspace changes instead
        AppEvent::WorkspaceChanged => {
            *tab_states.lock().unwrap() = crate::state::binding::tab_states(&state.read(cx).workspace.state.root);
        }
        AppEvent::SessionsChanged => {}
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
                .update(cx, move |root, window, cx| run(root, op, window, cx))
                .unwrap_or_else(|_| Err("makit window is closed".into()));
            // 发请求的一方可能已经超时走了，回不去也无所谓
            let _ = req.reply.send(result);
        }
    })
    .detach();
}

/// Forces `tab_id`'s terminal to actually spawn now (not the next render pass): `Ok` only once a process is
/// really running, `Err` with a reason otherwise (#D-2118 -- see `WorkspaceView::force_spawn_now`'s own doc for
/// why this step cannot be skipped).
fn confirm_spawned(ws: &Entity<crate::workspace::WorkspaceView>, tab_id: &str, window: &mut gpui::Window, cx: &mut gpui::App, what: &str) -> Result<(), OpenError> {
    if ws.update(cx, |w, cx| w.force_spawn_now(tab_id, window, cx)) {
        Ok(())
    } else {
        Err(format!("{what} did not start a process").into())
    }
}

/// #D-2168: canonicalizes `cwd` (resolves `..` and symlinks -- the same form Claude Code's own `process.cwd()`
/// ends up recording in `~/.claude.json`, since POSIX `getcwd()` is already symlink-free) and refuses unless it
/// lands under one of the user's own configured auto-trust roots (also canonicalized), *before* writing
/// anything. Only then does it mark that directory trusted via [`makit_core::claude_config::trust_directory`].
fn try_trust_directory(cwd: &str, cx: &gpui::App) -> Result<(), OpenError> {
    let canonical_cwd = std::fs::canonicalize(cwd).map_err(|e| OpenError::from(format!("cwd: {e}")))?;
    let roots: Vec<std::path::PathBuf> =
        AppState::global(cx).read(cx).prefs.trust_roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).collect();
    if !makit_core::claude_config::is_under_trusted_root(&canonical_cwd, &roots) {
        return Err(OpenError::not_trusted_root("cwd is not under any configured auto-trust root"));
    }
    makit_core::claude_config::trust_directory(&canonical_cwd.to_string_lossy()).map_err(OpenError::from)
}

fn run(root: &mut crate::app::Root, op: MainOp, window: &mut gpui::Window, cx: &mut gpui::Context<crate::app::Root>) -> Result<MainReply, OpenError> {
    let ws = root.workspace_view();
    match op {
        MainOp::Write { session_id, bytes } => ws.update(cx, |w, cx| w.write_to_session(&session_id, bytes, cx)).map(|_| MainReply::Done).map_err(OpenError::from),
        MainOp::Screen { session_id } => ws.update(cx, |w, cx| w.screen_of_session(&session_id, cx)).map(MainReply::Screen).map_err(OpenError::from),
        MainOp::Open { cwd, tool, text, trust, permission_mode } => {
            if let Some(e) = crate::overlays::session_ops::capacity_block(cx) {
                return Err(e);
            }
            if trust {
                try_trust_directory(&cwd, cx)?;
            }
            let state = AppState::global(cx);
            let tab_id = state.update(cx, |s, cx| {
                let tid = s.workspace.open_new_session(&cwd, makit_core::ai_provider::tool_arg(&tool), text.as_deref(), permission_mode.as_deref());
                s.workspace_changed(cx);
                tid
            });
            let result = confirm_spawned(&ws, &tab_id, window, cx, "open");
            if result.is_ok() {
                crate::overlays::show_toast(crate::ts!("link.toast.remote_input"), crate::overlays::toast::REMOTE_INPUT, cx);
            }
            result.map(|_| MainReply::Done)
        }
        MainOp::Resume { session_id, allow_new_tab, permission_mode } => {
            if AppState::global(cx).read(cx).session(&session_id).is_none() {
                return Err("session not found".into());
            }
            if let Some(e) = crate::overlays::session_ops::capacity_block(cx) {
                return Err(e);
            }
            // The tab-reuse-vs-open-new policy is the caller's, not makit's (#D-2138/#D-2140): `resume_session`
            // itself refuses with `OpenError::no_tab` when `allow_new_tab` is false and there is nothing to
            // reuse, without opening anything.
            let (tab_id, action) = crate::overlays::session_ops::resume_session(&session_id, allow_new_tab, permission_mode.as_deref(), cx)?;
            confirm_spawned(&ws, &tab_id, window, cx, "resume").map(|_| MainReply::Resumed(action))
        }
        MainOp::Stop { session_id, force } => crate::overlays::session_ops::stop_session_for_link(&session_id, force, cx).map(|_| MainReply::Done),
        MainOp::Close { session_id, force } => crate::overlays::session_ops::close_session_for_link(&session_id, force, cx).map(|_| MainReply::Done),
        MainOp::UpdateCheck => {
            let s = crate::update::status(cx);
            let (state, latest, notes, page_url) = match &s {
                crate::update::Status::Idle => ("idle", None, String::new(), String::new()),
                crate::update::Status::Checking => ("checking", None, String::new(), String::new()),
                crate::update::Status::Available(r) => ("available", Some(r.version.clone()), r.notes.clone(), r.page_url.clone()),
                crate::update::Status::Installing(r) => ("installing", Some(r.version.clone()), r.notes.clone(), r.page_url.clone()),
            };
            // 这次回复报的是现在已知的状态；没有在查 / 在装就顺手起一次新的检查，结果留给对方下次 check 再看
            crate::update::check_now(false, cx);
            Ok(MainReply::UpdateStatus(UpdateStatus { current: crate::update::current_version().to_string(), state: state.to_string(), latest, notes, page_url }))
        }
        MainOp::UpdateInstall { after_restart } => {
            let Some(release) = crate::update::available(cx) else {
                return Err("no update available, call update check first".into());
            };
            match crate::update::apply::plan(&release, crate::app::is_dev_instance()) {
                crate::update::apply::Plan::InPlace(_) => {
                    if let Some(ar) = &after_restart {
                        crate::update::after_restart::stash(&crate::update::after_restart::Continuation { session_id: ar.session_id.clone(), text: ar.text.clone() });
                    }
                    crate::update::install(release, cx);
                    Ok(MainReply::Done)
                }
                _ => Err("this machine cannot update in place (homebrew-managed install or a dev instance): use the in-app update pill instead".into()),
            }
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

    /// #D-2188: `Backend::session_exists`'s default implementation goes through `self.sessions(MAX_LIMIT)` --
    /// which `CoreBackend::sessions` filters to non-archived sessions and caps at `MAX_LIMIT` -- so it is wrong
    /// for any backend whose `sessions()` can leave sessions out for a reason that has nothing to do with
    /// whether they still exist (archived, or simply past the cap). `CoreBackend` itself gets this right
    /// (`session_exists` looks the file up on disk directly, ignoring `archived` and any cap) but
    /// `NativeBackend` wraps one and must not silently fall back to the broken default: it has to override
    /// `session_exists` to defer to its own `core`'s correct one. A missing override here is exactly how this
    /// bug slipped in once already (real repro: a session archived via the sidebar, still on disk, made
    /// `resume` reply `{"ok":false,"error":"session not found"}` on this machine on 2026-11-20) -- scanning the
    /// source for the override, rather than only asserting behavior, is what actually catches "someone deleted
    /// the override" the next time, since a real-disk assertion can't be made deterministic in a unit test.
    #[test]
    fn session_exists_is_overridden_to_defer_to_core_not_the_capped_sessions_list() {
        let src = include_str!("link_switch.rs");
        let impl_start = src.find("impl Backend for NativeBackend").expect("the impl block itself");
        let impl_block = &src[impl_start..];
        let impl_end = impl_block.find("\n}\n").map(|i| i + 1).unwrap_or(impl_block.len());
        assert!(impl_block[..impl_end].contains("fn session_exists"), "NativeBackend must override session_exists, not inherit the default");
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
        let mut b = NativeBackend::new(Arc::new(AtomicBool::new(true)), tx, Arc::new(Mutex::new(HashMap::new())));
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
        let b = NativeBackend::new(on.clone(), tx, Arc::new(Mutex::new(HashMap::new())));
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
            assert_eq!(req.op, MainOp::Open { cwd: "/Users/me/p".into(), tool: "codex".into(), text: None, trust: false, permission_mode: None });
            req.reply.send(Ok(MainReply::Done)).unwrap();
            let req = next(&mut rx);
            assert_eq!(
                req.op,
                MainOp::Open { cwd: "/Users/me/p".into(), tool: "claude".into(), text: Some("hi there".into()), trust: true, permission_mode: Some("acceptEdits".into()) }
            );
            req.reply.send(Ok(MainReply::Done)).unwrap();
            let req = next(&mut rx);
            assert_eq!(req.op, MainOp::Resume { session_id: "s9".into(), allow_new_tab: false, permission_mode: None });
            req.reply.send(Err("session not found".into())).unwrap();
            let req = next(&mut rx);
            assert_eq!(req.op, MainOp::Resume { session_id: "s3".into(), allow_new_tab: true, permission_mode: Some("acceptEdits".into()) });
            req.reply.send(Ok(MainReply::Resumed(makit_core::link::ResumeAction::OpenedNewTab))).unwrap();
            // A reply of the wrong kind is an error, not a silent success
            let req = next(&mut rx);
            req.reply.send(Ok(MainReply::Done)).unwrap();
        });
        assert_eq!(b.screen("s1"), Ok(expected));
        assert_eq!(b.open("/Users/me/p", "codex", None, false, None), Ok(()));
        assert_eq!(b.open("/Users/me/p", "claude", Some("hi there"), true, Some("acceptEdits")), Ok(()));
        assert_eq!(b.resume("s9", false, None), Err("session not found".into()));
        assert_eq!(
            b.resume("s3", true, Some("acceptEdits")),
            Ok(makit_core::link::ResumeAction::OpenedNewTab),
            "allow_new_tab and permission_mode reach the main thread and the Resumed reply carries the action back"
        );
        assert_eq!(b.screen("s1"), Err("unexpected reply".into()));
        main.join().unwrap();
    }
}
