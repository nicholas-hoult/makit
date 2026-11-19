//! 标签 ↔ 会话自动绑定（对齐 Tauri App.tsx `tryBindPtyTabs` + `[sessions]` effect）：
//! 在 shell / 新建标签里手动启动的 claude，要认出它是哪个会话，把 session id 写回标签
//! （标签变成 resume：标题、状态点、侧栏高亮 / 「已打开」、⌘L、「正看着不弹通知」、关标签杀子进程、重启后 `claude -r` 恢复都靠它）。
//!
//! 两条来源（和 Tauri 一样）：
//! 1. 运行中的会话自带 `pty_id`（进程的 `MAKIT_PTY_ID`），等于某个标签 id → 直接绑（`match_running`）
//! 2. 发出第一条消息之前 jsonl 还不存在、会话不在列表里：读 `~/.claude/sessions/<pid>.json`（core `resolve_pty_bindings`）
//!
//! 为什么单独测：错了在界面上就是侧栏不高亮当前会话、点它提示「正在运行中不能重复启动」、重启后对话丢了。

use std::collections::HashSet;

use makit_core::SessionMeta;

use crate::workspace::model::{collect_containers, LayoutNode, TabKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingTab {
    pub container_id: String,
    pub tab_id: String,
}

/// 还没绑上会话的 shell / 新建标签
pub fn pending_tabs(root: &LayoutNode) -> Vec<PendingTab> {
    let mut out = Vec::new();
    for c in collect_containers(root) {
        for t in &c.tabs {
            if matches!(t.kind, TabKind::Shell | TabKind::New) && t.session_id.is_none() {
                out.push(PendingTab { container_id: c.id.clone(), tab_id: t.id.clone() });
            }
        }
    }
    out
}

/// 运行中、`pty_id` 等于标签 id 的会话 → (标签, 会话)
pub fn match_running<'a>(pending: &[PendingTab], sessions: &'a [SessionMeta]) -> Vec<(PendingTab, &'a SessionMeta)> {
    pending
        .iter()
        .filter_map(|p| sessions.iter().find(|s| s.running && !s.pty_id.is_empty() && s.pty_id == p.tab_id).map(|s| (p.clone(), s)))
        .collect()
}

/// 点一条会话时该怎么办（#253）
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenDecision {
    /// 它已经在本软件的某个标签里：切过去
    Switch { container_id: String, tab_id: String },
    /// 它在**别的终端**里跑着（不是本软件）：不能重复启动，提示 PID
    BlockedExternal { pid: u32 },
    /// 没在跑：新开 resume 标签
    OpenNew,
}

/// 判定顺序：①有标签的 `session_id` 就是它（不分标签类型：shell 标签里手动跑起来、已被自动绑定的会话也算）
/// ②会话正在跑，且它的 `pty_id`（进程环境变量 `MAKIT_PTY_ID`）等于某个标签 id（用户在本软件的 shell / 新建标签里手动启动、
/// 还没来得及绑定）③正在跑但不是本软件的标签 → 外部进程 ④其余 → 新开
pub fn decide_open(root: &LayoutNode, s: &SessionMeta) -> OpenDecision {
    let containers = collect_containers(root);
    let find = |pred: &dyn Fn(&crate::workspace::model::PaneTab) -> bool| {
        containers.iter().find_map(|c| c.tabs.iter().find(|t| pred(t)).map(|t| OpenDecision::Switch { container_id: c.id.clone(), tab_id: t.id.clone() }))
    };
    if let Some(d) = find(&|t| t.session_id.as_deref() == Some(s.session_id.as_str())) {
        return d;
    }
    if s.running && !s.pty_id.is_empty() {
        if let Some(d) = find(&|t| t.id == s.pty_id) {
            return d;
        }
    }
    if s.running && s.pid != 0 {
        return OpenDecision::BlockedExternal { pid: s.pid };
    }
    OpenDecision::OpenNew
}

/// Where to bring an ended session back when a request comes in through the local-session link (#D-2044): the
/// caller only gives a `session_id`, already not running (checked before this), with no idea whether makit still
/// has a tab for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkResumeDecision {
    /// A tab for this session is still there -- stopped, or just sitting on its exited terminal: bring that one back.
    Revive { tab_id: String },
    /// No tab anywhere (closed, or never opened in makit): start a fresh one.
    OpenNew,
}

/// Find any tab bound to `session_id`, regardless of `stopped` (#D-2044): a resume through the link must reuse a
/// tab whose session simply exited on its own just as much as one the user explicitly stopped.
pub fn decide_link_resume(root: &LayoutNode, session_id: &str) -> LinkResumeDecision {
    for c in collect_containers(root) {
        if let Some(t) = c.tabs.iter().find(|t| t.session_id.as_deref() == Some(session_id)) {
            return LinkResumeDecision::Revive { tab_id: t.id.clone() };
        }
    }
    LinkResumeDecision::OpenNew
}

