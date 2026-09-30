//! 会话列表的增量合并（照搬 `src/running-merge.ts` 和 App.tsx 里 `sessions-changed` 的合并）。
//!
//! 数据流：core 的 watcher 报 `RunningChanged` → `list_running_sessions` → `merge_running`；
//! 报 `SessionsChanged(路径)` → `list_sessions_by_paths` → `merge_updated`。全量（启动 / ⌘R）直接替换。
//!
//! 为什么单独测：改名只触发 running-changed（写的是 `~/.claude/sessions/<pid>.json`，不写 jsonl），
//! 这条路搬不动标题的话改名要 ⌘R 才生效（#4 / #8）；而「没变化也造新数据」会让侧栏白重绘（#219）。

use makit_core::{RunningMeta, SessionMeta};
use std::collections::HashMap;

/// 把一条运行状态合进一条会话。返回是否有变化。
///
/// 已知边界（同 TS 版）：名字被清空 / 进程退出时**不动** display_name —— 回落目标
/// （customTitle / agentName）没随 SessionMeta 下发，硬清会用新错误换旧错误。
pub fn apply_running_meta(s: &mut SessionMeta, r: Option<&RunningMeta>) -> bool {
    let Some(r) = r else {
        if !s.running {
            return false;
        }
        s.running = false;
        s.status = "idle".into();
        s.waiting_for.clear();
        s.pid = 0;
        return true;
    };
    let renamed = !r.name.is_empty() && (s.display_name != r.name || s.name_source != "rename");
    if s.running && s.status == r.status && s.waiting_for == r.waiting_for && s.pid == r.pid && !renamed {
        return false;
    }
    s.running = true;
    s.status = r.status.clone();
    s.waiting_for = r.waiting_for.clone();
    s.pid = r.pid;
    if !r.name.is_empty() {
        s.display_name = r.name.clone();
        s.name_source = "rename".into();
    }
    true
}

/// 一次 running-changed 合进整个列表。返回是否有任何一条变了（没变就不必通知视图）。
/// `skip` 为真的会话不合并（归档写盘中的那几条）。
pub fn merge_running(list: &mut [SessionMeta], running: &[RunningMeta], skip: impl Fn(&SessionMeta) -> bool) -> bool {
    let map: HashMap<&str, &RunningMeta> = running.iter().map(|r| (r.session_id.as_str(), r)).collect();
    let mut changed = false;
    for s in list.iter_mut() {
        if skip(s) {
            continue;
        }
        changed |= apply_running_meta(s, map.get(s.session_id.as_str()).copied());
    }
    changed
}

/// 增量解析的结果合进列表：按 session_id 替换或新增；子进程列表沿用旧值（增量命令没跑进程全表）；
/// 最后按 mtime 重排（最近在前）。返回是否有变化。
pub fn merge_updated(list: &mut Vec<SessionMeta>, updated: Vec<SessionMeta>, skip: impl Fn(&str) -> bool) -> bool {
    let mut changed = false;
    for mut u in updated {
        if skip(&u.session_id) {
            continue;
        }
        match list.iter_mut().find(|s| s.session_id == u.session_id) {
            Some(old) => {
                u.child_processes = std::mem::take(&mut old.child_processes);
                *old = u;
            }
            None => list.push(u),
        }
        changed = true;
    }
    if changed {
        list.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    }
    changed
}

