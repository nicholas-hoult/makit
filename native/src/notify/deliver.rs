//! 投递决策（纯函数，#215 第 3 节）+ 「正看着」判定（清单 G 节）。
//!
//! 为什么单独测：判错了在 UI 上就是「正看着的会话还弹横幅 / 铃铛挂着红点」，
//! 或反过来「瞥一眼没处理、走开之后再也不提醒」（Tauri 版 flashedRef 那次修的 bug）、
//! 「最大化挡住的 pane 被当成看得见」、「已完成也叮一声」。

use crate::persist::state::NotifyPrefs;
use crate::workspace::model::{find_container, WorkspaceState};

use super::model::Kind;

/// 一条信号该产生哪些效果
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Delivery {
    /// 进通知中心（每会话一条）
    pub record: bool,
    /// 记成未读（铃铛点、Dock 角标）
    pub unread: bool,
    /// 系统横幅
    pub banner: bool,
    /// 横幅带声音
    pub sound: bool,
    /// 窗口闪一下（正看着的会话进入等待）
    pub flash: bool,
}

/// `quiet` = 启动时第一次看到的状态（只进通知中心、不弹横幅，清单 G 节「启动后第一次轮询」、#215「重启不补发」）。
/// `has_body` = 有没有能放进横幅正文的东西（已完成没有 Claude 原话就不弹，不编造，照 对标产品）。
///
/// | kind | 正看着 | 没看着 |
/// |---|---|---|
/// | 审批 / 回答 | 只闪窗口，不进中心 | 中心 + 未读 + 横幅 + 响 |
/// | 已完成 | 什么都不做 | 中心 + 未读 + 横幅（静音，有正文才弹） |
/// | 出错 | 中心 + 未读，不弹 | 中心 + 未读 + 横幅 + 响（不受分类开关控制） |
///
/// 横幅还受总开关 `system` 管，声音受 `sound` 管。
pub fn deliver(kind: Kind, on_screen: bool, has_body: bool, quiet: bool, p: &NotifyPrefs) -> Delivery {
    let enabled = match kind {
        Kind::NeedsPermission => p.approval,
        Kind::NeedsInput => p.user,
        Kind::TurnComplete => p.completed,
        Kind::Error => true,
    };
    if !enabled {
        return Delivery::default();
    }
    if on_screen {
        return match kind {
            Kind::NeedsPermission | Kind::NeedsInput => Delivery { flash: true, ..Default::default() },
            Kind::TurnComplete => Delivery::default(),
            Kind::Error => Delivery { record: true, unread: true, ..Default::default() },
        };
    }
    let banner = p.system && !quiet && (has_body || kind != Kind::TurnComplete);
    let rings = kind != Kind::TurnComplete;
    Delivery { record: true, unread: true, banner, sound: banner && rings && p.sound, flash: false }
}

/// 「这个会话正摆在眼前」：窗口有焦点 + 是 active pane 的 active tab + 没被别的 pane 的最大化挡住（清单 G 节）
pub fn is_on_screen(ws: &WorkspaceState, window_active: bool, session_id: &str) -> bool {
    on_screen_session(ws, window_active) == Some(session_id)
}

/// 眼前那个会话（没有就 None）。窗口聚焦 / 切标签时拿它标已读
pub fn on_screen_session(ws: &WorkspaceState, window_active: bool) -> Option<&str> {
    if !window_active {
        return None;
    }
    if ws.maximized_container_id.as_deref().is_some_and(|m| m != ws.active_container_id) {
        return None;
    }
    let c = find_container(&ws.root, &ws.active_container_id)?;
    c.tabs.iter().find(|t| t.id == c.active_tab_id)?.session_id.as_deref()
}

/// 要发系统横幅时，按当前授权状态怎么办（同 Tauri useNotifications.ts 的 postSystem）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostPlan {
    /// 已授权：直接发
    Now,
    /// 没问过 / 还没查到：先弹系统授权框，授权了再补发这一条。系统对未授权的 app 直接丢横幅、不报错，
    /// 所以不问的话第一条通知就悄悄没了，用户还不知道要去设置里点「请求授权」
    AskThenPost,
    /// 被拒绝 / 没有 app 身份：发也没用（系统不会再弹框）；通知中心和角标照常
    Skip,
}

