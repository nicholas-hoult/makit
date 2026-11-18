//! 会话操作（界面清单 M 节）：置顶、归档 / 取消归档（运行中先弹系统确认框）、复制、Finder 显示、打开会话。
//! 侧栏右键菜单、⌘K、详情面板共用这一份，别的包直接调。

use crate::ts;
use gpui::{App, AsyncApp, ClipboardItem, PromptButton, PromptLevel, Window};
use makit_core::SessionMeta;

use super::{host, show_toast, toast};
use crate::state::binding::{decide_link_resume, decide_open, LinkResumeDecision, OpenDecision};
use crate::state::AppState;
use crate::workspace::model::{collect_containers, resume_cmd, resume_init_command, Dir, Opened, Side, TabKind, TabSpec};

fn tool_of(s: &SessionMeta) -> Option<&str> {
    makit_core::ai_provider::tool_arg(&s.tool)
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
            let _ = cx.update(|cx| show_toast(ts!("session.toast.open_failed", error = e), toast::REVEAL_FAIL, cx));
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
                Err(e) => show_toast(ts!("sidebar.toast.archive_failed", error = e), toast::ARCHIVE_FAIL, cx),
            });
        })
        .detach();
        return;
    }
    let confirm = s.running.then(|| {
        window.prompt(
            PromptLevel::Warning,
            &ts!("sidebar.archive.confirm_title"),
            Some(&ts!("sidebar.archive.confirm_detail", pid = s.pid)),
            &[PromptButton::ok(ts!("sidebar.menu.archive")), PromptButton::cancel(ts!("common.cancel"))],
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
                Err(e) => show_toast(ts!("sidebar.toast.archive_failed", error = e), toast::ARCHIVE_FAIL, cx),
            }
        });
    })
    .detach();
}

/// Batch archive (#294): one confirm when any of them is running or open in a tab, one write of archived.json,
/// then each one's tabs are closed and its child processes killed (same as the single archive). The caller has
/// already left out pinned / archived ones (`sidebar::multi_select::plan_archive`); `pinned_skipped` only goes
/// into the toast.
pub fn archive_sessions(ids: Vec<String>, active: usize, pinned_skipped: usize, window: &mut Window, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let n = ids.len();
    let confirm = (active > 0).then(|| {
        window.prompt(
            PromptLevel::Warning,
            &ts!("sidebar.archive.batch_confirm_title", n = n),
            Some(&ts!("sidebar.archive.batch_confirm_detail", m = active)),
            &[PromptButton::ok(ts!("sidebar.menu.archive")), PromptButton::cancel(ts!("common.cancel"))],
            cx,
        )
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Some(rx) = confirm {
            if rx.await.ok() != Some(0) {
                return;
            }
        }
        // mark first, as the single archive does: keeps the background running-state merge off them during the write
        let _ = cx.update(|cx| AppState::global(cx).update(cx, |st, _| st.archiving.extend(ids.iter().cloned())));
        let batch = ids.clone();
        let r = cx.background_executor().spawn(async move { makit_core::archive::archive_sessions(&batch) }).await;
        let _ = cx.update(|cx| {
            AppState::global(cx).update(cx, |st, _| ids.iter().for_each(|id| { st.archiving.remove(id); }));
            match r {
                Ok(_) => {
                    for id in &ids {
                        close_session_tabs(id, cx);
                    }
                    update_sessions(&ids, cx, |m| {
                        m.archived = true;
                        m.running = false;
                        m.status = "idle".into();
                    });
                    let msg = if pinned_skipped > 0 {
                        ts!("sidebar.toast.archived_n_pinned_skipped", n = n, k = pinned_skipped)
                    } else {
                        ts!("sidebar.toast.archived_n", n = n)
                    };
                    show_toast(msg, toast::BATCH_ARCHIVED, cx);
                }
                Err(e) => show_toast(ts!("sidebar.toast.archive_failed", error = e), toast::ARCHIVE_FAIL, cx),
            }
        });
    })
    .detach();
}

