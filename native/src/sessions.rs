//! 侧栏的纯逻辑：标题、相对时间、运行状态、按状态 / 按项目分组（#221）。
//!
//! 为什么单独测：规则照搬 WebView 前端的 `sessionTitle.ts` / `relativeTime.ts` /
//! `sessionStatus.ts` / `sidebarGroups.ts`，两套界面要对同一份数据给出同样的分组和标题。
//! 错了在界面上的样子：会话掉进错的组（在跑的出现在「历史」里）、标题变成 `[短 id]`、
//! 时间显示成「200d」这类跟 WebView 版对不上的文字 —— 肉眼很难对出来。

/// 侧栏一行需要的字段。从后端的 `SessionMeta` 转过来，和它解耦是为了测试不依赖 Tauri。
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub cwd: String,
    /// 秒级 unix 时间
    pub mtime: i64,
    pub running: bool,
    /// 后端 hook 报的状态：`waiting` / `busy` / 其他
    pub status: String,
}

/// 状态点的四档，顺序即优先级：等你处理 > 正在跑 > 活着但闲着 > 已停止
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunState {
    Waiting,
    Busy,
    Idle,
    Stopped,
}

pub fn run_state(running: bool, status: &str) -> RunState {
    match status {
        "waiting" => RunState::Waiting,
        "busy" => RunState::Busy,
        _ if running => RunState::Idle,
        _ => RunState::Stopped,
    }
}

/// 压平空白：换行 / 制表符 / 连续空格收成单个空格
pub fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 显示名：display_name → 首条用户消息 → `[短 id]`（同 `deriveSessionTabLabel`）
pub fn session_title(display_name: &str, first_user_msg: &str, short_id: &str) -> String {
    if !display_name.trim().is_empty() {
        return flatten(display_name);
    }
    if !first_user_msg.trim().is_empty() {
        return flatten(first_user_msg);
    }
    format!("[{short_id}]")
}

/// 项目名：git 根目录（没有就用 cwd）的最后一段
pub fn project_name(git_root: &str, cwd: &str) -> String {
    let path = if git_root.is_empty() { cwd } else { git_root };
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if name.is_empty() { "其他".to_string() } else { name.to_string() }
}

/// 紧凑相对时间（同 `relativeTime.ts`）：刚刚 / 5m / 3h / 2d / 4mo / 1y
pub fn relative_time(mtime: i64, now: i64) -> String {
    let diff = now - mtime;
    if diff < 60 {
        "刚刚".to_string()
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else if diff < 86400 * 30 {
        format!("{}d", diff / 86400)
    } else if diff < 86400 * 365 {
        format!("{}mo", diff / (86400 * 30))
    } else {
        format!("{}y", diff / (86400 * 365))
    }
}

/// 一个分组：`rows` 是输入切片里的下标
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// 存折叠状态用，不随内容变
    pub id: String,
    pub label: String,
    pub rows: Vec<usize>,
}

/// 按状态分组。先到先得，一条会话只进它符合条件的最靠前那一组：
/// 已打开 → 需要回应 → 进行中 → 空闲 → 历史。空组不出现。
/// 已打开按 `opened` 里的顺序（标签顺序），其余按最近活动。
pub fn status_groups(rows: &[Row], opened: &[String]) -> Vec<Group> {
    let mut taken = vec![false; rows.len()];
    let mut by_recent: Vec<usize> = (0..rows.len()).collect();
    by_recent.sort_by(|&a, &b| rows[b].mtime.cmp(&rows[a].mtime));

    let mut groups = Vec::new();

    let mut opened_rows = Vec::new();
    for id in opened {
        if let Some(i) = rows.iter().position(|r| &r.session_id == id) {
            if !taken[i] {
                taken[i] = true;
                opened_rows.push(i);
            }
        }
    }
    if !opened_rows.is_empty() {
        groups.push(Group { id: "opened".into(), label: "已打开".into(), rows: opened_rows });
    }

    let chain: [(&str, &str, Option<RunState>); 4] = [
        ("attention", "需要回应", Some(RunState::Waiting)),
        ("busy", "进行中", Some(RunState::Busy)),
        ("idle", "空闲", Some(RunState::Idle)),
        ("history", "历史", None),
    ];
    for (id, label, want) in chain {
        let mut out = Vec::new();
        for &i in &by_recent {
            if taken[i] {
                continue;
            }
            let st = run_state(rows[i].running, &rows[i].status);
            if want.map_or(true, |w| w == st) {
                taken[i] = true;
                out.push(i);
            }
        }
        if !out.is_empty() {
            groups.push(Group { id: id.into(), label: label.into(), rows: out });
        }
    }
    groups
}

