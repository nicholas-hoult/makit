mod ai_provider;
mod archive;
mod hook_server;
mod paths;
mod perf;
mod process;
mod pty;
mod pty_batch;
mod pty_prep;
mod recovery;
mod running;
mod scan_cache;
mod sessions;
mod watcher;
mod worktree;

pub use sessions::{humanize_duration, get_git_root, ConversationMessage, SessionMeta};
pub use process::ProcessInfo;
pub use running::PtyBinding;

/// #216：Tauri 2 里不带 async 的命令在**主线程**上跑（macOS 上 IPC 回调就在主线程），
/// 命令执行期间窗口、终端、侧栏全部冻住 —— 会话输出时每 1 秒多一次的增量解析
/// （大 jsonl 0.2–0.4s）就是用户感到的「咔咔的」。所以凡是同步 fn 的命令一律标
/// `#[tauri::command]`，放到后台线程执行。新加命令漏了标记，这里会红。
#[cfg(test)]
mod command_thread_tests {
    #[test]
    fn no_sync_command_runs_on_main_thread() {
        let mut bad = Vec::new();
        for (file, src) in [
            ("lib.rs", include_str!("lib.rs")),
            ("pty.rs", include_str!("pty.rs")),
            ("sessions.rs", include_str!("sessions.rs")),
            ("archive.rs", include_str!("archive.rs")),
            ("recovery.rs", include_str!("recovery.rs")),
            ("paths.rs", include_str!("paths.rs")),
            ("running.rs", include_str!("running.rs")),
            ("hook_server.rs", include_str!("hook_server.rs")),
            ("worktree.rs", include_str!("worktree.rs")),
            ("perf.rs", include_str!("perf.rs")),
        ] {
            let lines: Vec<&str> = src.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                if l.trim() != "#[tauri::command]" { continue; }
                let next = lines.get(i + 1).map(|s| s.trim()).unwrap_or("");
                if !next.contains("async fn") {
                    bad.push(format!("{file}:{} {next}", i + 2));
                }
            }
        }
        assert!(bad.is_empty(), "这些同步命令会卡主线程，改成 #[tauri::command(async)]：\n{}", bad.join("\n"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    perf::startup_mark("run 开始");
    let app = tauri::Builder::default()
        .on_page_load(|_w, payload| {
            perf::startup_mark(match payload.event() {
                tauri::webview::PageLoadEvent::Started => "页面开始加载",
                tauri::webview::PageLoadEvent::Finished => "页面加载完成",
            })
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(pty::PtyState::default())
        .setup(|app| {
            perf::startup_mark("窗口已创建");
            // 清理已废弃的 session-index.json（v2 扁平化方案不再使用）
            if let Some(home) = dirs::home_dir() {
                let legacy = home.join(".claude").join("makit").join("session-index.json");
                if legacy.exists() {
                    let _ = std::fs::remove_file(&legacy);
                }
            }
            watcher::start_session_watcher(app.handle().clone());
            hook_server::start(app.handle().clone());

            // 监听主窗口焦点变化，emit 给前端
            use tauri::Manager;
            if let Some(win) = app.handle().get_webview_window("main") {
                let win2 = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(focused) = event {
                        use tauri::Emitter;
                        let _ = win2.emit("window-focus-changed", focused);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sessions::list_sessions,
            sessions::read_session_messages,
            archive::archive_session,
            archive::unarchive_session,
            sessions::find_session_in_cwd_after,
            sessions::read_session_meta,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            pty::kill_pids,
            paths::open_path,
            paths::paths_exist,
            running::list_running_sessions,
            running::resolve_pty_bindings,
            sessions::list_sessions_by_paths,
            recovery::ensure_session_symlink,
            recovery::dir_exists,
            recovery::recover_session_cwd,
            worktree::list_worktrees,
            hook_server::install_claude_hook,
            perf::perf_startup,
            perf::perf_record,
            paths::get_tool_logo
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // app 退出 / 窗口关闭时杀掉所有 PTY 进程组并退出
    // macOS：红叉关窗不退出 app（Dock 还在），必须主动 exit 确保进程清理
    app.run(|app_handle, event| {
        use tauri::{Emitter, Manager};
        match event {
            tauri::RunEvent::ExitRequested { .. } => {
                let state = app_handle.state::<pty::PtyState>();
                pty::kill_all_ptys(&state);
            }
            tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { .. }, .. } => {
                let state = app_handle.state::<pty::PtyState>();
                pty::kill_all_ptys(&state);
                app_handle.exit(0);
            }
            _ => {}
        }
    });
}
