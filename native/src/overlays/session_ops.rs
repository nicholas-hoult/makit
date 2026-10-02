//! 会话操作（界面清单 M 节）：置顶、归档 / 取消归档（运行中先弹系统确认框）、复制、Finder 显示、打开会话。
//! 侧栏右键菜单、⌘K、详情面板共用这一份，别的包直接调。

use gpui::{App, AsyncApp, ClipboardItem, PromptButton, PromptLevel, Window};
use makit_core::SessionMeta;

use super::{host, show_toast, toast};
use crate::state::binding::{decide_open, OpenDecision};
use crate::state::AppState;
use crate::workspace::model::{collect_containers, resume_cmd, resume_init_command, Dir, Opened, Side, TabKind, TabSpec};

fn tool_of(s: &SessionMeta) -> Option<&str> {
    (s.tool == "codex").then_some("codex")
}

/// 复制恢复命令：`cd <cwd> && claude -r <id>`（codex 是 `codex resume <id>`）
pub fn resume_command(s: &SessionMeta) -> String {
    format!("cd {} && {}", s.cwd, resume_cmd(&s.session_id, tool_of(s)))
}

pub fn toggle_pin(session_id: &str, cx: &mut App) {
    let state = AppState::global(cx);
    state.update(cx, |s, cx| s.toggle_pin(session_id, cx));
}

/// 复制到剪贴板 + toast（「已复制 / 已复制路径 / 已复制命令」1.4s）。
/// GPUI 的剪贴板写入没有失败返回，所以没有「复制失败」那一支
pub fn copy_text(text: impl Into<String>, ok_msg: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
    show_toast(ok_msg.to_string(), toast::COPY_OK, cx);
}

/// 静默复制（侧栏菜单里的复制不弹 toast，清单 M 节）
pub fn copy_silent(text: impl Into<String>, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
}

/// 在 Finder 中显示（`open_path reveal`）。调用方传路径：会话优先用 last_cwd
pub fn reveal_in_finder(path: String, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let r = cx.background_executor().spawn(async move { makit_core::paths::open_path(path, true) }).await;
        if let Err(e) = r {
            let _ = cx.update(|cx| show_toast(format!("打开失败: {e}"), toast::REVEAL_FAIL, cx));
        }
    })
    .detach();
}

/// 会话最近所在的目录（Finder 显示用）：last_cwd 优先
pub fn best_cwd(s: &SessionMeta) -> String {
    if s.last_cwd.is_empty() { s.cwd.clone() } else { s.last_cwd.clone() }
}

fn update_session(session_id: &str, cx: &mut App, f: impl FnOnce(&mut SessionMeta)) {
    let state = AppState::global(cx);
    state.update(cx, |s, cx| {
        if let Some(m) = s.sessions.iter_mut().find(|m| m.session_id == session_id) {
            f(m);
            s.sessions_changed(cx);
        }
    });
}

/// 归档 / 取消归档。运行中的会话归档前弹**系统**确认框（NSAlert，不自绘）：
/// 归档会关掉这个会话的所有标签（连同杀子进程）。成功后乐观地把 running 置 false、status 置 idle。
pub fn toggle_archive(session_id: &str, window: &mut Window, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(s) = state.read(cx).session(session_id).cloned() else { return };
    if s.archived {
        let id = s.session_id.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let sid = id.clone();
            let r = cx.background_executor().spawn(async move { makit_core::archive::unarchive_session(sid) }).await;
            let _ = cx.update(|cx| match r {
                Ok(()) => update_session(&id, cx, |m| m.archived = false),
                Err(e) => show_toast(format!("归档失败: {e}"), toast::ARCHIVE_FAIL, cx),
            });
        })
        .detach();
        return;
    }
    let confirm = s.running.then(|| {
        window.prompt(
            PromptLevel::Warning,
            "确定归档？",
            Some(&format!("该 session 正在运行中（PID {}），归档将关闭终端并杀死子进程。", s.pid)),
            &[PromptButton::ok("归档"), PromptButton::cancel("取消")],
            cx,
        )
    });
    let id = s.session_id.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Some(rx) = confirm {
            if rx.await.ok() != Some(0) {
                return;
            }
        }
        // 先标记：挡住写盘期间后台的运行状态合并把归档状态冲掉（同 TS 的 archivingRef）
        let _ = cx.update(|cx| AppState::global(cx).update(cx, |st, _| st.archiving.insert(id.clone())));
        let sid = id.clone();
        let r = cx.background_executor().spawn(async move { makit_core::archive::archive_session(sid) }).await;
        let _ = cx.update(|cx| {
            AppState::global(cx).update(cx, |st, _| st.archiving.remove(&id));
            match r {
                Ok(()) => {
                    close_session_tabs(&id, cx);
                    update_session(&id, cx, |m| {
                        m.archived = true;
                        m.running = false;
                        m.status = "idle".into();
                    });
                }
                Err(e) => show_toast(format!("归档失败: {e}"), toast::ARCHIVE_FAIL, cx),
            }
        });
    })
    .detach();
}