/// Batch unarchive (#294): one write, no confirm (nothing is closed)
pub fn unarchive_sessions(ids: Vec<String>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        let batch = ids.clone();
        let r = cx.background_executor().spawn(async move { makit_core::archive::unarchive_sessions(&batch) }).await;
        let _ = cx.update(|cx| match r {
            Ok(_) => {
                update_sessions(&ids, cx, |m| m.archived = false);
                show_toast(ts!("sidebar.toast.unarchived_n", n = ids.len()), toast::BATCH_ARCHIVED, cx);
            }
            Err(e) => show_toast(ts!("sidebar.toast.archive_failed", error = e), toast::ARCHIVE_FAIL, cx),
        });
    })
    .detach();
}

/// `update_session` for many, with one `sessions_changed`
fn update_sessions(ids: &[String], cx: &mut App, f: impl Fn(&mut SessionMeta)) {
    let state = AppState::global(cx);
    state.update(cx, |s, cx| {
        let mut changed = false;
        for m in s.sessions.iter_mut().filter(|m| ids.contains(&m.session_id)) {
            f(m);
            changed = true;
        }
        if changed {
            s.sessions_changed(cx);
        }
    });
}

/// 关掉这个会话的所有标签（WorkspaceView::close_tab 会一并杀终端和它记下的子进程）。
/// Not open in any tab: still kill its recorded child processes (the ones that escaped the process group), as the
/// sidebar's own archive used to — otherwise they would outlive the archived session.
fn close_session_tabs(session_id: &str, cx: &mut App) {
    let st = AppState::global(cx);
    let targets: Vec<(String, String)> = {
        let s = st.read(cx);
        collect_containers(&s.workspace.state.root)
            .into_iter()
            .flat_map(|c| c.tabs.iter().filter(|t| t.session_id.as_deref() == Some(session_id)).map(move |t| (c.id.clone(), t.id.clone())))
            .collect()
    };
    if targets.is_empty() {
        let pids: Vec<u32> = st.read(cx).session(session_id).map(|m| m.child_processes.iter().map(|p| p.pid).collect()).unwrap_or_default();
        if !pids.is_empty() {
            makit_core::process::kill_pids(&pids);
        }
        return;
    }
    let Some(h) = host(cx) else { return };
    let ws = h.read(cx).workspace.clone();
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
        log::warn!(target: "session", "session {}: creating the storage symlink before resuming failed: {e}", s.short_id);
    }
}

/// 打开一条会话（`openResumeTab`）：已经在本软件的某个标签里（resume 标签、被绑定的 shell 标签、或进程的 `MAKIT_PTY_ID`
/// 指向某个标签）就切过去；是别的终端里的进程就 toast 拦下；否则先过一遍容量上限（`capacity_decision`），再确保软链、开
/// resume 标签。判定见 `state::binding::decide_open`（#253）。`window` 只在容量上限触发且策略是「每次问」时才用得上
/// （本机确认框，#D-2090）——其余分支都不需要它
pub fn open_session(session_id: &str, window: &mut Window, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(s) = state.read(cx).session(session_id).cloned() else { return };
    let decision = decide_open(&state.read(cx).workspace.state.root, &s);
    log::info!(target: "session", "open session {}: {}", s.short_id, match &decision {
        OpenDecision::Switch { .. } => ts!("session.decision.switch").to_string(),
        OpenDecision::BlockedExternal { pid } => ts!("session.decision.external", pid = pid),
        OpenDecision::OpenNew => ts!("session.decision.open_new").to_string(),
    });
    match decision {
        OpenDecision::Switch { container_id, tab_id } => {
            state.update(cx, |st, cx| {
                st.workspace.tab_click(&container_id, &tab_id);
                // Opening a session on purpose brings its stopped tab back (#287)
                st.workspace.set_tab_stopped(&tab_id, false);
                st.workspace_changed(cx);
            });
        }
        OpenDecision::BlockedExternal { pid } => {
            show_toast(ts!("sidebar.toast.already_running", pid = pid), toast::ALREADY_RUNNING, cx);
        }
        OpenDecision::OpenNew => open_new_session(s, window, cx),
    }
}

