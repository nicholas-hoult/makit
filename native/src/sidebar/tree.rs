//! 侧栏拍平成一维列表（#219 TRD 的设计，对标 VS Code listView）+ 键盘导航 + 虚拟列表的几何计算。
//!
//! - **拍平**：置顶 / 已打开 / 状态组 / 历史日期段 / 项目组 → `Vec<Item>`（组头、会话行、间距、空状态）。
//!   折叠的组只出组头。这个数组**同时是 ↑↓ 导航的顺序**（替代 treeOrder.ts 的 visibleSessionOrder，
//!   渲染和导航用同一份，漂不了）。分页（RECENT_PAGE / pageProject）虚拟化之后不需要，去掉了。
//! - **行高按类型固定**（照 SessionTree.css 的数值算出来的常量，见各常量注释），虚拟列表的 top 表、
//!   ⌘L 居中、↑↓ 贴边滚动都靠它算，不依赖「已经量过的行」。
//!
//! 为什么单独测：折叠是唯一会让「有哪些会话」≠「渲染了哪些行」的功能，而键盘导航的顺序严格是后者；
//! 忘了摘掉折叠组，↑↓ 就走到不存在的行上、选中框凭空消失。用例移植自 `scripts/test-tree-order.ts`、
//! `scripts/test-tree-nav.ts`（moveSelection 部分），外加几何（居中 / 贴边 / 增量 splice）的边界。

use std::collections::HashMap;
use std::ops::Range;

/// `.tree-body { padding: 4px 0 }`
pub const PAD_H: f32 = 4.0;
/// `.tree-group + .tree-group` / `.tree-group + .tree-section` 的 margin-top
pub const GROUP_GAP: f32 = 10.0;
/// `.tree-section { margin-bottom: 2px }`（项目组之间）
pub const SECTION_GAP: f32 = 2.0;
/// 会话行：padding 5 + 标题行 17（12px 字，WebKit 实测行盒 17px）+ 2 + 元信息行 14（10px 字）+ 5
pub const SESSION_H: f32 = 43.0;
/// 组标签 `.tree-group-label`：padding 2 / 3 + 10px 字行盒 14
pub const GROUP_LABEL_H: f32 = 19.0;
/// 项目头 `.tree-project-header`：padding 5 + 「+」按钮 16（比 11px 字的行盒高）+ 5
pub const PROJECT_HEADER_H: f32 = 26.0;
/// 空状态 `.tree-empty`：padding 20 + 12px 字 17 + 20
pub const EMPTY_H: f32 = 57.0;

/// 一条会话行属于哪个可折叠的组（←/→ 折叠 / 展开、⌘L 展开都靠它）
#[derive(Clone, Debug, PartialEq)]
pub enum GroupRef {
    /// 状态视图里带标题的组（置顶 / 已打开 / 需要回应 / 进行中 / 空闲 / 日期段）
    Status(String),
    /// 项目组
    Project { key: String, has_active: bool },
    /// 不可折叠（不分段时那个没有标题的历史组）
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// 空白（上下内边距、组间距）
    Space(f32),
    GroupHeader { id: String, label: String, count: usize, collapsed: bool, warn: bool },
    ProjectHeader { key: String, name: String, cwd: String, count: usize, collapsed: bool, has_active: bool },
    /// `row` 是会话列表里的下标；`show_status` 按组给（整组都停止时不画状态点）
    Session { row: usize, show_status: bool },
    Empty(String),
}

impl Item {
    pub fn height(&self) -> f32 {
        match self {
            Item::Space(h) => *h,
            Item::GroupHeader { .. } => GROUP_LABEL_H,
            Item::ProjectHeader { .. } => PROJECT_HEADER_H,
            Item::Session { .. } => SESSION_H,
            Item::Empty(_) => EMPTY_H,
        }
    }
}

/// 一个状态视图的组。`label` 为空 = 没有表头、不可折叠（不分段的历史组）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeGroup {
    pub id: String,
    pub label: String,
    pub warn: bool,
    pub rows: Vec<usize>,
}

/// 一个项目组（折没折由调用方按 collapse.rs 的编码算好传进来）
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeProject {
    pub key: String,
    pub name: String,
    pub cwd: String,
    pub has_active: bool,
    pub collapsed: bool,
    pub rows: Vec<usize>,
}