/// 关掉这个会话的所有标签（WorkspaceView::close_tab 会一并杀终端）
fn close_session_tabs(session_id: &str, cx: &mut App) {
    let Some(h) = host(cx) else { return };
    let ws = h.read(cx).workspace.clone();
    let targets: Vec<(String, String)> = {
        let st = AppState::global(cx);
        let s = st.read(cx);
        collect_containers(&s.workspace.state.root)
            .into_iter()
            .flat_map(|c| c.tabs.iter().filter(|t| t.session_id.as_deref() == Some(session_id)).map(move |t| (c.id.clone(), t.id.clone())))
            .collect()
    };
    for (cid, tid) in targets.into_iter().rev() {
        ws.update(cx, |w, cx| w.close_tab(&cid, &tid, cx));
    }
}

/// 恢复前确保存储路径的软链在（`ensureSessionPath`）。失败不拦着用户（还是会去开 resume），但要留痕
fn ensure_symlink(s: &SessionMeta) {
    if s.cwd.is_empty() || s.storage_folder.is_empty() {
        return;
    }
    if let Err(e) = makit_core::recovery::ensure_session_symlink(s.session_id.clone(), s.cwd.clone(), s.storage_folder.clone()) {
        log::warn!(target: "session", "会话 {} 恢复前建存储软链失败：{e}", s.short_id);
    }
}

/// 打开一条会话（`openResumeTab`）：已经在本软件的某个标签里（resume 标签、被绑定的 shell 标签、或进程的 `MAKIT_PTY_ID`
/// 指向某个标签）就切过去；是别的终端里的进程就 toast 拦下；否则确保软链后开 resume 标签。判定见 `state::binding::decide_open`（#253）
pub fn open_session(session_id: &str, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(s) = state.read(cx).session(session_id).cloned() else { return };
    let decision = decide_open(&state.read(cx).workspace.state.root, &s);
    log::info!(target: "session", "打开会话 {}：{}", s.short_id, match &decision {
        OpenDecision::Switch { .. } => "已在本软件的标签里，切过去".to_string(),
        OpenDecision::BlockedExternal { pid } => format!("在别的终端里运行（PID {pid}），不重复启动"),
        OpenDecision::OpenNew => "新开 resume 标签".to_string(),
    });
    match decision {
        OpenDecision::Switch { container_id, tab_id } => {
            state.update(cx, |st, cx| {
                st.workspace.tab_click(&container_id, &tab_id);
                st.workspace_changed(cx);
            });
        }
        OpenDecision::BlockedExternal { pid } => {
            show_toast(format!("该 session 正在运行中（PID {pid}），不能重复启动"), toast::ALREADY_RUNNING, cx);
        }
        OpenDecision::OpenNew => {
            ensure_symlink(&s);
            let title = crate::sidebar::groups::session_title(&s.display_name, &s.first_user_msg, &s.short_id);
            state.update(cx, |st, cx| {
                let _: Opened = st.workspace.open_session(&s.session_id, &s.short_id, &s.cwd, Some(&title), tool_of(&s));
                st.workspace_changed(cx);
            });
        }
    }
}

/// ⌘Enter / ⌘⇧Enter：在当前 pane 旁边分屏打开（`handleSplitWithSession`，dir V = 左右、H = 上下）
pub fn open_session_split(session_id: &str, dir: Dir, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(s) = state.read(cx).session(session_id).cloned() else { return };
    ensure_symlink(&s);
    let spec = TabSpec {
        kind: TabKind::Resume,
        cwd: s.cwd.clone(),
        init_command: Some(resume_init_command(&s.session_id, tool_of(&s))),
        session_id: Some(s.session_id.clone()),
        session_short_id: Some(s.short_id.clone()),
    };
    state.update(cx, |st, cx| {
        let cid = st.workspace.state.active_container_id.clone();
        st.workspace.split_with_session(&cid, dir, Side::After, &spec);
        st.workspace_changed(cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_command_per_tool() {
        let mut s = crate::state::sessions::tests::session("abc");
        s.cwd = "/Users/me/p".into();
        assert_eq!(resume_command(&s), "cd /Users/me/p && claude -r abc");
        s.tool = "codex".into();
        assert_eq!(resume_command(&s), "cd /Users/me/p && codex resume abc");
        s.last_cwd = String::new();
        assert_eq!(best_cwd(&s), "/Users/me/p");
        s.last_cwd = "/Users/me/q".into();
        assert_eq!(best_cwd(&s), "/Users/me/q", "Finder 显示优先 last_cwd");
    }
}