/// 按项目分组：组按组内最近活动排，组内按「需要回应 → 进行中 → 空闲 → 已停止」、同级按最近活动
pub fn project_groups(rows: &[Row]) -> Vec<Group> {
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        map.entry(r.project.clone())
            .or_insert_with(|| {
                order.push(r.project.clone());
                Vec::new()
            })
            .push(i);
    }
    let mut groups: Vec<Group> = order
        .into_iter()
        .map(|p| {
            let mut list = map.remove(&p).unwrap_or_default();
            list.sort_by(|&a, &b| {
                let (ra, rb) = (&rows[a], &rows[b]);
                run_state(ra.running, &ra.status)
                    .cmp(&run_state(rb.running, &rb.status))
                    .then(rb.mtime.cmp(&ra.mtime))
            });
            Group { id: format!("project:{p}"), label: p, rows: list }
        })
        .collect();
    let latest = |g: &Group| g.rows.iter().map(|&i| rows[i].mtime).max().unwrap_or(0);
    groups.sort_by(|a, b| latest(b).cmp(&latest(a)));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, project: &str, mtime: i64, running: bool, status: &str) -> Row {
        Row {
            session_id: id.into(),
            title: id.into(),
            project: project.into(),
            cwd: format!("/Users/me/{project}"),
            mtime,
            running,
            status: status.into(),
        }
    }

    #[test]
    fn run_state_prefers_status_over_running_flag() {
        assert_eq!(run_state(false, "waiting"), RunState::Waiting);
        assert_eq!(run_state(true, "busy"), RunState::Busy);
        assert_eq!(run_state(true, ""), RunState::Idle);
        assert_eq!(run_state(false, "idle"), RunState::Stopped);
    }

    #[test]
    fn title_falls_back_display_name_then_first_msg_then_short_id() {
        assert_eq!(session_title("改名了", "你好", "abcd"), "改名了");
        assert_eq!(session_title("  ", "帮我\n  看一下\t这个", "abcd"), "帮我 看一下 这个");
        assert_eq!(session_title("", " ", "abcd"), "[abcd]");
    }

    #[test]
    fn project_name_uses_git_root_then_cwd_last_segment() {
        assert_eq!(project_name("/Users/me/makit", "/Users/me/makit/src"), "makit");
        assert_eq!(project_name("", "/Users/me/tmp/"), "tmp");
        assert_eq!(project_name("", ""), "其他");
    }

    #[test]
    fn relative_time_is_compact() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now - 5, now), "刚刚");
        assert_eq!(relative_time(now - 300, now), "5m");
        assert_eq!(relative_time(now - 3 * 3600, now), "3h");
        assert_eq!(relative_time(now - 2 * 86400, now), "2d");
        assert_eq!(relative_time(now - 200 * 86400, now), "6mo");
        assert_eq!(relative_time(now - 800 * 86400, now), "2y");
    }

    #[test]
    fn status_groups_first_match_wins_and_skips_empty() {
        let rows = vec![
            row("old", "a", 10, false, ""),
            row("wait", "a", 20, true, "waiting"),
            row("busy", "b", 30, true, "busy"),
            row("idle-open", "b", 40, true, ""),
            row("new", "a", 50, false, ""),
        ];
        let g = status_groups(&rows, &["idle-open".to_string()]);
        let labels: Vec<&str> = g.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["已打开", "需要回应", "进行中", "历史"]);
        // 开着又在跑的只出现在「已打开」，不会重复出现在「空闲」
        assert_eq!(g[0].rows, vec![3]);
        // 历史按最近活动
        assert_eq!(g[3].rows, vec![4, 0]);
    }

    #[test]
    fn project_groups_sort_groups_by_latest_and_rows_by_priority() {
        let rows = vec![
            row("a-old", "a", 10, false, ""),
            row("b-new", "b", 100, false, ""),
            row("a-idle", "a", 5, true, ""),
            row("a-wait", "a", 1, true, "waiting"),
        ];
        let g = project_groups(&rows);
        assert_eq!(g.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(g[1].rows, vec![3, 2, 0]);
        assert_eq!(g[1].id, "project:a");
    }
}