/// Every session id that currently has a tab somewhere, regardless of `stopped` (#D-2143): the local-session
/// link's `sessions` reports this per session as `has_tab` -- a caller can tell, for a whole batch, which
/// sessions `resume { allow_new_tab: false }` would actually revive without calling it on each one.
pub fn tabbed_session_ids(root: &LayoutNode) -> HashSet<String> {
    collect_containers(root).into_iter().flat_map(|c| c.tabs.iter()).filter_map(|t| t.session_id.clone()).collect()
}

/// Whether `WorkspaceView::resume_tab` has anything to do, given whether that tab currently has a live
/// `TerminalView` entry and if so whether its process has exited. `terminal_exited` is `None` when there is no
/// entry at all -- true for every tab right after a cold start (nothing has been rendered yet this launch) and
/// also right after the tab was explicitly stopped (its entry gets torn down then).
///
/// #D-2137: the bug this fixes. The old check was `!exited && !was_stopped` with `exited` defaulting to `false`
/// when there was no terminal entry -- so "no entry yet" read as "already running fine", and a tab resumed
/// through the local-session link (or any tab that simply was not the active one in its pane since launch) never
/// got its terminal built at all: `resume_tab` silently no-opped, `force_spawn_now` then found nothing to spawn,
/// and every `resume` after a restart came back "did not start a process". Needing revival is really just "is
/// there *not* a live, still-running terminal for it" -- `None` and `Some(true)` both mean yes.
pub fn needs_revival(terminal_exited: Option<bool>) -> bool {
    terminal_exited != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::sessions::tests::session;
    use crate::workspace::model::{ContainerNode, PaneTab};

    fn tab(id: &str, kind: TabKind, sid: Option<&str>) -> PaneTab {
        PaneTab {
            id: id.into(),
            kind,
            cwd: "/w".into(),
            init_command: None,
            session_id: sid.map(String::from),
            session_short_id: None,
            label: String::new(),
            stopped: false, pinned: false,
        }
    }

    fn root(tabs: Vec<PaneTab>) -> LayoutNode {
        LayoutNode::Container(ContainerNode { id: "c1".into(), active_tab_id: tabs[0].id.clone(), tabs, tab_history: vec![] })
    }

    #[test]
    fn only_unbound_shell_and_new_tabs_are_pending() {
        let r = root(vec![
            tab("t_shell", TabKind::Shell, None),
            tab("t_new", TabKind::New, None),
            tab("t_bound", TabKind::Shell, Some("s1")),
            tab("t_resume", TabKind::Resume, Some("s2")),
        ]);
        let ids: Vec<_> = pending_tabs(&r).into_iter().map(|p| p.tab_id).collect();
        assert_eq!(ids, ["t_shell", "t_new"]);
    }

    #[test]
    fn running_session_with_matching_pty_id_binds() {
        let pending = vec![PendingTab { container_id: "c1".into(), tab_id: "t_a".into() }, PendingTab { container_id: "c1".into(), tab_id: "t_b".into() }];
        let mut a = session("sa");
        a.running = true;
        a.pty_id = "t_a".into();
        let mut stopped = session("sb");
        stopped.running = false; // 已经退出的会话不绑（pty_id 是上次的残留）
        stopped.pty_id = "t_b".into();
        let mut other = session("sc");
        other.running = true;
        other.pty_id = String::new(); // 不在 makit 里跑的会话没有 pty_id
        let sessions = vec![a, stopped, other];
        let got: Vec<_> = match_running(&pending, &sessions).into_iter().map(|(p, s)| (p.tab_id, s.session_id.clone())).collect();
        assert_eq!(got, [("t_a".to_string(), "sa".to_string())]);
    }

    /// 为什么要测（#253）：错了在界面上就是「会话明明在本软件的某个标签里跑着，点它却弹『正在运行中，不能重复启动』，不跳过去」
    fn running(id: &str, pid: u32, pty: &str) -> SessionMeta {
        let mut s = session(id);
        s.running = true;
        s.pid = pid;
        s.pty_id = pty.into();
        s
    }

    #[test]
    fn session_in_a_resume_tab_switches_there() {
        let r = root(vec![tab("t1", TabKind::Shell, None), tab("t2", TabKind::Resume, Some("s1"))]);
        let d = decide_open(&r, &running("s1", 100, "t2"));
        assert_eq!(d, OpenDecision::Switch { container_id: "c1".into(), tab_id: "t2".into() });
    }

    #[test]
    fn session_bound_to_a_shell_tab_also_switches() {
        // 在 shell 标签里手动跑起来、已被自动绑定：session_id 写进去了，但标签类型还是 Shell
        let r = root(vec![tab("t_shell", TabKind::Shell, Some("s1"))]);
        let d = decide_open(&r, &running("s1", 100, "t_shell"));
        assert_eq!(d, OpenDecision::Switch { container_id: "c1".into(), tab_id: "t_shell".into() }, "不能因为不是 Resume 标签就当成外部进程");
    }

    #[test]
    fn running_session_not_yet_bound_is_found_by_pty_id() {
        // 手动在 shell 标签里跑 claude，还没发第一条消息，标签没绑定，但进程环境里的 MAKIT_PTY_ID 就是这个标签
        let r = root(vec![tab("t_shell", TabKind::Shell, None), tab("t_other", TabKind::Shell, None)]);
        let d = decide_open(&r, &running("s9", 100, "t_shell"));
        assert_eq!(d, OpenDecision::Switch { container_id: "c1".into(), tab_id: "t_shell".into() });
    }

    #[test]
    fn running_in_another_terminal_is_blocked_with_its_pid() {
        let r = root(vec![tab("t1", TabKind::Shell, None)]);
        assert_eq!(decide_open(&r, &running("s2", 4242, "")), OpenDecision::BlockedExternal { pid: 4242 }, "没有 pty_id = 不是本软件开的");
        // pty_id 指向一个已经不存在的标签（上次的残留）：也是外部
        assert_eq!(decide_open(&r, &running("s2", 4242, "t_gone")), OpenDecision::BlockedExternal { pid: 4242 });
    }

    #[test]
    fn not_running_opens_a_new_tab() {
        let r = root(vec![tab("t1", TabKind::Shell, None)]);
        let mut s = session("s3");
        s.running = false;
        s.pty_id = "t1".into(); // 退出后残留的 pty_id 不能当成「在这个标签里跑」
        assert_eq!(decide_open(&r, &s), OpenDecision::OpenNew);
    }

    #[test]
    fn pid_zero_running_flag_is_not_blocked() {
        // running 但 pid 为 0（还没取到）：保持原行为——不拦，直接开
        let r = root(vec![tab("t1", TabKind::Shell, None)]);
        assert_eq!(decide_open(&r, &running("s4", 0, "")), OpenDecision::OpenNew);
    }

    /// #D-2044: a resume through the local-session link must reuse a stopped tab, not open a second one for the
    /// same session -- that was piling up tabs every time the other side resumed an ended session.
    #[test]
    fn link_resume_revives_a_stopped_tab_instead_of_opening_a_new_one() {
        let mut t = tab("t1", TabKind::Resume, Some("s1"));
        t.stopped = true;
        let r = root(vec![t]);
        assert_eq!(decide_link_resume(&r, "s1"), LinkResumeDecision::Revive { tab_id: "t1".into() });
    }

    /// A tab whose process just exited on its own (not through the explicit "stop") is not marked `stopped` either,
    /// but it is still the same tab and must be reused -- `stopped` only tells the UI which placeholder to render.
    #[test]
    fn link_resume_revives_a_live_but_not_explicitly_stopped_tab_too() {
        let r = root(vec![tab("t1", TabKind::Resume, Some("s1"))]);
        assert_eq!(decide_link_resume(&r, "s1"), LinkResumeDecision::Revive { tab_id: "t1".into() });
    }

    #[test]
    fn link_resume_opens_a_new_tab_only_when_none_matches() {
        let r = root(vec![tab("t1", TabKind::Shell, None), tab("t2", TabKind::Resume, Some("other"))]);
        assert_eq!(decide_link_resume(&r, "s1"), LinkResumeDecision::OpenNew);
    }

    /// #D-2143: `sessions`'s `has_tab` field reads straight off this -- a stopped tab and a shell tab with no
    /// session both count correctly, and the same session bound to two tabs (split) is still just one entry.
    #[test]
    fn tabbed_session_ids_collects_every_bound_tab_once_regardless_of_stopped() {
        let mut stopped = tab("t2", TabKind::Resume, Some("s2"));
        stopped.stopped = true;
        let r = root(vec![tab("t1", TabKind::Resume, Some("s1")), stopped, tab("t3", TabKind::Shell, None), tab("t4", TabKind::Resume, Some("s1"))]);
        let mut ids: Vec<_> = tabbed_session_ids(&r).into_iter().collect();
        ids.sort();
        assert_eq!(ids, ["s1", "s2"]);
    }

    /// #D-2137: repro for "resume did not start a process" after a restart. No terminal entry yet must mean
    /// "needs reviving", not "already running fine" -- that was the whole bug.
    #[test]
    fn no_terminal_entry_needs_revival_just_like_an_exited_one() {
        assert!(needs_revival(None), "cold start / just-stopped: no entry at all -- must still revive");
        assert!(needs_revival(Some(true)), "entry exists but its process exited -- must revive");
        assert!(!needs_revival(Some(false)), "entry exists and is still alive -- leave it alone");
    }
}
