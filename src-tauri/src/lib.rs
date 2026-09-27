//! Tauri 外壳：命令薄转发（commands.rs）、PTY（pty.rs）、窗口事件和 emit。
//! 业务逻辑全在 makit-core（#226）。

mod commands;
mod pty;

use makit_core::{hook, perf, watcher};

pub use makit_core::sessions::{get_git_root, humanize_duration, ConversationMessage, SessionMeta};
pub use makit_core::{ProcessInfo, PtyBinding};

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
            ("commands.rs", include_str!("commands.rs")),
            ("pty.rs", include_str!("pty.rs")),
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
            // 会话目录变化 → 前端事件（载荷格式和回调式改造之前一样）
            let handle = app.handle().clone();
            watcher::start(move |ev| {
                use tauri::Emitter;
                match ev {
                    watcher::WatchEvent::RunningChanged => {
                        let _ = handle.emit("running-changed", ());
                    }
                    watcher::WatchEvent::SessionsChanged(paths) => {
                        let _ = handle.emit("sessions-changed", paths);
                    }
                }
            });
            // claude 的 Notification hook → 前端 `claude-hook` 事件（原样一行 JSON）
            let handle = app.handle().clone();
            hook::start(move |line| {
                use tauri::Emitter;
                let _ = handle.emit("claude-hook", line);
            });

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
            commands::list_sessions,
            commands::read_session_messages,
            commands::archive_session,
            commands::unarchive_session,
            commands::find_session_in_cwd_after,
            commands::read_session_meta,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_ack,
            pty::pty_resize,
            pty::pty_kill,
            commands::kill_pids,
            commands::open_path,
            commands::paths_exist,
            commands::list_running_sessions,
            commands::resolve_pty_bindings,
            commands::list_sessions_by_paths,
            commands::ensure_session_symlink,
            commands::dir_exists,
            commands::recover_session_cwd,
            commands::list_worktrees,
            commands::install_claude_hook,
            commands::perf_startup,
            commands::perf_record,
            commands::get_tool_logo
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // app 退出 / 窗口关闭时杀掉所有 PTY 进程组并退出
    // macOS：红叉关窗不退出 app（Dock 还在），必须主动 exit 确保进程清理
    app.run(|app_handle, event| {
        use tauri::Manager;
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
