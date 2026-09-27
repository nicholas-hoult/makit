//! 会话目录监听。

// fs.watch 增量推送：
// - sessions/ 变化 → emit "running-changed"（只需重读运行状态，轻量）
// - projects/ 变化 → emit "sessions-changed"，**载荷是变化的 jsonl 路径列表**。
//   这里以前发的是空载荷、注释写"低频，只在新 session 创建时触发"，但 projects/ 是
//   递归监听：每追加一条消息都会触发。前端因此只能把它接成空函数，新建会话要 ⌘R
//   才出现（#168）。带上路径之后前端可以只解析这几个文件（list_sessions_by_paths）。
pub fn start_session_watcher(app_handle: tauri::AppHandle) {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use tauri::Emitter;
    use std::time::Duration;

    let home = match dirs::home_dir() { Some(h) => h, None => return };
    let sessions_dir = home.join(".claude").join("sessions");
    let projects_dir = home.join(".claude").join("projects");
    let codex_sessions_dir = home.join(".codex").join("sessions");

    let _ = std::fs::create_dir_all(&sessions_dir);
    let _ = std::fs::create_dir_all(&projects_dir);

    let sessions_dir_clone = sessions_dir.clone();

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
                let mut has_session_event = false;
                let mut changed_jsonl: Vec<String> = Vec::new();
                for ev in &events {
                    if ev.path.starts_with(&sessions_dir_clone) {
                        has_session_event = true;
                    } else if ev.path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        // 只报 jsonl：目录创建、锁文件之类的事件对前端没有信息量。
                        // 新 session 一定会连带写出自己的 jsonl，不会漏。
                        changed_jsonl.push(ev.path.to_string_lossy().into_owned());
                    }
                }
                // sessions/ 变化 = running 状态更新（轻量：前端只刷 running 状态）
                if has_session_event {
                    let _ = app_handle.emit("running-changed", ());
                }
                // projects/ 变化 = 某个 session 的内容变了（新建 / 新消息 / 改名）
                if !changed_jsonl.is_empty() {
                    changed_jsonl.sort();
                    changed_jsonl.dedup();
                    let _ = app_handle.emit("sessions-changed", changed_jsonl);
                }
            }
        }
    });
}
