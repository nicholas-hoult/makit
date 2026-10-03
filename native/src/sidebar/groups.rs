//! 侧栏的纯逻辑：标题、相对时间、运行状态、过滤、排序、按状态 / 按项目分组、历史按日分段（#221 / #226）。
//!
//! 为什么单独测：规则照搬 WebView 前端的 `sessionTitle.ts` / `relativeTime.ts` / `sessionStatus.ts` /
//! `sidebarGroups.ts` / `SessionTree.tsx`（groupByGitRoot、搜索过滤、排序），两套界面要对同一份数据
//! 给出同样的分组和标题。错了在界面上的样子：会话掉进错的组（在跑的出现在「历史」里）、标题变成
//! `[短 id]`、昨天的会话出现在「今天」下面、时间显示成「200d」—— 肉眼很难对出来。
//! 测试向量逐条移植自 `scripts/test-sidebar-groups.ts`、`test-session-status.ts`、`test-session-title.ts`。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use makit_core::SessionMeta;

/// 状态的**唯一一套叫法**（`sessionStatus.ts` 的 STATUS_LABEL），侧栏和 ⌘K 都从这里取（#187）
pub mod status_label {
    pub const WAITING_APPROVAL: &str = "等待审批";
    pub const WAITING_USER: &str = "等待回答";
    pub const BUSY: &str = "进行中";
    pub const IDLE: &str = "空闲";
    pub const STOPPED: &str = "已停止";
    pub const ARCHIVED: &str = "已归档";
}

/// 在等你的会话具体在等什么：`waiting_for == "user"` 是等你回答问题，其余都是等你批准操作
pub fn waiting_label(waiting_for: &str) -> &'static str {
    if waiting_for == "user" { status_label::WAITING_USER } else { status_label::WAITING_APPROVAL }
}

/// 状态点的四档，顺序即优先级：等你处理 > 正在跑 > 活着但闲着 > 已停止
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

pub fn meta_state(m: &SessionMeta) -> RunState {
    run_state(m.running, &m.status)
}

impl RunState {
    /// 形状只表达进程生死：实心 = 还活着，空心 = 已停止（`runStateIcon`）
    pub fn icon(self) -> &'static str {
        if self == RunState::Stopped { "○" } else { "●" }
    }

    /// 状态点的 tooltip（`runStateTitle`），开头的词和分组名一致
    pub fn title(self) -> &'static str {
        match self {
            RunState::Waiting => "需要回应：在等你批准操作或回答问题",
            RunState::Busy => "进行中：进程活着且在产出",
            RunState::Idle => "空闲：进程活着，点进去可以直接接着用",
            RunState::Stopped => "已停止：进程不在了，打开会恢复之前的上下文",
        }
    }
}

/// 这一组里有没有活着的会话 —— 决定要不要给这组画状态点那一列（`anyAlive`）
pub fn any_alive<'a>(list: impl IntoIterator<Item = &'a SessionMeta>) -> bool {
    list.into_iter().any(|s| meta_state(s) != RunState::Stopped)
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

/// 项目名：git 根目录（没有就用 cwd）的最后一段；都没有叫「其他」
pub fn project_name(git_root: &str, cwd: &str) -> String {
    let path = if git_root.is_empty() { cwd } else { git_root };
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if name.is_empty() { "其他".to_string() } else { name.to_string() }
}

/// 同 SessionTree.tsx 的 `basename`：去掉结尾的 `/` 取最后一段，取不到就原样返回
pub fn basename(path: &str) -> String {
    let t = path.trim_end_matches('/');
    match t.rsplit('/').next() {
        Some(last) if !last.is_empty() => last.to_string(),
        _ => path.to_string(),
    }
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

// ---------- 过滤 / 排序 ----------

/// 搜索：大小写不敏感，匹配 display_name / short_id / first_user_msg / git_branch / git_root / cwd
/// （SessionTree.tsx 的 `filtered`）。`q` 已经 trim + 转小写，空串表示不过滤
pub fn matches_query(s: &SessionMeta, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    [&s.display_name, &s.short_id, &s.first_user_msg, &s.git_branch, &s.git_root, &s.cwd]
        .iter()
        .any(|f| f.to_lowercase().contains(q))
}

/// 过滤后的下标（保持输入顺序）：不显示已归档时去掉归档的，再按搜索词筛
pub fn filter_sessions(list: &[SessionMeta], query: &str, show_archived: bool) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    (0..list.len()).filter(|&i| (show_archived || !list[i].archived) && matches_query(&list[i], &q)).collect()
}

/// 定位（⌘L / 标签栏的定位按钮 / 通知跳转）的结果（#259）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealResult {
    /// 找到了并滚过去了
    Located,
    /// 找到了，但它是已归档的会话——为了让它出现，把「显示已归档」打开了
    LocatedArchived,
    /// 会话列表里根本没有这条（还没写出第一条消息、或已被清理）
    NotInList,
    /// 在列表里但最终没能在侧栏里画出来
    NotShown,
}

