//! 持久化：`~/.claude/makit/native-state.json`。
//!
//! - 形状在 `state.rs`（`NativeState`）。加字段见那里的规矩。
//! - 写盘：`Saver` 后台线程防抖 500ms（和 TS 版 `makit-workspace` 的 debounce 一样），
//!   先写 `.tmp` 再 rename（写到一半被杀也不会留下半个文件）；退出前 `flush()`。
//! - 首次启动（文件不存在）写出一份默认状态。
//!
//! 谁来调：只有 `state::AppState` 读写它。各功能包要持久化新东西 = 在 `NativeState` 里加字段，
//! 改完调 `AppState::save_prefs`（见 state/mod.rs）。

pub mod state;

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
            log::warn!(target: "persist", "{} failed to parse ({e}), starting with defaults, original renamed to .bad", path.display());
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
    // The temp name carries the pid and a sequence number: several makit instances (packaged + dev) share this
    // file, and with one fixed temp name the second rename fails with "No such file or directory".
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let result = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 启动时读状态：文件在就用它；不在（首次启动）写出一份默认状态。坏文件的处理见 `load_from`。
pub fn load_or_default(path: &Path) -> NativeState {
    if let Some(s) = load_from(path) {
        return s;
    }
    let state = NativeState::default();
    if let Err(e) = save_to(path, &state) {
        log::warn!(target: "persist", "writing {} failed: {e}", path.display());
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
            .expect("failed to start the persist thread");
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
        log::warn!(target: "persist", "writing {} failed: {e}", path.display());
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
        assert!(std::fs::read_dir(p.parent().unwrap()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")), "临时文件不留");
    }

    /// Several instances (or threads) saving the same state file at once must not fail on a shared temp file.
    #[test]
    fn concurrent_saves_to_the_same_file_all_succeed() {
        let dir = std::env::temp_dir().join(format!("makit-state-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("native-state.json");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || (0..100).map(|_| save_to(&path, &NativeState::default())).filter_map(Result::err).map(|e| e.to_string()).collect::<Vec<_>>())
            })
            .collect();
        let errors: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        assert!(errors.is_empty(), "concurrent saves failed: {:?}", &errors[..errors.len().min(3)]);
        let leftovers = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0, "temp files must not be left behind");
        let _ = std::fs::remove_dir_all(&dir);
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
    fn first_launch_writes_defaults_and_later_launches_read_them() {
        let d = tmp("first");
        let p = d.join("native-state.json");
        assert_eq!(load_or_default(&p), NativeState::default());
        assert!(p.exists(), "首次启动立刻写盘");
        let mut changed = NativeState::default();
        changed.theme.id = "nord".into();
        save_to(&p, &changed).unwrap();
        assert_eq!(load_or_default(&p).theme.id, "nord", "已有文件不能被默认值覆盖");
    }
}
