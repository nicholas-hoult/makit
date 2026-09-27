//! Tauri 命令：全部是薄转发，业务逻辑在各业务模块（#226 之后在 makit-core）。
//!
//! 命令名就是前端 `invoke("…")` 的名字，改名 = 改前端协议，不要动。
//! 同步函数一律 `#[tauri::command(async)]`（#216，`command_thread_tests` 盯着）。

use serde_json::Value;

use makit_core::{archive, hook, paths, perf, process, recovery, running, sessions, worktree};

#[tauri::command(async)]
pub fn list_sessions(cwd_mode: Option<String>) -> Result<Vec<sessions::SessionMeta>, String> {
    sessions::list_sessions(cwd_mode)
}

#[tauri::command(async)]
pub fn list_sessions_by_paths(paths: Vec<String>, cwd_mode: Option<String>) -> Result<Vec<sessions::SessionMeta>, String> {
    sessions::list_sessions_by_paths(paths, cwd_mode)
}

#[tauri::command(async)]
pub fn read_session_messages(session_id: String) -> Result<Vec<sessions::ConversationMessage>, String> {
    sessions::read_session_messages(session_id)
}

#[tauri::command(async)]
pub fn read_session_meta(session_id: String) -> Result<Option<sessions::SessionMeta>, String> {
    sessions::read_session_meta(session_id)
}

#[tauri::command(async)]
pub fn find_session_in_cwd_after(cwd: String, after_ts: i64) -> Result<Option<String>, String> {
    sessions::find_session_in_cwd_after(cwd, after_ts)
}

#[tauri::command(async)]
pub fn archive_session(session_id: String) -> Result<(), String> {
    archive::archive_session(session_id)
}

#[tauri::command(async)]
pub fn unarchive_session(session_id: String) -> Result<(), String> {
    archive::unarchive_session(session_id)
}

#[tauri::command(async)]
pub fn ensure_session_symlink(session_id: String, cwd: String, storage_folder: String) -> Result<String, String> {
    recovery::ensure_session_symlink(session_id, cwd, storage_folder)
}

#[tauri::command(async)]
pub fn dir_exists(path: String) -> bool {
    recovery::dir_exists(path)
}

#[tauri::command(async)]
pub fn recover_session_cwd(
    mode: String,
    session_id: String,
    original_cwd: String,
    target_cwd: String,
    storage_folder: String,
) -> Result<recovery::RecoveredSession, String> {
    recovery::recover_session_cwd(mode, session_id, original_cwd, target_cwd, storage_folder)
}

#[tauri::command(async)]
pub fn list_worktrees(git_root: String) -> Result<Vec<worktree::WorktreeInfo>, String> {
    Ok(worktree::list_worktrees_for(&git_root))
}

#[tauri::command]
pub async fn open_path(path: String, reveal: bool) -> Result<(), String> {
    paths::open_path(path, reveal)
}

#[tauri::command(async)]
pub fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    paths::paths_exist(paths)
}

#[tauri::command]
pub async fn get_tool_logo(tool: String) -> Result<String, String> {
    paths::get_tool_logo(tool).await
}

#[tauri::command(async)]
pub fn list_running_sessions() -> Vec<running::RunningMeta> {
    running::list_running_sessions()
}

#[tauri::command(async)]
pub fn resolve_pty_bindings(pty_ids: Vec<String>) -> Vec<running::PtyBinding> {
    running::resolve_pty_bindings(pty_ids)
}

#[tauri::command(async)]
pub fn install_claude_hook() -> Result<String, String> {
    hook::install_claude_hook()
}

#[tauri::command(async)]
pub fn kill_pids(pids: Vec<u32>) -> Result<(), String> {
    process::kill_pids(&pids);
    Ok(())
}

#[tauri::command(async)]
pub fn perf_startup() -> perf::StartupInfo {
    perf::startup_info()
}

#[tauri::command(async)]
pub fn perf_record(events: Vec<Value>) {
    perf::record_batch(events)
}