/// 定位前「显示已归档」该设成什么：目标是已归档的会话就必须打开，否则怎么都找不到它；
/// 其它情况仍然是关掉（保持 ⌘L 原来「清掉搜索和归档，露出当前会话」的行为）
pub fn reveal_show_archived(target_archived: bool) -> bool {
    target_archived
}

/// 定位结果对用户说的话。**只有真的定位到了才说「已定位」**：以前不管找没找到都弹，用户看到「已定位」却什么都没发生
pub fn reveal_message(r: RevealResult) -> &'static str {
    match r {
        RevealResult::Located => "已定位 session",
        RevealResult::LocatedArchived => "已定位（这条会话已归档，已显示归档的会话）",
        RevealResult::NotInList => "没有找到这条会话（可能还没有第一条消息，或已被清理）",
        RevealResult::NotShown => "没能在侧栏里定位到这条会话",
    }
}

/// 侧栏排序（`makit-tree-sort`）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// 最近活动（默认）
    Recent,
    /// 消息数多的在前
    Count,
    /// 首条消息字典序
    FirstMsg,
}

impl SortKey {
    pub fn parse(s: &str) -> Self {
        match s {
            "count" => SortKey::Count,
            "firstMsg" => SortKey::FirstMsg,
            _ => SortKey::Recent,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Recent => "recent",
            SortKey::Count => "count",
            SortKey::FirstMsg => "firstMsg",
        }
    }

    /// 同 `sortFn`。firstMsg 在 TS 版是 `localeCompare`（ICU 排序），这里按码点比 ——
    /// 中文开头的消息顺序可能和 WebView 版略有不同（已知差异，见 #226 报告）
    pub fn compare(self, a: &SessionMeta, b: &SessionMeta) -> Ordering {
        match self {
            SortKey::Recent => b.mtime.cmp(&a.mtime),
            SortKey::Count => b.user_msg_count.cmp(&a.user_msg_count),
            SortKey::FirstMsg => a.first_user_msg.cmp(&b.first_user_msg),
        }
    }
}

/// 项目组内的顺序：需要回应 → 进行中 → 空闲 → 已停止，同级按最近活动（`priorityCompare`）
pub fn priority_compare(a: &SessionMeta, b: &SessionMeta) -> Ordering {
    meta_state(a).cmp(&meta_state(b)).then(b.mtime.cmp(&a.mtime))
}

// ---------- 状态分组链 ----------

/// 按状态分组的结果，每组是 `list` 里的下标
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusGroups {
    pub pinned: Vec<usize>,
    pub opened: Vec<usize>,
    pub attention: Vec<usize>,
    pub busy: Vec<usize>,
    pub idle: Vec<usize>,
    pub history: Vec<usize>,
}