pub struct TreeInput<'a> {
    /// 两个视图都常驻最上面的组：置顶、已打开
    pub top: Vec<TreeGroup>,
    pub project_view: bool,
    /// 状态视图：需要回应 / 进行中 / 空闲
    pub labeled: Vec<TreeGroup>,
    /// 状态视图：历史（按日期分好的段，或一个 id 为 "history"、没有标题的组）
    pub history: Vec<TreeGroup>,
    /// 项目视图的项目组
    pub projects: Vec<TreeProject>,
    /// 状态组的折叠集合（`makit-group-collapsed`，单侧编码：在里面 = 折叠）
    pub collapsed_groups: &'a [String],
    /// 过滤后还剩几条（状态视图的空状态看它）
    pub filtered_count: usize,
    /// 空状态文案：加载中… / 无匹配 session / 无 session
    pub empty_text: String,
    /// 这条会话活着吗（决定组要不要画状态列）
    pub alive: &'a dyn Fn(usize) -> bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub items: Vec<Item>,
    /// 渲染出来的会话行，按从上到下的顺序（↑↓ 导航的顺序）
    pub order: Vec<usize>,
    /// 每条会话（含折叠组里没渲染的）属于哪个组
    pub membership: HashMap<usize, GroupRef>,
}

impl Tree {
    /// 会话行在 items 里的位置
    pub fn item_of_row(&self, row: usize) -> Option<usize> {
        self.items.iter().position(|i| matches!(i, Item::Session { row: r, .. } if *r == row))
    }
}

/// 拍平（同 SessionTree.tsx 的渲染结构 + treeOrder.ts 的顺序规则）
pub fn flatten(input: &TreeInput) -> Tree {
    #[derive(PartialEq)]
    enum Prev {
        Nothing,
        Group,
        Project,
    }
    let mut t = Tree { items: vec![Item::Space(PAD_H)], ..Default::default() };
    let mut prev = Prev::Nothing;
    let collapsed_groups = input.collapsed_groups;

    // 一组 = 可选的组标签 + 会话行；组之间 10px（`.tree-group + .tree-group`）。空组整块不渲染
    let mut push_group = |t: &mut Tree, prev: &mut Prev, g: &TreeGroup| {
        let collapsible = !g.id.is_empty() && !g.label.is_empty();
        let gref = if collapsible { GroupRef::Status(g.id.clone()) } else { GroupRef::None };
        for &row in &g.rows {
            t.membership.insert(row, gref.clone());
        }
        if g.rows.is_empty() {
            return;
        }
        if *prev != Prev::Nothing {
            t.items.push(Item::Space(GROUP_GAP));
        }
        *prev = Prev::Group;
        let collapsed = collapsible && collapsed_groups.iter().any(|c| *c == g.id);
        if !g.label.is_empty() {
            t.items.push(Item::GroupHeader { id: g.id.clone(), label: g.label.clone(), count: g.rows.len(), collapsed, warn: g.warn });
        }
        if !collapsed {
            let show_status = g.rows.iter().any(|&r| (input.alive)(r));
            for &row in &g.rows {
                t.items.push(Item::Session { row, show_status });
                t.order.push(row);
            }
        }
    };

    for g in &input.top {
        push_group(&mut t, &mut prev, g);
    }
    if !input.project_view {
        for g in input.labeled.iter().chain(input.history.iter()) {
            push_group(&mut t, &mut prev, g);
        }
        if input.filtered_count == 0 {
            t.items.push(Item::Empty(input.empty_text.clone()));
        }
    } else {
        for p in &input.projects {
            let gref = GroupRef::Project { key: p.key.clone(), has_active: p.has_active };
            for &row in &p.rows {
                t.membership.insert(row, gref.clone());
            }
            // `.tree-group + .tree-section` 10px；项目组之间靠上一组的 margin-bottom 2px
            if prev == Prev::Group {
                t.items.push(Item::Space(GROUP_GAP));
            }
            prev = Prev::Project;
            t.items.push(Item::ProjectHeader {
                key: p.key.clone(),
                name: p.name.clone(),
                cwd: p.cwd.clone(),
                count: p.rows.len(),
                collapsed: p.collapsed,
                has_active: p.has_active,
            });
            if !p.collapsed {
                for &row in &p.rows {
                    // hasActive 就是 anyAlive(组)，整组一起画状态列
                    t.items.push(Item::Session { row, show_status: p.has_active });
                    t.order.push(row);
                }
            }
            t.items.push(Item::Space(SECTION_GAP));
        }
        let top_empty = input.top.iter().all(|g| g.rows.is_empty());
        if input.projects.is_empty() && top_empty {
            t.items.push(Item::Empty(input.empty_text.clone()));
        }
    }
    t.items.push(Item::Space(PAD_H));
    t
}