/// Opens a brand-new tab for `s`, going through the capacity cap first (#D-2090 接线阶段). Shared by the sidebar's
/// own open (this file) and link.sock's `resume` when it has nowhere to revive (`resume_session` below).
fn do_open_new(s: &SessionMeta, cx: &mut App) {
    ensure_symlink(s);
    let title = crate::sidebar::groups::session_title(&s.display_name, &s.first_user_msg, &s.short_id);
    let state = AppState::global(cx);
    state.update(cx, |st, cx| {
        let _: Opened = st.workspace.open_session(&s.session_id, &s.short_id, &s.cwd, Some(&title), tool_of(s));
        st.workspace_changed(cx);
    });
}

/// The capacity cap acting on a **local** open (sidebar click / ⌘K / Enter): unlike the remote link.sock path, the
/// person triggering this is sitting right at the machine, so `AskEachTime` gets an actual confirm dialog here
/// instead of degrading to a refusal (#D-2090; see `capacity_block` below for the remote side of that split).
fn open_new_session(s: SessionMeta, window: &mut Window, cx: &mut App) {
    use crate::state::capacity::EvictionDecision;
    match capacity_decision(cx) {
        None => do_open_new(&s, cx),
        Some((limit, EvictionDecision::Blocked(_))) => show_toast(limit.reason().to_string(), toast::CAPACITY_BLOCKED, cx),
        Some((_, EvictionDecision::AutoSleep { session_id })) => {
            sleep_eviction_victim(&session_id, cx);
            do_open_new(&s, cx);
        }
        Some((_, EvictionDecision::AskToSleep { session_id: victim })) => {
            let label = AppState::global(cx).read(cx).session(&victim).map(|m| m.short_id.clone()).unwrap_or(victim.clone());
            let rx = window.prompt(
                PromptLevel::Warning,
                &ts!("capacity.ask_evict.title"),
                Some(&ts!("capacity.ask_evict.detail", session = label)),
                &[PromptButton::ok(ts!("capacity.ask_evict.confirm")), PromptButton::cancel(ts!("common.cancel"))],
                cx,
            );
            cx.spawn(async move |cx: &mut AsyncApp| {
                if rx.await.ok() != Some(0) {
                    return;
                }
                let _ = cx.update(|cx| {
                    sleep_eviction_victim(&victim, cx);
                    do_open_new(&s, cx);
                });
            })
            .detach();
        }
    }
}

/// Puts the eviction queue's victim to sleep (#287's stop-but-keep-tab, via `WorkspaceView::stop_tabs_now`
/// directly rather than the confirm-having `stop_tabs`): the victim is always idle (`capacity::evictable`'s own
/// precondition), so the busy-confirm `stop_tabs` would show never applies here, and skipping it means this needs
/// no `Window` -- important for the `AskToSleep` branch above, which calls this from inside an async continuation
/// after its own confirm dialog, where there is no `Window` to give it.
fn sleep_eviction_victim(session_id: &str, cx: &mut App) {
    let Some(h) = host(cx) else { return };
    let ws = h.read(cx).workspace.clone();
    let tids = stoppable_tabs(&AppState::global(cx).read(cx).workspace.state, session_id);
    if tids.is_empty() {
        return;
    }
    ws.update(cx, |w, cx| w.stop_tabs_now(&tids, cx));
}

/// The live capacity decision (#D-2090 接线阶段): resource readings + the active session list + the user's chosen
/// threshold/policy settings. `None` means there is room, proceed as normal. Returns the triggering `CapacityLimit`
/// alongside the decision so callers that end up refusing (`Blocked`, or a remote caller's `AskToSleep`) have a
/// reason to report without recomputing it.
pub fn capacity_decision(cx: &App) -> Option<(crate::state::capacity::CapacityLimit, crate::state::capacity::EvictionDecision)> {
    use crate::state::capacity::*;
    let state = AppState::global(cx);
    let s = state.read(cx);
    let active_count = s.sessions.iter().filter(|m| m.running).count();
    let free_mem_ratio = crate::state::resources::free_mem_ratio().unwrap_or(1.0);
    let load_1m = crate::state::resources::load_1m().unwrap_or(0.0);
    let limits = limits_from_prefs(s.prefs.capacity.max_active, s.prefs.capacity.min_free_mem_percent, s.prefs.capacity.max_load_1m);
    let limit = capacity_limit(active_count, free_mem_ratio, load_1m, limits)?;
    let pinned: std::collections::HashSet<String> = s.prefs.pinned_sessions.iter().cloned().collect();
    let focused = focused_session_ids(&s.workspace.state.root);
    let candidates = active_candidates(&s.sessions, &pinned, &focused);
    let victim = eviction_victim(&candidates);
    let policy = policy_from_str(&s.prefs.capacity.eviction_policy);
    Some((limit, decide_eviction(limit, victim, policy)))
}