/// 按状态分组（`statusGroups`）。链条先到先得，一条会话只进它符合条件的最靠前那一组：
/// 置顶 → 已打开 → 需要回应 → 进行中 → 空闲 → 历史（已停止）。
/// - 置顶、历史尊重用户选的排序；已打开按屏幕上的标签顺序（`opened_index`）；其余固定按最近活动
/// - `candidates` 是参与分组的下标（过滤后的），顺序即 JS 里 `list` 的顺序（排序稳定，平手时保持它）
pub fn status_groups(
    list: &[SessionMeta],
    candidates: &[usize],
    opened_index: &HashMap<String, usize>,
    pinned: &HashSet<String>,
    sort: SortKey,
) -> StatusGroups {
    let mut taken: HashSet<usize> = HashSet::new();
    let mut take = |pred: &dyn Fn(&SessionMeta) -> bool, cmp: &dyn Fn(&SessionMeta, &SessionMeta) -> Ordering| {
        let mut out: Vec<usize> = candidates.iter().copied().filter(|i| !taken.contains(i) && pred(&list[*i])).collect();
        taken.extend(out.iter().copied());
        out.sort_by(|&a, &b| cmp(&list[a], &list[b]));
        out
    };
    let by_recent = |a: &SessionMeta, b: &SessionMeta| b.mtime.cmp(&a.mtime);
    let user_sort = |a: &SessionMeta, b: &SessionMeta| sort.compare(a, b);
    let opened_pos = |s: &SessionMeta| opened_index.get(&s.session_id).copied().unwrap_or(usize::MAX);
    StatusGroups {
        pinned: take(&|s| pinned.contains(&s.session_id), &user_sort),
        opened: take(&|s| opened_index.contains_key(&s.session_id), &|a, b| opened_pos(a).cmp(&opened_pos(b))),
        attention: take(&|s| meta_state(s) == RunState::Waiting, &by_recent),
        busy: take(&|s| meta_state(s) == RunState::Busy, &by_recent),
        idle: take(&|s| meta_state(s) == RunState::Idle, &by_recent),
        history: take(&|_| true, &user_sort),
    }
}

// ---------- 历史按日期分段 ----------

/// 今天、昨天之外，再单独列出几天（第 2~6 天）；第 7 天起并入「更早」。照 Claude 桌面版
const SINGLE_DAYS: i64 = 7;

#[derive(Clone, Debug, PartialEq)]
pub struct DayBucket {
    /// 存折叠状态用：day-today / day-yesterday / day-YYYY-M-D / day-older
    pub id: String,
    pub label: String,
    pub rows: Vec<usize>,
}

/// 公历日期 → 自 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）。
/// 按「日期」而不是按时间戳相减算天数差，夏令时那一小时天然不影响结果
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 秒级时间戳 → 本地日历日（年, 月, 日）
pub fn local_ymd(ts: i64) -> (i32, u32, u32) {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
}

/// 把历史按**本地日历日**分段（`dayBuckets`）：今天 / 昨天 / 近几天各一天（「9月22日」）/ 更早。
/// 段内保持传入顺序；空段不出现；段按日期新旧排。`ymd` 把时间戳换成本地日期（测试里换成固定时区）
pub fn day_buckets_with(list: &[SessionMeta], rows: &[usize], now: i64, ymd: impl Fn(i64) -> (i32, u32, u32)) -> Vec<DayBucket> {
    let (ny, nm, nd) = ymd(now);
    let today = days_from_civil(ny, nm, nd);
    let mut buckets: Vec<(DayBucket, i64)> = Vec::new();
    for &i in rows {
        let (y, m, d) = ymd(list[i].mtime);
        let diff = today - days_from_civil(y, m, d);
        let (id, label) = if diff <= 0 {
            ("day-today".to_string(), "今天".to_string())
        } else if diff == 1 {
            ("day-yesterday".to_string(), "昨天".to_string())
        } else if diff < SINGLE_DAYS {
            (format!("day-{y}-{m}-{d}"), format!("{m}月{d}日"))
        } else {
            ("day-older".to_string(), "更早".to_string())
        };
        match buckets.iter_mut().find(|(b, _)| b.id == id) {
            Some((b, newest)) => {
                b.rows.push(i);
                *newest = (*newest).max(list[i].mtime);
            }
            None => buckets.push((DayBucket { id, label, rows: vec![i] }, list[i].mtime)),
        }
    }
    let rank = |id: &str| match id {
        "day-today" => 0,
        "day-yesterday" => 1,
        "day-older" => 3,
        _ => 2,
    };
    buckets.sort_by(|(a, am), (b, bm)| rank(&a.id).cmp(&rank(&b.id)).then(bm.cmp(am)));
    buckets.into_iter().map(|(b, _)| b).collect()
}

pub fn day_buckets(list: &[SessionMeta], rows: &[usize], now: i64) -> Vec<DayBucket> {
    day_buckets_with(list, rows, now, local_ymd)
}

// ---------- 项目视图 ----------

/// 一个项目组（SessionTree.tsx 的 `groupByGitRoot`）
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectGroup {
    pub name: String,
    pub git_root: String,
    /// 组里有没有活着的会话：默认折不折、组排序、要不要画状态列都用它（`anyAlive`）
    pub has_active: bool,
    /// 组内按优先级排好的下标
    pub rows: Vec<usize>,
}