/// 在渲染出来的行里移动选中（`moveSelection`）：空 → None；没选中或选中项已不在 → 第一条（两个方向都是）；
/// 到首尾停住不循环
pub fn move_selection(order: &[usize], current: Option<usize>, down: bool) -> Option<usize> {
    if order.is_empty() {
        return None;
    }
    let Some(i) = current.and_then(|c| order.iter().position(|&x| x == c)) else { return Some(order[0]) };
    let next = if down { i + 1 } else { i.wrapping_sub(1) };
    Some(order.get(next).copied().unwrap_or(order[i]))
}

// ---------- 几何 ----------

/// 每项的 top（前缀和），长度 n+1，最后一个是总高
pub fn tops(items: &[Item]) -> Vec<f32> {
    let mut out = Vec::with_capacity(items.len() + 1);
    let mut y = 0.0;
    out.push(y);
    for it in items {
        y += it.height();
        out.push(y);
    }
    out
}

/// ⌘L：把第 ix 项滚到视口正中，夹在 [0, 总高 - 视口高]
pub fn center_scroll_top(tops: &[f32], ix: usize, viewport_h: f32) -> f32 {
    let total = tops.last().copied().unwrap_or(0.0);
    let (top, bottom) = (tops[ix.min(tops.len() - 1)], tops[(ix + 1).min(tops.len() - 1)]);
    let goal = (top + bottom) / 2.0 - viewport_h / 2.0;
    goal.min(total - viewport_h).max(0.0)
}

/// ↑↓：第 ix 项在视口上方 → 滚到它的顶；在下方 → 滚到它的底贴视口底；已完整可见 → None（同 block: nearest）
pub fn reveal_nearest(tops: &[f32], ix: usize, scroll_top: f32, viewport_h: f32) -> Option<f32> {
    let (top, bottom) = (*tops.get(ix)?, *tops.get(ix + 1)?);
    if top < scroll_top {
        Some(top)
    } else if bottom > scroll_top + viewport_h {
        Some((bottom - viewport_h).max(0.0))
    } else {
        None
    }
}

/// 像素偏移 → (第几项, 项内偏移)。GPUI `ListState::scroll_to` 要这个形状
pub fn offset_to_item(tops: &[f32], y: f32) -> (usize, f32) {
    let n = tops.len().saturating_sub(1);
    if n == 0 {
        return (0, 0.0);
    }
    // 最后一个 top <= y 的项（tops 单调不减；partition_point 找第一个 > y 的位置）
    let ix = tops[..n].partition_point(|&t| t <= y).saturating_sub(1);
    (ix, y - tops[ix])
}