pub fn plan_post(p: super::system::Permission) -> PostPlan {
    use super::system::Permission::*;
    match p {
        Granted => PostPlan::Now,
        NotDetermined | Unknown => PostPlan::AskThenPost,
        Denied | Unavailable => PostPlan::Skip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{ContainerNode, Dir, LayoutNode, PaneTab, SplitNode, TabKind};

    fn prefs() -> NotifyPrefs {
        NotifyPrefs { system: true, approval: true, user: true, completed: true, sound: true }
    }
    fn d(record: bool, unread: bool, banner: bool, sound: bool, flash: bool) -> Delivery {
        Delivery { record, unread, banner, sound, flash }
    }

    #[test]
    fn needs_action_off_screen_rings_on_screen_only_flashes() {
        for k in [Kind::NeedsPermission, Kind::NeedsInput] {
            assert_eq!(deliver(k, false, true, false, &prefs()), d(true, true, true, true, false), "{k:?} 没看着");
            assert_eq!(deliver(k, true, true, false, &prefs()), d(false, false, false, false, true), "{k:?} 正看着：只闪");
        }
    }

    #[test]
    fn turn_complete_is_silent_and_skipped_when_watching_or_bodyless() {
        assert_eq!(deliver(Kind::TurnComplete, false, true, false, &prefs()), d(true, true, true, false, false), "静音横幅");
        assert_eq!(deliver(Kind::TurnComplete, false, false, false, &prefs()), d(true, true, false, false, false), "没正文不弹，但记未读");
        assert_eq!(deliver(Kind::TurnComplete, true, true, false, &prefs()), Delivery::default(), "正看着什么都不做");
    }

    #[test]
    fn error_always_records_and_ignores_kind_switches() {
        let off = NotifyPrefs { approval: false, user: false, completed: false, ..prefs() };
        assert_eq!(deliver(Kind::Error, false, true, false, &off), d(true, true, true, true, false));
        assert_eq!(deliver(Kind::Error, true, true, false, &off), d(true, true, false, false, false), "正看着：记未读不弹");
    }

    #[test]
    fn kind_switches() {
        let p = NotifyPrefs { approval: false, ..prefs() };
        assert_eq!(deliver(Kind::NeedsPermission, false, true, false, &p), Delivery::default());
        assert_eq!(deliver(Kind::NeedsPermission, true, true, false, &p), Delivery::default(), "关掉的连闪都不闪");
        let p = NotifyPrefs { user: false, ..prefs() };
        assert_eq!(deliver(Kind::NeedsInput, false, true, false, &p), Delivery::default(), "等待回答默认关");
        assert_eq!(deliver(Kind::NeedsPermission, false, true, false, &p).banner, true);
        let p = NotifyPrefs { completed: false, ..prefs() };
        assert_eq!(deliver(Kind::TurnComplete, false, true, false, &p), Delivery::default());
    }

    #[test]
    fn master_switch_sound_switch_and_quiet_start() {
        let p = NotifyPrefs { system: false, ..prefs() };
        assert_eq!(deliver(Kind::NeedsPermission, false, true, false, &p), d(true, true, false, false, false), "总开关只管横幅，中心照记");
        let p = NotifyPrefs { sound: false, ..prefs() };
        assert_eq!(deliver(Kind::NeedsPermission, false, true, false, &p), d(true, true, true, false, false));
        assert_eq!(deliver(Kind::NeedsPermission, false, true, true, &prefs()), d(true, true, false, false, false), "启动时只进中心");
        assert_eq!(deliver(Kind::Error, false, true, true, &prefs()), d(true, true, false, false, false));
    }

    fn tab(id: &str, sid: Option<&str>) -> PaneTab {
        PaneTab {
            id: id.into(),
            kind: TabKind::Resume,
            cwd: "/tmp".into(),
            init_command: None,
            session_id: sid.map(String::from),
            session_short_id: None,
            label: String::new(),
        }
    }
    fn container(id: &str, tabs: Vec<PaneTab>, active: &str) -> LayoutNode {
        LayoutNode::Container(ContainerNode { id: id.into(), tabs, active_tab_id: active.into(), tab_history: vec![] })
    }
    /// 左 c1（t1=s1 active, t2=s2），右 c2（t3=s3）
    fn ws(active: &str, max: Option<&str>) -> WorkspaceState {
        WorkspaceState {
            root: LayoutNode::Split(SplitNode {
                dir: Dir::V,
                ratio: 0.5,
                a: Box::new(container("c1", vec![tab("t1", Some("s1")), tab("t2", Some("s2"))], "t1")),
                b: Box::new(container("c2", vec![tab("t3", Some("s3")), tab("t4", None)], "t3")),
            }),
            active_container_id: active.into(),
            maximized_container_id: max.map(String::from),
        }
    }

    #[test]
    fn on_screen_needs_focus_active_pane_active_tab_and_not_hidden() {
        let w = ws("c1", None);
        assert!(is_on_screen(&w, true, "s1"));
        assert!(!is_on_screen(&w, false, "s1"), "窗口没焦点");
        assert!(!is_on_screen(&w, true, "s2"), "同 pane 的非 active tab");
        assert!(!is_on_screen(&w, true, "s3"), "别的 pane 的 active tab（看得见但不是焦点 pane，同 Tauri 版）");
        assert!(!is_on_screen(&w, true, "nope"));
        assert!(is_on_screen(&ws("c1", Some("c1")), true, "s1"), "自己最大化");
        assert!(!is_on_screen(&ws("c1", Some("c2")), true, "s1"), "被别的 pane 的最大化挡住");
        assert_eq!(on_screen_session(&w, true), Some("s1"));
        assert_eq!(on_screen_session(&ws("c1", Some("c2")), true), None);
        assert_eq!(on_screen_session(&w, false), None);
        let mut w2 = ws("c2", None);
        if let LayoutNode::Split(sp) = &mut w2.root {
            if let LayoutNode::Container(c) = sp.b.as_mut() {
                c.active_tab_id = "t4".into();
            }
        }
        assert_eq!(on_screen_session(&w2, true), None, "shell 标签没绑会话");
    }

    #[test]
    fn first_banner_asks_for_permission_instead_of_being_silently_dropped() {
        use super::super::system::Permission::*;
        assert_eq!(plan_post(Granted), PostPlan::Now);
        assert_eq!(plan_post(NotDetermined), PostPlan::AskThenPost, "没问过：先问再发");
        assert_eq!(plan_post(Unknown), PostPlan::AskThenPost, "状态还没查到：当作没问过");
        assert_eq!(plan_post(Denied), PostPlan::Skip, "拒绝过系统不会再弹框");
        assert_eq!(plan_post(Unavailable), PostPlan::Skip, "没有 app 身份发不了");
    }
}
