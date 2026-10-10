//! 标题栏文字、标签标题、标签状态点（纯逻辑）。
//!
//! 照搬 `App.tsx` 的 `titlebarText` / `stableGetTabTitle` / `stableGetTabStatus`。
//! 为什么单独测：这三处的回退链（有会话 → 没会话 → shell → kind）各有四五档，
//! 错一档在 UI 上就是标签 / 标题栏显示成 "resume"、空白或别的会话的名字。

use makit_core::SessionMeta;

use super::model::{PaneTab, TabKind};
use crate::sidebar::groups::{run_state, session_title, RunState};

/// 路径最后一段（`/a/b/` → `b`），空了就原样返回（同 App.tsx `basename`）
pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => path.to_string(),
    }
}

/// 标题栏：当前 active 标签的「项目名 · 分支」。没有标签是 "makit"；标签没绑会话或查不到 meta 时是 basename(cwd)
pub fn titlebar_text(tab: Option<&PaneTab>, meta: Option<&SessionMeta>) -> String {
    let Some(tab) = tab else { return "makit".into() };
    if tab.session_id.is_some() {
        if let Some(m) = meta {
            let proj = basename(if m.git_root.is_empty() { &m.cwd } else { &m.git_root });
            if m.git_branch.is_empty() {
                return proj;
            }
            return format!("{proj} · {}", m.git_branch);
        }
    }
    basename(&tab.cwd)
}

/// 标签标题：有 meta 用会话显示名；否则 label；shell 是「basename · shell」；再不然就是 kind
pub fn tab_title(tab: &PaneTab, meta: Option<&SessionMeta>) -> String {
    if tab.session_id.is_some() {
        if let Some(m) = meta {
            return session_title(&m.display_name, &m.first_user_msg, &m.short_id);
        }
    }
    if !tab.label.is_empty() {
        return tab.label.clone();
    }
    match tab.kind {
        TabKind::Shell => format!("{} · shell", basename(&tab.cwd)),
        TabKind::Resume => "resume".into(),
        TabKind::New => "new".into(),
    }
}

/// 会话在跑时才有状态点；没绑会话 / 没在跑 → None
pub fn tab_status(tab: &PaneTab, meta: Option<&SessionMeta>) -> Option<RunState> {
    tab.session_id.as_ref()?;
    let m = meta?;
    if !m.running {
        return None;
    }
    Some(run_state(true, &m.status))
}

/// 类型图标（ContainerView `kindIcon`）：resume ↻ / new ✦ / shell $
pub fn kind_icon(kind: TabKind) -> &'static str {
    match kind {
        TabKind::Resume => "↻",
        TabKind::New => "✦",
        TabKind::Shell => "$",
    }
}

#[cfg(test)]
pub(crate) fn meta(id: &str) -> SessionMeta {
    SessionMeta {
        session_id: id.into(),
        short_id: id.chars().take(8).collect(),
        cwd: "/Users/me/proj/sub".into(),
        last_cwd: String::new(),
        git_branch: "main".into(),
        user_msg_count: 1,
        first_user_msg: "帮我看看".into(),
        last_user_msg: String::new(),
        mtime: 0,
        mtime_display: String::new(),
        humanize: String::new(),
        is_worktree: false,
        git_root: "/Users/me/proj".into(),
        display_name: String::new(),
        name_source: String::new(),
        running: false,
        status: String::new(),
        waiting_for: String::new(),
        pid: 0,
        child_processes: vec![],
        archived: false,
        storage_folder: String::new(),
        pty_id: String::new(),
        tool: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(kind: TabKind, sid: Option<&str>, label: &str, cwd: &str) -> PaneTab {
        PaneTab {
            id: "t_1".into(),
            kind,
            cwd: cwd.into(),
            init_command: None,
            session_id: sid.map(String::from),
            session_short_id: None,
            label: label.into(),
        }
    }

    #[test]
    fn basename_vectors() {
        assert_eq!(basename("/a/b"), "b");
        assert_eq!(basename("/a/b/"), "b", "尾部斜杠要去掉");
        assert_eq!(basename("~"), "~");
        assert_eq!(basename("/"), "/", "只剩斜杠时原样返回");
    }

    #[test]
    fn titlebar_falls_back_step_by_step() {
        assert_eq!(titlebar_text(None, None), "makit", "没有标签");
        let m = meta("abc");
        let t = tab(TabKind::Resume, Some("abc"), "", "/x/y");
        assert_eq!(titlebar_text(Some(&t), Some(&m)), "proj · main", "项目名取 git 根目录");
        let mut m2 = meta("abc");
        m2.git_root = String::new();
        m2.git_branch = String::new();
        assert_eq!(titlebar_text(Some(&t), Some(&m2)), "sub", "没 git 根用 cwd；没分支不带 ·");
        assert_eq!(titlebar_text(Some(&t), None), "y", "查不到 meta → basename(tab.cwd)");
        let s = tab(TabKind::Shell, None, "", "/Users/me/code/");
        assert_eq!(titlebar_text(Some(&s), Some(&m)), "code", "没绑会话时不看 meta");
    }

    #[test]
    fn tab_title_falls_back_step_by_step() {
        let mut m = meta("abc");
        let t = tab(TabKind::Resume, Some("abc"), "[abc]", "/x");
        assert_eq!(tab_title(&t, Some(&m)), "帮我看看", "display_name 空 → 首条消息");
        m.display_name = "改名了".into();
        assert_eq!(tab_title(&t, Some(&m)), "改名了");
        assert_eq!(tab_title(&t, None), "[abc]", "查不到 meta → label");
        assert_eq!(tab_title(&tab(TabKind::Shell, None, "", "/Users/me/code"), None), "code · shell");
        assert_eq!(tab_title(&tab(TabKind::New, None, "新会话", "/x"), None), "新会话");
        assert_eq!(tab_title(&tab(TabKind::New, None, "", "/x"), None), "new", "最后回退到 kind");
    }

    #[test]
    fn status_only_when_running() {
        let t = tab(TabKind::Resume, Some("abc"), "", "/x");
        let mut m = meta("abc");
        assert_eq!(tab_status(&t, Some(&m)), None, "没在跑 → 没有点");
        m.running = true;
        m.status = "waiting".into();
        assert_eq!(tab_status(&t, Some(&m)), Some(RunState::Waiting));
        m.status = "busy".into();
        assert_eq!(tab_status(&t, Some(&m)), Some(RunState::Busy));
        m.status = "idle".into();
        assert_eq!(tab_status(&t, Some(&m)), Some(RunState::Idle));
        m.status = String::new();
        assert_eq!(tab_status(&t, Some(&m)), Some(RunState::Idle), "未知状态算 idle");
        assert_eq!(tab_status(&tab(TabKind::Shell, None, "", "/x"), Some(&m)), None, "没绑会话 → 没有点");
    }

    #[test]
    fn kind_icons_same_chars_as_tauri() {
        assert_eq!(kind_icon(TabKind::Resume), "↻");
        assert_eq!(kind_icon(TabKind::New), "✦");
        assert_eq!(kind_icon(TabKind::Shell), "$");
    }
}