/// 数据变了之后要告诉虚拟列表哪一段换了：返回 (旧区间, 新条数)。只比行高 —— 虚拟列表只缓存每项的高度，
/// 内容每帧都重画；前后相同的段不动，已经量过的行和滚动位置都保住。完全一样返回 None
pub fn splice_plan(old: &[f32], new: &[f32]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old.iter().rev().zip(new.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

/// 侧栏宽度范围（`makit-project-list-width`，180–480）
pub fn clamp_width(w: f32) -> f32 {
    w.clamp(180.0, 480.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 会话名 → 下标：测试里用名字读写，flatten 只认下标
    const NAMES: &[&str] = &["o1", "o2", "a1", "r1", "r2", "p1", "h1", "h2", "x1", "x2", "y1", "p0", "t1", "e1", "e2", "z1"];
    fn r(name: &str) -> usize {
        NAMES.iter().position(|n| *n == name).expect("测试名字写错了")
    }
    fn rows(names: &[&str]) -> Vec<usize> {
        names.iter().map(|n| r(n)).collect()
    }
    fn names(order: &[usize]) -> Vec<&'static str> {
        order.iter().map(|&i| NAMES[i]).collect()
    }
    fn g(id: &str, list: &[&str]) -> TreeGroup {
        TreeGroup { id: id.into(), label: format!("组{id}"), warn: false, rows: rows(list) }
    }
    fn history_plain(list: &[&str]) -> TreeGroup {
        TreeGroup { id: "history".into(), label: String::new(), warn: false, rows: rows(list) }
    }
    fn proj(key: &str, collapsed: bool, list: &[&str]) -> TreeProject {
        TreeProject { key: key.into(), name: key.into(), cwd: format!("/{key}"), has_active: false, collapsed, rows: rows(list) }
    }

    struct Case {
        top: Vec<TreeGroup>,
        project_view: bool,
        labeled: Vec<TreeGroup>,
        history: Vec<TreeGroup>,
        projects: Vec<TreeProject>,
        collapsed: Vec<String>,
    }

    impl Default for Case {
        fn default() -> Self {
            Self {
                top: vec![g("opened", &["o1", "o2"])],
                project_view: false,
                labeled: vec![g("attention", &["a1"]), g("running", &["r1", "r2"]), g("pinned", &["p1"])],
                history: vec![history_plain(&["h1", "h2"])],
                projects: vec![proj("x", false, &["x1", "x2"]), proj("y", true, &["y1"])],
                collapsed: vec![],
            }
        }
    }

    fn build(c: Case) -> Tree {
        let alive = |_: usize| false;
        flatten(&TreeInput {
            top: c.top,
            project_view: c.project_view,
            labeled: c.labeled,
            history: c.history,
            projects: c.projects,
            collapsed_groups: &c.collapsed,
            filtered_count: 1,
            empty_text: "无 session".into(),
            alive: &alive,
        })
    }
    fn order(c: Case) -> Vec<&'static str> {
        names(&build(c).order)
    }
    fn col(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    // ---- scripts/test-tree-order.ts ----

    #[test]
    fn baseline_orders() {
        assert_eq!(order(Case::default()), ["o1", "o2", "a1", "r1", "r2", "p1", "h1", "h2"], "状态视图基线顺序");
        assert_eq!(order(Case { project_view: true, ..Case::default() }), ["o1", "o2", "x1", "x2"], "项目视图基线顺序");
    }

    #[test]
    fn collapsed_status_groups_leave_the_order() {
        assert_eq!(order(Case { collapsed: col(&["running"]), ..Case::default() }), ["o1", "o2", "a1", "p1", "h1", "h2"]);
        assert_eq!(order(Case { collapsed: col(&["opened"]), ..Case::default() }), ["a1", "r1", "r2", "p1", "h1", "h2"], "折叠「已打开」（状态视图）");
        assert_eq!(order(Case { project_view: true, collapsed: col(&["opened"]), ..Case::default() }), ["x1", "x2"], "折叠「已打开」（项目视图）");
        assert_eq!(order(Case { collapsed: col(&["opened", "attention", "running", "pinned"]), ..Case::default() }), ["h1", "h2"], "历史组仍在");
        assert_eq!(order(Case { collapsed: col(&["history"]), ..Case::default() }), ["o1", "o2", "a1", "r1", "r2", "p1", "h1", "h2"], "\"history\" 不影响历史组");
    }

    #[test]
    fn two_collapse_sets_do_not_interfere() {
        assert_eq!(order(Case { project_view: true, collapsed: col(&["running", "attention", "pinned"]), ..Case::default() }), ["o1", "o2", "x1", "x2"]);
        let all_collapsed = vec![proj("x", true, &["x1", "x2"]), proj("y", true, &["y1"])];
        assert_eq!(order(Case { project_view: true, projects: all_collapsed, ..Case::default() }), ["o1", "o2"]);
        let all_open = vec![proj("x", false, &["x1", "x2"]), proj("y", false, &["y1"])];
        assert_eq!(order(Case { project_view: true, projects: all_open, ..Case::default() }), ["o1", "o2", "x1", "x2", "y1"]);
    }

    #[test]
    fn empty_groups_leave_no_trace() {
        assert_eq!(order(Case { history: vec![], ..Case::default() }), ["o1", "o2", "a1", "r1", "r2", "p1"]);
        assert_eq!(order(Case { labeled: vec![g("attention", &[]), g("running", &["r1"])], ..Case::default() }), ["o1", "o2", "r1", "h1", "h2"]);
        assert!(order(Case { top: vec![g("opened", &[])], labeled: vec![], history: vec![], ..Case::default() }).is_empty());
    }

    #[test]
    fn pinned_on_top_and_day_segments() {
        let top = || vec![g("pinned", &["p0"]), g("opened", &["o1", "o2"])];
        assert_eq!(order(Case { top: top(), ..Case::default() })[..3], ["p0", "o1", "o2"]);
        assert_eq!(order(Case { project_view: true, top: top(), ..Case::default() }), ["p0", "o1", "o2", "x1", "x2"]);
        assert_eq!(order(Case { top: top(), collapsed: col(&["pinned"]), ..Case::default() })[..2], ["o1", "o2"]);
        let by_day = || vec![g("day-today", &["t1"]), g("day-yesterday", &["z1"]), g("day-older", &["e1", "e2"])];
        let o = order(Case { history: by_day(), ..Case::default() });
        assert_eq!(o[o.len() - 4..], ["t1", "z1", "e1", "e2"]);
        let o = order(Case { history: by_day(), collapsed: col(&["day-yesterday"]), ..Case::default() });
        assert_eq!(o[o.len() - 3..], ["t1", "e1", "e2"], "折叠「昨天」只摘掉那一段");
        let all = build(Case::default()).order;
        let set: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(set.len(), all.len(), "顺序里无重复");
    }

    // ---- 拍平的结构：组头、间距、状态列、组归属 ----

    #[test]
    fn items_structure_matches_the_dom() {
        let t = build(Case { labeled: vec![g("attention", &["a1"])], collapsed: col(&["attention"]), ..Case::default() });
        let kinds: Vec<String> = t
            .items
            .iter()
            .map(|i| match i {
                Item::Space(h) => format!("_{h}"),
                Item::GroupHeader { id, collapsed, count, .. } => format!("H:{id}{}{count}", if *collapsed { "-" } else { "+" }),
                Item::Session { row, .. } => NAMES[*row].to_string(),
                Item::ProjectHeader { key, .. } => format!("P:{key}"),
                Item::Empty(s) => format!("E:{s}"),
            })
            .collect();
        assert_eq!(kinds, ["_4", "H:opened+2", "o1", "o2", "_10", "H:attention-1", "_10", "h1", "h2", "_4"], "折叠后组头和计数还在；不分段的历史没有组头");
        assert_eq!(t.membership.get(&r("a1")), Some(&GroupRef::Status("attention".into())), "折叠组里没渲染的行也知道自己的组");
        assert_eq!(t.membership.get(&r("h1")), Some(&GroupRef::None));
        assert_eq!(t.item_of_row(r("o2")), Some(3));
        assert_eq!(t.item_of_row(r("a1")), None);
    }

    #[test]
    fn project_view_gaps_and_headers() {
        let t = build(Case { project_view: true, ..Case::default() });
        let hs: Vec<f32> = t.items.iter().map(|i| i.height()).collect();
        // pad, 已打开组头, o1, o2, 组→项目间距 10, x 头, x1, x2, 2, y 头(折叠), 2, pad
        assert_eq!(hs, [4.0, 19.0, 43.0, 43.0, 10.0, 26.0, 43.0, 43.0, 2.0, 26.0, 2.0, 4.0]);
        assert!(matches!(&t.items[9], Item::ProjectHeader { key, collapsed: true, count: 1, .. } if key == "y"));
        assert_eq!(t.membership.get(&r("y1")), Some(&GroupRef::Project { key: "y".into(), has_active: false }));
    }

    #[test]
    fn status_column_is_per_group() {
        let alive = |i: usize| i == r("r2");
        let t = flatten(&TreeInput {
            top: vec![],
            project_view: false,
            labeled: vec![g("running", &["r1", "r2"]), g("pinned", &["p1"])],
            history: vec![],
            projects: vec![],
            collapsed_groups: &[],
            filtered_count: 3,
            empty_text: String::new(),
            alive: &alive,
        });
        let st: Vec<(usize, bool)> = t.items.iter().filter_map(|i| if let Item::Session { row, show_status } = i { Some((*row, *show_status)) } else { None }).collect();
        assert_eq!(st, [(r("r1"), true), (r("r2"), true), (r("p1"), false)], "组里有一条活的就整组画；全死的组不画");
    }

    #[test]
    fn empty_states() {
        let alive = |_: usize| false;
        let mk = |project_view: bool, top: Vec<TreeGroup>, filtered: usize| {
            flatten(&TreeInput {
                top,
                project_view,
                labeled: vec![],
                history: vec![],
                projects: vec![],
                collapsed_groups: &[],
                filtered_count: filtered,
                empty_text: "无匹配 session".into(),
                alive: &alive,
            })
            .items
        };
        assert_eq!(mk(false, vec![], 0), [Item::Space(4.0), Item::Empty("无匹配 session".into()), Item::Space(4.0)]);
        assert!(mk(true, vec![], 0).iter().any(|i| matches!(i, Item::Empty(_))), "项目视图：项目组、置顶、已打开全空才显示");
        assert!(!mk(true, vec![g("opened", &["o1"])], 1).iter().any(|i| matches!(i, Item::Empty(_))), "全部匹配项都开着时不是「无 session」");
    }

    // ---- scripts/test-tree-nav.ts（moveSelection）----

    #[test]
    fn move_selection_edges() {
        let l = [10, 11, 12];
        assert_eq!(move_selection(&[], None, true), None, "空列表");
        assert_eq!(move_selection(&[], Some(10), true), None);
        assert_eq!(move_selection(&l, None, true), Some(10), "没有选中时按 ↓：第一条");
        assert_eq!(move_selection(&l, None, false), Some(10), "没有选中时按 ↑：也是第一条");
        assert_eq!(move_selection(&l, Some(99), true), Some(10), "选中项已被筛掉：回到第一条");
        assert_eq!(move_selection(&l, Some(99), false), Some(10));
        assert_eq!(move_selection(&l, Some(10), true), Some(11));
        assert_eq!(move_selection(&l, Some(11), false), Some(10));
        assert_eq!(move_selection(&l, Some(11), true), Some(12));
        assert_eq!(move_selection(&l, Some(12), true), Some(12), "到底停住");
        assert_eq!(move_selection(&l, Some(10), false), Some(10), "到顶停住");
        assert_eq!(move_selection(&[7], Some(7), true), Some(7));
        assert_eq!(move_selection(&[7], Some(7), false), Some(7));
    }

    // ---- 几何 ----

    fn items_h(hs: &[f32]) -> Vec<Item> {
        hs.iter().map(|h| Item::Space(*h)).collect()
    }

    #[test]
    fn tops_and_offsets() {
        let t = tops(&items_h(&[4.0, 19.0, 43.0, 43.0]));
        assert_eq!(t, [0.0, 4.0, 23.0, 66.0, 109.0]);
        assert_eq!(offset_to_item(&t, 0.0), (0, 0.0));
        assert_eq!(offset_to_item(&t, 30.0), (2, 7.0));
        assert_eq!(offset_to_item(&t, 66.0), (3, 0.0), "正好落在边界上算下一项");
        assert_eq!(offset_to_item(&t, 500.0), (3, 434.0), "超出总高落在最后一项");
        assert_eq!(offset_to_item(&[0.0], 10.0), (0, 0.0), "空列表");
    }

    #[test]
    fn center_and_nearest() {
        let t = tops(&items_h(&[100.0; 10])); // 总高 1000
        assert_eq!(center_scroll_top(&t, 5, 300.0), 400.0, "550 居中 → 400");
        assert_eq!(center_scroll_top(&t, 0, 300.0), 0.0, "夹住顶");
        assert_eq!(center_scroll_top(&t, 9, 300.0), 700.0, "夹住底");
        assert_eq!(center_scroll_top(&t, 3, 2000.0), 0.0, "内容比视口短");
        assert_eq!(reveal_nearest(&t, 2, 250.0, 300.0), Some(200.0), "在上方 → 顶对齐");
        assert_eq!(reveal_nearest(&t, 6, 250.0, 300.0), Some(400.0), "在下方 → 底贴视口底");
        assert_eq!(reveal_nearest(&t, 3, 250.0, 300.0), None, "完整可见 → 不动");
    }

    #[test]
    fn splice_only_the_changed_middle() {
        assert_eq!(splice_plan(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]), None);
        assert_eq!(splice_plan(&[1.0, 2.0, 3.0], &[1.0, 9.0, 9.0, 3.0]), Some((1..2, 2)));
        assert_eq!(splice_plan(&[1.0, 2.0, 3.0], &[1.0, 3.0]), Some((1..2, 0)), "删掉中间一项");
        assert_eq!(splice_plan(&[], &[1.0, 2.0]), Some((0..0, 2)));
        assert_eq!(splice_plan(&[1.0, 1.0], &[1.0, 1.0, 1.0]), Some((2..2, 1)), "前缀后缀重叠时不越界");
        assert_eq!(splice_plan(&[5.0], &[]), Some((0..1, 0)));
    }

    #[test]
    fn width_range() {
        assert_eq!(clamp_width(100.0), 180.0);
        assert_eq!(clamp_width(300.0), 300.0);
        assert_eq!(clamp_width(999.0), 480.0);
    }
}
