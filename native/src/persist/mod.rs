//! 持久化：`~/.claude/makit/native-state.json`（替代 Tauri 版的 localStorage）。
//!
//! - 形状在 `state.rs`（`NativeState`）。加字段见那里的规矩。
//! - 写盘：`Saver` 后台线程防抖 500ms（和 TS 版 `makit-workspace` 的 debounce 一样），
//!   先写 `.tmp` 再 rename（写到一半被杀也不会留下半个文件）；退出前 `flush()`。
//! - 首次启动（文件不存在）从 Tauri 版的 WebKit localStorage 导入一次（`webkit.rs`，只读），
//!   导入失败 / 没找到都不影响启动，照样写出一份默认状态，之后不再导。
//!
//! 谁来调：只有 `state::AppState` 读写它。各功能包要持久化新东西 = 在 `NativeState` 里加字段，
//! 改完调 `AppState::save_prefs`（见 state/mod.rs）。

pub mod state;
pub mod webkit;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

pub use state::NativeState;

pub const DEBOUNCE: Duration = Duration::from_millis(500);

pub fn state_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("native-state.json"))
}

/// 读状态文件。不存在返回 None（= 首次启动）；坏了把它改名成 `.bad` 留证据，按默认状态继续。
pub fn load_from(path: &Path) -> Option<NativeState> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<NativeState>(&bytes) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[persist] {} 解析失败（{e}），按默认状态启动，原文件改名 .bad", path.display());
            let _ = std::fs::rename(path, path.with_extension("json.bad"));
            Some(NativeState::default())
        }
    }
}

/// 原子写：先写临时文件再 rename
pub fn save_to(path: &Path, state: &NativeState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

/// 启动时读状态：文件在就用它；不在就尝试从 WebKit localStorage 导入（只这一次），然后立刻写盘。
/// `webkit_root` 是 `~/Library/WebKit/com.hoult.makit`（测试可注入）。
pub fn load_or_import(path: &Path, webkit_root: Option<&Path>) -> NativeState {
    if let Some(s) = load_from(path) {
        return s;
    }
    let state = match webkit_root.and_then(webkit::import_from) {
        Some((s, from)) => {
            eprintln!("[persist] 首次启动，已从 {from} 导入 Tauri 版的设置");
            s
        }
        None => NativeState::default(),
    };
    if let Err(e) = save_to(path, &state) {
        eprintln!("[persist] 写 {} 失败：{e}", path.display());
    }
    state
}

enum Msg {
    Save(Box<NativeState>),
    Flush(mpsc::Sender<()>),
}

/// 防抖写盘。`save` 只是把最新快照交给后台线程，500ms 内没有新快照才真写；`flush` 立刻写掉并等写完。
#[derive(Clone)]
pub struct Saver {
    tx: Sender<Msg>,
}

impl Saver {
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new()
            .name("makit-persist".into())
            .spawn(move || {
                let mut pending: Option<Box<NativeState>> = None;
                loop {
                    let msg = if pending.is_some() { rx.recv_timeout(DEBOUNCE) } else { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) };
                    match msg {
                        Ok(Msg::Save(s)) => pending = Some(s),
                        Ok(Msg::Flush(done)) => {
                            if let Some(s) = pending.take() {
                                write_logged(&path, &s);
                            }
                            let _ = done.send(());
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some(s) = pending.take() {
                                write_logged(&path, &s);
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            if let Some(s) = pending.take() {
                                write_logged(&path, &s);
                            }
                            break;
                        }
                    }
                }
            })
            .expect("persist 线程起不来");
        Self { tx }
    }

    pub fn save(&self, state: NativeState) {
        let _ = self.tx.send(Msg::Save(Box::new(state)));
    }

    /// 退出前调：把还在防抖窗口里的快照立刻写掉（最多等 2 秒）
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

fn write_logged(path: &Path, s: &NativeState) {
    if let Err(e) = save_to(path, s) {
        eprintln!("[persist] 写 {} 失败：{e}", path.display());
    }
}

/// 防抖、原子写、首启导入只一次：错了是「重启后布局 / 置顶 / 主题没了」或「每次启动都重新导入覆盖掉新设置」。
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("makit-persist-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn saver_debounces_and_flush_writes_latest() {
        let d = tmp("debounce");
        let p = d.join("native-state.json");
        let saver = Saver::new(p.clone());
        for i in 0..5 {
            let mut s = NativeState::default();
            s.pinned_sessions = vec![format!("v{i}")];
            saver.save(s);
        }
        assert!(!p.exists(), "防抖窗口内不写");
        let t = Instant::now();
        while !p.exists() && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(load_from(&p).unwrap().pinned_sessions, ["v4"], "只写最后一份");
        let mut s = NativeState::default();
        s.pinned_sessions = vec!["flushed".into()];
        saver.save(s);
        saver.flush();
        assert_eq!(load_from(&p).unwrap().pinned_sessions, ["flushed"], "flush 立刻写");
        assert!(!p.with_extension("json.tmp").exists(), "临时文件不留");
    }

    #[test]
    fn corrupt_file_is_kept_aside_and_defaults_used() {
        let d = tmp("corrupt");
        let p = d.join("native-state.json");
        std::fs::write(&p, "{坏").unwrap();
        assert_eq!(load_from(&p), Some(NativeState::default()));
        assert!(p.with_extension("json.bad").exists());
    }

    #[test]
    fn first_launch_imports_once_then_never_again() {
        let d = tmp("import");
        let p = d.join("native-state.json");
        let wk = d.join("WebKit");
        webkit::tests::make_localstorage(&wk, &[("makit-theme", "dracula"), ("makit-pinned-sessions", "[\"x\"]")]);
        let s = load_or_import(&p, Some(&wk));
        assert_eq!(s.theme.id, "dracula");
        assert_eq!(s.pinned_sessions, ["x"]);
        assert!(s.imported_from.is_some());
        assert!(p.exists(), "导入后立刻写盘");
        // 用户在 GPUI 版里改了主题；再启动时不能被 localStorage 覆盖回去
        let mut changed = s.clone();
        changed.theme.id = "nord".into();
        save_to(&p, &changed).unwrap();
        assert_eq!(load_or_import(&p, Some(&wk)).theme.id, "nord");
    }

    #[test]
    fn import_failure_does_not_block_startup() {
        let d = tmp("noimport");
        let p = d.join("native-state.json");
        let s = load_or_import(&p, Some(&d.join("不存在")));
        assert_eq!(s, NativeState::default());
        assert!(p.exists());
    }
}