/// The capacity cap acting on `open` / `resume` through the local-session link (#D-2066, #D-2078, #D-2090): a
/// remote caller never gets asked interactively -- it cannot see or answer a confirm dialog on this machine (TRD
/// #316: "跨 link 的调用方不能代替本机用户做决定") -- so `AskToSleep` resolves the same as `Off` here, a plain
/// refusal with the reason. `Auto` still actually sleeps the victim and lets the request through; `None` means
/// there is room. Readings that cannot be taken (non-macOS, `vm_stat` missing) fail open: a cap nobody can
/// satisfy would make the cap itself the outage.
pub fn capacity_block(cx: &mut App) -> Option<String> {
    use crate::state::capacity::EvictionDecision;
    match capacity_decision(cx)? {
        (_, EvictionDecision::AutoSleep { session_id }) => {
            sleep_eviction_victim(&session_id, cx);
            None
        }
        (limit, _) => Some(limit.reason().to_string()),
    }
}

/// Resume an ended session asked for through the local-session link (#299, #D-2044). Whether it is already
/// running is checked by the caller (the `resume` op, before this is even reached) -- this only decides where:
/// reuse the tab it still has (stopped, or just sitting on its exited terminal) instead of opening a second one,
/// and only opens a new tab when none exists (`state::binding::decide_link_resume`). Plain `open_session` above
/// cannot be reused here: its "switch to an existing tab" path only clears `stopped`, which does nothing for a
/// tab whose terminal simply exited on its own without ever being marked stopped, leaving it sitting dead instead
/// of actually resuming -- `resume_tab` is the one path that rebuilds the terminal in both cases.
///
/// Remote-only (called from `link_switch`'s `MainOp::Resume`, which already ran `capacity_block` before this):
/// the local sidebar/palette resume a stopped tab through `WorkspaceView::resume_tab` directly, uncapped for now
/// (#D-2090 分期：手动恢复自己已知的某一个标签是一次性、可见的动作，不是这次事故的成因，留作后续)
pub fn resume_session(session_id: &str, cx: &mut App) {
    let state = AppState::global(cx);
    let Some(s) = state.read(cx).session(session_id).cloned() else { return };
    match decide_link_resume(&state.read(cx).workspace.state.root, session_id) {
        LinkResumeDecision::Revive { tab_id } => {
            let Some(h) = host(cx) else { return };
            let ws = h.read(cx).workspace.clone();
            ws.update(cx, |w, cx| w.resume_tab(&tab_id, cx));
        }
        LinkResumeDecision::OpenNew => do_open_new(&s, cx),
    }
}

/// The tabs bound to this session that can be stopped (#287)
pub fn stoppable_tabs(ws: &crate::workspace::model::WorkspaceState, session_id: &str) -> Vec<String> {
    collect_containers(&ws.root)
        .into_iter()
        .flat_map(|c| c.tabs.iter())
        .filter(|t| t.session_id.as_deref() == Some(session_id) && crate::workspace::model::can_stop(t))
        .map(|t| t.id.clone())
        .collect()
}

/// Sidebar "Stop": stop every tab of this session, keeping them (#287)
pub fn stop_session_tabs(session_id: &str, window: &mut Window, cx: &mut App) {
    let Some(h) = host(cx) else { return };
    let ws = h.read(cx).workspace.clone();
    let tids = stoppable_tabs(&AppState::global(cx).read(cx).workspace.state, session_id);
    ws.update(cx, |w, cx| w.stop_tabs(tids, window, cx));
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
