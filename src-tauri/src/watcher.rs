//! 会话目录监听。
//!
//! 不依赖任何 UI 框架：调用方传一个回调，Tauri 版在回调里 emit 事件给前端，
//! GPUI 版在回调里把事件送进自己的通道。
//!
//! fs.watch 增量推送：
//! - sessions/ 变化 → `RunningChanged`（只需重读运行状态，轻量）
//! - projects/ 变化 → `SessionsChanged(路径)`，**载荷是变化的 jsonl 路径列表**。
//!   这里以前发的是空载荷、注释写"低频，只在新 session 创建时触发"，但 projects/ 是
//!   递归监听：每追加一条消息都会触发。前端因此只能把它接成空函数，新建会话要 ⌘R
//!   才出现（#168）。带上路径之后前端可以只解析这几个文件（list_sessions_by_paths）。

use std::path::Path;

/// 一批（500ms 防抖）文件变化归纳出来的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// `~/.claude/sessions/` 有变化：运行状态 / 改名，调用方重读 `list_running_sessions`
    RunningChanged,
    /// 这些 jsonl 有变化（已排序去重）：调用方用 `list_sessions_by_paths` 增量解析
    SessionsChanged(Vec<String>),
}

/// 把一批变化的路径归纳成事件。同一批里两种都有时，`RunningChanged` 在前（和原来 emit 的顺序一致）。
pub fn classify<'a>(paths: impl IntoIterator<Item = &'a Path>, sessions_dir: &Path) -> Vec<WatchEvent> {
    let mut has_session_event = false;
    let mut changed_jsonl: Vec<String> = Vec::new();
    for path in paths {
        if path.starts_with(sessions_dir) {
            has_session_event = true;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            // 只报 jsonl：目录创建、锁文件之类的事件对前端没有信息量。
            // 新 session 一定会连带写出自己的 jsonl，不会漏。
            changed_jsonl.push(path.to_string_lossy().into_owned());
        }
    }
    let mut out = Vec::new();
    // sessions/ 变化 = running 状态更新（轻量：前端只刷 running 状态）
    if has_session_event {
        out.push(WatchEvent::RunningChanged);
    }
    // projects/ 变化 = 某个 session 的内容变了（新建 / 新消息 / 改名）
    if !changed_jsonl.is_empty() {
        changed_jsonl.sort();
        changed_jsonl.dedup();
        out.push(WatchEvent::SessionsChanged(changed_jsonl));
    }
    out
}

/// 起一个后台线程监听 `~/.claude/sessions`（非递归）、`~/.claude/projects`（递归）、
/// `~/.codex/sessions`（递归，存在时）。500ms 防抖，每批变化按 `classify` 归纳后逐个回调。
/// 线程跟进程同寿，没有停止接口（两边都只在启动时调一次）。
pub fn start(on_event: impl Fn(WatchEvent) + Send + 'static) {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use std::time::Duration;

    let home = match dirs::home_dir() { Some(h) => h, None => return };
    let sessions_dir = home.join(".claude").join("sessions");
    let projects_dir = home.join(".claude").join("projects");
    let codex_sessions_dir = home.join(".codex").join("sessions");

    let _ = std::fs::create_dir_all(&sessions_dir);
    let _ = std::fs::create_dir_all(&projects_dir);

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = match new_debouncer(Duration::from_millis(500), tx) {
            Ok(d) => d,
            Err(e) => { eprintln!("watcher init failed: {}", e); return; }
        };
        let _ = debouncer.watcher().watch(&sessions_dir, RecursiveMode::NonRecursive);
        let _ = debouncer.watcher().watch(&projects_dir, RecursiveMode::Recursive);
        if codex_sessions_dir.exists() {
            let _ = debouncer.watcher().watch(&codex_sessions_dir, RecursiveMode::Recursive);
        }

        for result in rx {
            if let Ok(events) = result {
                for ev in classify(events.iter().map(|e| e.path.as_path()), &sessions_dir) {
                    on_event(ev);
                }
            }
        }
    });
}

/// 回调式改造（#226）之后，「一批变化 → 发什么」这段判断从 emit 里拆了出来，Tauri 和 GPUI 共用。
/// 错了在 UI 上：侧栏运行状态不刷新（RunningChanged 丢了），或新消息 / 新会话要 ⌘R 才出现
/// （jsonl 路径没报上去），或每条消息都触发一次全量重扫（非 jsonl 的噪声也报了上去）。
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_running_and_jsonl_changes() {
        let sessions = PathBuf::from("/Users/me/.claude/sessions");
        let paths = [
            PathBuf::from("/Users/me/.claude/projects/-a/b.jsonl"),
            PathBuf::from("/Users/me/.claude/sessions/123.json"),
            PathBuf::from("/Users/me/.claude/projects/-a/a.jsonl"),
            PathBuf::from("/Users/me/.claude/projects/-a/b.jsonl"),
            PathBuf::from("/Users/me/.claude/projects/-a"),
            PathBuf::from("/Users/me/.claude/projects/-a/x.lock"),
        ];
        let got = classify(paths.iter().map(|p| p.as_path()), &sessions);
        assert_eq!(
            got,
            vec![
                WatchEvent::RunningChanged,
                WatchEvent::SessionsChanged(vec![
                    "/Users/me/.claude/projects/-a/a.jsonl".into(),
                    "/Users/me/.claude/projects/-a/b.jsonl".into(),
                ]),
            ],
            "运行状态在前；jsonl 排序去重；目录和锁文件不报"
        );
    }

    #[test]
    fn nothing_interesting_means_no_event() {
        let sessions = PathBuf::from("/Users/me/.claude/sessions");
        let paths = [PathBuf::from("/Users/me/.claude/projects/-a")];
        assert!(classify(paths.iter().map(|p| p.as_path()), &sessions).is_empty());
        // 只有 sessions/ 下的变化：只报运行状态（sessions/ 下的 .jsonl 也不当会话内容）
        let paths = [PathBuf::from("/Users/me/.claude/sessions/9.jsonl")];
        assert_eq!(classify(paths.iter().map(|p| p.as_path()), &sessions), vec![WatchEvent::RunningChanged]);
    }
}