impl ProjectGroup {
    /// 折叠集合里的键（`projectCollapseKey`）
    pub fn key(&self) -> String {
        super::collapse::project_collapse_key(&self.git_root, &self.name)
    }

    /// 右键 / 「+」用的目录：仓库根，没有就用组里第一条的 cwd，再没有就 `~`
    pub fn cwd(&self, list: &[SessionMeta]) -> String {
        if !self.git_root.is_empty() {
            return self.git_root.clone();
        }
        self.rows.first().map(|&i| list[i].cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| "~".into())
    }
}

/// 按 git_root 分组：空 git_root 的全进一个组（名字取第一条的 cwd basename，再没有叫「其他」）；
/// 组内按优先级；组按「有活跃的在前，再按组内最新 mtime」排（排序稳定，平手保持首次出现的顺序）
pub fn project_groups(list: &[SessionMeta], rows: &[usize]) -> Vec<ProjectGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in rows {
        let key = list[i].git_root.clone();
        map.entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(i);
    }
    let mut groups: Vec<ProjectGroup> = order
        .into_iter()
        .map(|git_root| {
            let mut rows = map.remove(&git_root).unwrap_or_default();
            let name = if git_root.is_empty() {
                let first_cwd = rows.first().map(|&i| list[i].cwd.as_str()).filter(|c| !c.is_empty()).unwrap_or("其他");
                Some(basename(first_cwd)).filter(|n| !n.is_empty()).unwrap_or_else(|| "其他".into())
            } else {
                basename(&git_root)
            };
            let has_active = any_alive(rows.iter().map(|&i| &list[i]));
            rows.sort_by(|&a, &b| priority_compare(&list[a], &list[b]));
            ProjectGroup { name, git_root, has_active, rows }
        })
        .collect();
    let latest = |g: &ProjectGroup| g.rows.iter().map(|&i| list[i].mtime).max().unwrap_or(i64::MIN);
    groups.sort_by(|a, b| b.has_active.cmp(&a.has_active).then(latest(b).cmp(&latest(a))));
    groups
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::state::sessions::tests::session;

    /// 测试用会话：id、running、status、mtime
    pub fn mk(id: &str, running: bool, status: &str, mtime: i64) -> SessionMeta {
        SessionMeta { running, status: status.into(), mtime, ..session(id) }
    }

    fn ids(list: &[SessionMeta], rows: &[usize]) -> Vec<String> {
        rows.iter().map(|&i| list[i].session_id.clone()).collect()
    }

    fn all_rows(list: &[SessionMeta]) -> Vec<usize> {
        (0..list.len()).collect()
    }

    // ---- scripts/test-session-status.ts ----

    #[test]
    fn run_state_mapping_table() {
        let cases: [(bool, &str, RunState); 8] = [
            (true, "waiting", RunState::Waiting),
            (true, "busy", RunState::Busy),
            (true, "idle", RunState::Idle),
            (false, "idle", RunState::Stopped),
            (true, "", RunState::Idle),
            (false, "", RunState::Stopped),
            (false, "waiting", RunState::Waiting),
            (false, "busy", RunState::Busy),
        ];
        for (running, status, want) in cases {
            assert_eq!(run_state(running, status), want, "running={running} status={status:?}");
            // anyAlive 必须和 runState 的「非 stopped」严格等价
            assert_eq!(any_alive([&mk("x", running, status, 0)]), want != RunState::Stopped);
        }
    }

    #[test]
    fn icons_and_titles_are_distinct() {
        assert_eq!(RunState::Waiting.icon(), "●");
        assert_eq!(RunState::Busy.icon(), "●");
        assert_eq!(RunState::Idle.icon(), "●");
        assert_eq!(RunState::Stopped.icon(), "○");
        let all = [RunState::Waiting, RunState::Busy, RunState::Idle, RunState::Stopped];
        let titles: HashSet<&str> = all.iter().map(|s| s.title()).collect();
        assert_eq!(titles.len(), 4, "四档 tooltip 互不相同");
        assert!(all.iter().all(|s| s.title().chars().count() > 4));
    }

    #[test]
    fn any_alive_by_group() {
        assert!(!any_alive(std::iter::empty()));
        let dead = mk("d", false, "idle", 0);
        let live = mk("l", true, "idle", 0);
        assert!(!any_alive([&dead, &dead]));
        assert!(any_alive([&dead, &live, &dead]));
    }

    // ---- scripts/test-session-title.ts / relativeTime ----

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
        assert_eq!(basename("/Users/me/makit/"), "makit");
        assert_eq!(basename(""), "");
        assert_eq!(basename("/"), "/", "取不到最后一段就原样返回（同 TS）");
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

    // ---- scripts/test-sidebar-groups.ts ----

    #[test]
    fn status_chain_first_match_wins() {
        let all = vec![
            mk("wait", true, "waiting", 50),
            mk("busy", true, "busy", 40),
            mk("idle", true, "idle", 30),
            mk("dead", false, "", 20),
            mk("pinDead", false, "", 10),
            mk("pinBusy", true, "busy", 60),
            mk("openIdle", true, "idle", 5),
            mk("openPinned", false, "", 70),
        ];
        let opened: HashMap<String, usize> = [("openIdle".to_string(), 0), ("openPinned".to_string(), 1)].into();
        let pinned: HashSet<String> = ["pinDead", "pinBusy", "openPinned"].iter().map(|s| s.to_string()).collect();
        let g = status_groups(&all, &all_rows(&all), &opened, &pinned, SortKey::Recent);
        assert_eq!(ids(&all, &g.pinned), ["openPinned", "pinBusy", "pinDead"], "置顶在最上面，且拿走所有置顶的（哪怕在跑、哪怕开着）");
        assert_eq!(ids(&all, &g.opened), ["openIdle"], "已打开：置顶拿走的不再重复");
        assert_eq!(ids(&all, &g.attention), ["wait"]);
        assert_eq!(ids(&all, &g.busy), ["busy"]);
        assert_eq!(ids(&all, &g.idle), ["idle"]);
        assert_eq!(ids(&all, &g.history), ["dead"], "历史 = 剩下的已停止");
        let total = g.pinned.len() + g.opened.len() + g.attention.len() + g.busy.len() + g.idle.len() + g.history.len();
        assert_eq!(total, all.len(), "每条会话恰好出现一次");

        let two = vec![mk("a", true, "idle", 1), mk("b", true, "idle", 9)];
        let opened: HashMap<String, usize> = [("a".to_string(), 0), ("b".to_string(), 1)].into();
        let g2 = status_groups(&two, &all_rows(&two), &opened, &HashSet::new(), SortKey::Recent);
        assert_eq!(ids(&two, &g2.opened), ["a", "b"], "已打开按标签顺序，不按时间");
    }

    /// F0 原型的旧用例（已打开 → 需要回应 → 进行中 → 历史，空组不出现），换成新接口照样成立
    #[test]
    fn status_groups_old_prototype_case_still_holds() {
        let rows = vec![
            mk("old", false, "", 10),
            mk("wait", true, "waiting", 20),
            mk("busy", true, "busy", 30),
            mk("idle-open", true, "", 40),
            mk("new", false, "", 50),
        ];
        let opened: HashMap<String, usize> = [("idle-open".to_string(), 0)].into();
        let g = status_groups(&rows, &all_rows(&rows), &opened, &HashSet::new(), SortKey::Recent);
        assert_eq!(g.opened, vec![3], "开着又在跑的只出现在「已打开」，不会重复出现在「空闲」");
        assert!(g.idle.is_empty());
        assert_eq!(g.history, vec![4, 0], "历史按最近活动");
    }

    #[test]
    fn user_sort_applies_to_pinned_and_history_only() {
        let mut a = mk("a", false, "", 1);
        a.user_msg_count = 9;
        a.first_user_msg = "b 开头".into();
        let mut b = mk("b", false, "", 5);
        b.user_msg_count = 2;
        b.first_user_msg = "a 开头".into();
        let mut w1 = mk("w1", true, "waiting", 1);
        w1.user_msg_count = 99;
        let w2 = mk("w2", true, "waiting", 9);
        let list = vec![a, b, w1, w2];
        let g = status_groups(&list, &all_rows(&list), &HashMap::new(), &HashSet::new(), SortKey::Count);
        assert_eq!(ids(&list, &g.history), ["a", "b"], "按消息数");
        assert_eq!(ids(&list, &g.attention), ["w2", "w1"], "需要回应固定按最近活动，不管用户排序");
        let g = status_groups(&list, &all_rows(&list), &HashMap::new(), &HashSet::new(), SortKey::FirstMsg);
        assert_eq!(ids(&list, &g.history), ["b", "a"], "按首条消息");
    }

    /// 固定东八区（无夏令时）的「本地日期」，测试不依赖机器时区
    fn utc8(ts: i64) -> (i32, u32, u32) {
        let days = (ts + 8 * 3600).div_euclid(86400);
        // civil_from_days
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
    }

    /// 东八区本地时间 → 时间戳
    fn t(y: i32, mo: u32, d: u32, h: i64) -> i64 {
        days_from_civil(y, mo, d) * 86400 + h * 3600 - 8 * 3600
    }

    #[test]
    fn civil_roundtrip() {
        for ts in [0i64, 86399, 1_790_000_000, t(2026, 2, 28, 23), t(2024, 2, 29, 1)] {
            let (y, m, d) = utc8(ts);
            assert_eq!(days_from_civil(y, m, d), (ts + 8 * 3600).div_euclid(86400));
        }
        assert_eq!(utc8(t(2026, 9, 24, 10)), (2026, 9, 24));
    }

    #[test]
    fn day_buckets_by_local_calendar_day() {
        let now = t(2026, 9, 24, 10);
        let hist = vec![
            mk("today-early", false, "", t(2026, 9, 24, 0)),
            mk("yest-late", false, "", t(2026, 9, 23, 23)),
            mk("d2", false, "", t(2026, 9, 22, 12)),
            mk("d6", false, "", t(2026, 9, 18, 12)),
            mk("d7", false, "", t(2026, 9, 17, 12)),
            mk("lastMonth", false, "", t(2026, 8, 31, 12)),
        ];
        let b = day_buckets_with(&hist, &all_rows(&hist), now, utc8);
        let labels: Vec<&str> = b.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, ["今天", "昨天", "9月22日", "9月18日", "更早"]);
        assert_eq!(ids(&hist, &b[0].rows), ["today-early"]);
        assert_eq!(ids(&hist, &b[1].rows), ["yest-late"], "跨午夜：不到 24 小时但属于昨天");
        assert_eq!(ids(&hist, &b[4].rows), ["d7", "lastMonth"]);
        assert_eq!([b[0].id.as_str(), b[1].id.as_str(), b[4].id.as_str()], ["day-today", "day-yesterday", "day-older"], "组 id 稳定");
        assert_eq!(b[2].id, "day-2026-9-22");
        assert!(day_buckets_with(&hist, &[], now, utc8).is_empty(), "空段不出现");
        let x = vec![mk("x", false, "", t(2026, 9, 30, 12))];
        let b = day_buckets_with(&x, &[0], t(2026, 10, 1, 8), utc8);
        assert_eq!(b[0].label, "昨天", "跨月的昨天");
    }

    #[test]
    fn day_buckets_sorted_by_date_even_if_input_is_not() {
        let now = t(2026, 9, 24, 10);
        let l = vec![mk("old", false, "", t(2026, 1, 1, 1)), mk("d3", false, "", t(2026, 9, 21, 1)), mk("today", false, "", t(2026, 9, 24, 9)), mk("d2", false, "", t(2026, 9, 22, 1))];
        let b = day_buckets_with(&l, &all_rows(&l), now, utc8);
        assert_eq!(b.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(), ["今天", "9月22日", "9月21日", "更早"]);
    }

    #[test]
    fn priority_sort_in_project() {
        let p = vec![mk("s-old", false, "", 1), mk("i", true, "idle", 5), mk("s-new", false, "", 9), mk("w", true, "waiting", 2), mk("b", true, "busy", 3)];
        let mut r = all_rows(&p);
        r.sort_by(|&a, &b| priority_compare(&p[a], &p[b]));
        assert_eq!(ids(&p, &r), ["w", "b", "i", "s-new", "s-old"]);
    }

    #[test]
    fn status_vocabulary() {
        use status_label::*;
        assert_eq!([WAITING_APPROVAL, WAITING_USER, BUSY, IDLE, STOPPED, ARCHIVED], ["等待审批", "等待回答", "进行中", "空闲", "已停止", "已归档"]);
        assert_eq!(waiting_label("user"), "等待回答");
        assert_eq!(waiting_label("permission"), "等待审批");
        assert_eq!(waiting_label(""), "等待审批");
    }

    // ---- SessionTree.tsx：groupByGitRoot / 搜索过滤 ----

    fn at(id: &str, root: &str, cwd: &str, mtime: i64, running: bool, status: &str) -> SessionMeta {
        SessionMeta { git_root: root.into(), cwd: cwd.into(), ..mk(id, running, status, mtime) }
    }

    #[test]
    fn project_groups_active_first_then_latest() {
        let rows = vec![
            at("a-old", "/r/a", "/r/a", 10, false, ""),
            at("b-new", "/r/b", "/r/b", 100, false, ""),
            at("a-idle", "/r/a", "/r/a/x", 5, true, ""),
            at("a-wait", "/r/a", "/r/a", 1, true, "waiting"),
            at("c-new", "/r/c", "/r/c", 200, false, ""),
        ];
        let g = project_groups(&rows, &all_rows(&rows));
        assert_eq!(g.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["a", "c", "b"], "有活跃的在前，其余按组内最新");
        assert_eq!(ids(&rows, &g[0].rows), ["a-wait", "a-idle", "a-old"], "组内按优先级");
        assert!(g[0].has_active && !g[1].has_active);
        assert_eq!(g[0].key(), "/r/a");
        assert_eq!(g[0].cwd(&rows), "/r/a");
    }

    #[test]
    fn sessions_without_git_root_share_one_group() {
        let rows = vec![at("x", "", "/tmp/foo/", 3, false, ""), at("y", "", "/Users/me/bar", 2, false, ""), at("z", "", "", 1, false, "")];
        let g = project_groups(&rows, &all_rows(&rows));
        assert_eq!(g.len(), 1, "没有 git 仓库的全进一个组");
        assert_eq!(g[0].name, "foo", "组名取第一条的 cwd basename");
        assert_eq!(g[0].key(), "foo", "折叠键退到组名");
        assert_eq!(g[0].cwd(&rows), "/tmp/foo/");
        let only_empty = vec![at("z", "", "", 1, false, "")];
        assert_eq!(project_groups(&only_empty, &[0])[0].name, "其他");
        assert_eq!(project_groups(&only_empty, &[0])[0].cwd(&only_empty), "~");
    }

    #[test]
    fn search_is_case_insensitive_over_six_fields() {
        let mut s = mk("abcd1234", false, "", 0);
        s.display_name = "修 Bug".into();
        s.git_branch = "feat/Sidebar".into();
        s.cwd = "/Users/me/Proj".into();
        s.git_root = "/Users/me/Proj".into();
        s.first_user_msg = "你好".into();
        let list = vec![s, SessionMeta { archived: true, ..mk("arch", false, "", 0) }];
        assert_eq!(filter_sessions(&list, "bug", false), vec![0]);
        assert_eq!(filter_sessions(&list, "SIDEBAR", false), vec![0]);
        assert_eq!(filter_sessions(&list, " proj ", false), vec![0], "前后空白不算");
        assert_eq!(filter_sessions(&list, "ABCD", false), vec![0], "短 id");
        assert_eq!(filter_sessions(&list, "你", false), vec![0]);
        assert!(filter_sessions(&list, "nope", false).is_empty());
        assert_eq!(filter_sessions(&list, "", false), vec![0], "默认不显示已归档");
        assert_eq!(filter_sessions(&list, "", true), vec![0, 1]);
    }

    #[test]
    fn sort_key_parse_roundtrip() {
        for k in [SortKey::Recent, SortKey::Count, SortKey::FirstMsg] {
            assert_eq!(SortKey::parse(k.as_str()), k);
        }
        assert_eq!(SortKey::parse("乱写"), SortKey::Recent);
    }

    /// 为什么要测（#259）：定位说「已定位」却什么都没发生，是用户最直接看到的「定位有问题」
    #[test]
    fn reveal_message_only_says_located_when_it_really_located() {
        assert_eq!(reveal_message(RevealResult::Located), "已定位 session");
        assert!(reveal_message(RevealResult::LocatedArchived).contains("已归档"), "要说清为什么多出了归档的会话");
        for r in [RevealResult::NotInList, RevealResult::NotShown] {
            let m = reveal_message(r);
            assert!(!m.contains("已定位"), "没找到不能说已定位：{m}");
            assert!(m.contains("没有找到") || m.contains("没能"), "{m}");
        }
    }

    #[test]
    fn archived_target_turns_show_archived_on() {
        assert!(reveal_show_archived(true), "目标是已归档的：不打开就找不到它");
        assert!(!reveal_show_archived(false), "别的情况照旧：关掉");
    }
}