/// 全量扫描失败时怎么办（同 Tauri App.tsx `load()` 的 catch）：**保留原来的列表**，不能拿一次
/// 失败的扫描把用户已经看到的会话清空；`error` 进 `load_error`，界面照它显示「加载失败」横幅
pub fn apply_full_scan(current: &mut Vec<SessionMeta>, result: Result<Vec<SessionMeta>, String>) -> Option<String> {
    match result {
        Ok(list) => {
            *current = list;
            None
        }
        Err(e) => Some(e),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn session(id: &str) -> SessionMeta {
        SessionMeta {
            session_id: id.into(),
            short_id: id.into(),
            cwd: "/p".into(),
            last_cwd: "/p".into(),
            git_branch: String::new(),
            user_msg_count: 1,
            first_user_msg: "build当前项目".into(),
            last_user_msg: String::new(),
            mtime: 0,
            mtime_display: String::new(),
            humanize: String::new(),
            is_worktree: false,
            git_root: "/p".into(),
            display_name: String::new(),
            name_source: String::new(),
            running: false,
            status: "idle".into(),
            waiting_for: String::new(),
            pid: 0,
            child_processes: Vec::new(),
            archived: false,
            storage_folder: String::new(),
            pty_id: String::new(),
            tool: "claude".into(),
        }
    }

    fn meta(status: &str, pid: u32, name: &str) -> RunningMeta {
        RunningMeta { session_id: "s1".into(), status: status.into(), waiting_for: String::new(), pid, name: name.into() }
    }

    // ---- scripts/test-running-merge.ts ----

    #[test]
    fn rename_takes_effect_without_rescan() {
        let mut s = SessionMeta { display_name: "旧名".into(), name_source: "custom-title".into(), ..session("s1") };
        assert!(apply_running_meta(&mut s, Some(&meta("busy", 4242, "新名"))));
        assert_eq!((s.display_name.as_str(), s.name_source.as_str()), ("新名", "rename"));

        let mut s = SessionMeta { display_name: "oc-install-pack脚本编写".into(), name_source: "custom-title".into(), ..session("s1") };
        apply_running_meta(&mut s, Some(&meta("busy", 4242, "")));
        assert_eq!((s.display_name.as_str(), s.name_source.as_str()), ("oc-install-pack脚本编写", "custom-title"), "名字为空时不动");
    }

    #[test]
    fn running_fields_are_carried_and_cleared() {
        let mut s = session("s1");
        let r = RunningMeta { waiting_for: "permission".into(), ..meta("waiting", 999, "") };
        apply_running_meta(&mut s, Some(&r));
        assert!(s.running);
        assert_eq!((s.status.as_str(), s.waiting_for.as_str(), s.pid), ("waiting", "permission", 999));

        let mut s = SessionMeta { running: true, status: "busy".into(), pid: 123, display_name: "旧名".into(), name_source: "rename".into(), ..session("s1") };
        assert!(apply_running_meta(&mut s, None));
        assert!(!s.running);
        assert_eq!((s.status.as_str(), s.pid, s.display_name.as_str()), ("idle", 0, "旧名"), "进程没了：状态清干净，标题故意不动");
    }

    #[test]
    fn unchanged_reports_no_change() {
        let mut s = session("s1");
        assert!(!apply_running_meta(&mut s, None), "本来就没跑、这轮也不在");
        let r = meta("busy", 7, "");
        let mut s = SessionMeta { running: true, status: "busy".into(), pid: 7, ..session("s1") };
        assert!(!apply_running_meta(&mut s, Some(&r)), "运行中且无变化");
        let mut s = SessionMeta { running: true, status: "busy".into(), pid: 7, display_name: "新名".into(), name_source: "rename".into(), ..session("s1") };
        assert!(!apply_running_meta(&mut s, Some(&meta("busy", 7, "新名"))), "改名已生效过");
        assert!(apply_running_meta(&mut s, Some(&meta("waiting", 7, "新名"))), "状态变了");
    }

    #[test]
    fn merge_list_and_skip() {
        let a = SessionMeta { running: true, status: "busy".into(), pid: 1, ..session("a") };
        let b = session("b");
        let mut list = vec![a, b];
        let r = RunningMeta { session_id: "a".into(), status: "busy".into(), waiting_for: String::new(), pid: 1, name: String::new() };
        assert!(!merge_running(&mut list, &[r.clone()], |_| false), "列表无变化");
        let w = RunningMeta { status: "waiting".into(), ..r };
        assert!(!merge_running(&mut list, &[w.clone()], |s| s.session_id == "a"), "被跳过的不合并");
        assert!(merge_running(&mut list, &[w], |_| false));
        assert_eq!(list[0].status, "waiting");
    }

    #[test]
    fn merge_updated_keeps_children_and_sorts_by_mtime() {
        let mut old = SessionMeta { mtime: 10, ..session("a") };
        old.child_processes = vec![makit_core::ProcessInfo { pid: 5, ppid: 1, command: "node".into() }];
        let mut list = vec![SessionMeta { mtime: 20, ..session("b") }, old];
        let upd = vec![SessionMeta { mtime: 30, user_msg_count: 9, ..session("a") }, SessionMeta { mtime: 25, ..session("new") }];
        assert!(merge_updated(&mut list, upd, |_| false));
        let ids: Vec<&str> = list.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["a", "new", "b"], "最近在前");
        assert_eq!(list[0].user_msg_count, 9);
        assert_eq!(list[0].child_processes.len(), 1, "子进程沿用旧值");
        assert!(!merge_updated(&mut list, vec![SessionMeta { mtime: 99, ..session("a") }], |id| id == "a"), "归档中的跳过");
    }

    #[test]
    fn full_scan_failure_keeps_old_list_and_reports_error() {
        let mut list = vec![session("a"), session("b")];
        let err = apply_full_scan(&mut list, Err("拒绝访问 ~/.claude/projects".into()));
        assert_eq!(err.as_deref(), Some("拒绝访问 ~/.claude/projects"));
        assert_eq!(list.len(), 2, "失败不能把已经看到的会话清空");
    }

    #[test]
    fn full_scan_success_replaces_list_and_clears_error() {
        let mut list = vec![session("a")];
        let err = apply_full_scan(&mut list, Ok(vec![session("b"), session("c")]));
        assert_eq!(err, None);
        assert_eq!(list.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    }
}
