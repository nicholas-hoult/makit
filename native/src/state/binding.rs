//! 标签 ↔ 会话自动绑定（对齐 Tauri App.tsx `tryBindPtyTabs` + `[sessions]` effect）：
//! 在 shell / 新建标签里手动启动的 claude，要认出它是哪个会话，把 session id 写回标签
//! （标签变成 resume：标题、状态点、侧栏高亮 / 「已打开」、⌘L、「正看着不弹通知」、关标签杀子进程、重启后 `claude -r` 恢复都靠它）。
//!
//! 两条来源（和 Tauri 一样）：
//! 1. 运行中的会话自带 `pty_id`（进程的 `MAKIT_PTY_ID`），等于某个标签 id → 直接绑（`match_running`）
//! 2. 发出第一条消息之前 jsonl 还不存在、会话不在列表里：读 `~/.claude/sessions/<pid>.json`（core `resolve_pty_bindings`）
//!
//! 为什么单独测：错了在界面上就是侧栏不高亮当前会话、点它提示「正在运行中不能重复启动」、重启后对话丢了。

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
}
