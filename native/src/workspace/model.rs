//! 工作区模型：递归分割树（照搬 `src/workspace-types.ts` + `src/useWorkspace.ts`）。
//!
//! - 数据形状和 TS 版一致，serde 出来的 JSON 就是 localStorage `makit-workspace` 的格式，
//!   所以 Tauri 版存的布局可以原样导入。
//! - 树操作是纯函数（`find_container` / `close_tab` / `layout_tree` / `find_nearest_container` …）；
//!   `Workspace` 把 useWorkspace 里的各个 handler 收成方法，每次改动之后统一过 `normalize` 并维护激活栈 ——
//!   和 TS 版在 setWorkspace 出口收口是同一个理由（见 `normalize` 的注释）。
//! - 不碰终端：关 tab 返回被关掉的 tab id，调用方（工作区视图）负责杀对应的 PTY。
//!
//! 为什么单独测：放大态、激活栈、tabHistory 这些不变量以前在 TS 版里被 4 条路径各自破坏过，
//! 错了在 UI 上是「键盘输入进了一个看不见的 pane」「关掉 tab 后焦点跳到莫名其妙的地方」。
//! `scripts/test-workspace-invariant.ts` / `test-resume-tab.ts` / `test-context-menu.ts`（tabsToClose）/
//! `test-tree-nav.ts`（openedOrder）里的用例逐个移植在下面的测试里。

use crate::ts;
use serde::{Deserialize, Serialize};

// ---------- 数据形状 ----------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TabKind {
    Resume,
    New,
    Shell,
}

/// 一个标签页。`id` 就是 PTY id（环境变量 `MAKIT_PTY_ID`），持久化后不变。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaneTab {
    pub id: String,
    pub kind: TabKind,
    pub cwd: String,
    /// 启动后像打字一样写进 shell 的命令。生成 PTY 时**只看它**，不看 kind
    pub init_command: Option<String>,
    pub session_id: Option<String>,
    pub session_short_id: Option<String>,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerNode {
    pub id: String,
    pub tabs: Vec<PaneTab>,
    pub active_tab_id: String,
    #[serde(default)]
    pub tab_history: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// 上下分（a 在上）
    #[serde(rename = "h")]
    H,
    /// 左右分（a 在左）
    #[serde(rename = "v")]
    V,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SplitNode {
    pub dir: Dir,
    pub ratio: f64,
    pub a: Box<LayoutNode>,
    pub b: Box<LayoutNode>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LayoutNode {
    Container(ContainerNode),
    Split(SplitNode),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub root: LayoutNode,
    pub active_container_id: String,
    /// 非空时该 container 占满工作区，其它隐藏；container 消失会被自动清空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximized_container_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseScope {
    Others,
    Right,
}

/// 从侧栏 / ⌘K 打开会话时的 tab 规格（`handleSplitWithSession` / `handleDropSession` 的 spec）
#[derive(Clone, Debug, PartialEq)]
pub struct TabSpec {
    pub kind: TabKind,
    pub cwd: String,
    pub init_command: Option<String>,
    pub session_id: Option<String>,
    pub session_short_id: Option<String>,
}

// ---------- ID 生成：`c_` / `t_` + base36 毫秒时间戳 + 4 位随机 ----------

fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn make_id(prefix: &str) -> String {
    use std::sync::Mutex;
    // (上次的毫秒, 这一毫秒的起点, 这一毫秒里已经造了几个)
    static LAST: Mutex<(u64, u64, u64)> = Mutex::new((u64::MAX, 0, 0));
    const SPACE: u64 = 36 * 36 * 36 * 36;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // 4 位「随机」= 每毫秒一个随机起点 + 这一毫秒里的序号（mod 36⁴）。
    // 以前是 (时间 ^ 序号) 过一遍 xorshift 再逐字节 mod 36：那不是单射，36⁴ 的空间里同一毫秒造 500 个
    // 有约 7% 的生日碰撞（并行跑测试时同一毫秒里挤进的 id 更多，偶发失败就是这么来的）。
    // 起点 + 序号在同一毫秒内严格不重（除非一毫秒造满 36⁴ 个），跨进程靠随机起点和 pid 错开。
    // 时间戳必须在锁里取、且不许倒退：锁外取的话，线程 A 取到第 100ms、线程 B 取到 101ms 并先拿到锁，
    // A 再进来会把第 100ms 的序号从 0 重来 —— 和 A 之前在 100ms 造过的撞上（这就是并行跑偶发失败的第二个原因）
    let (ms, n) = {
        let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let ms = if g.0 != u64::MAX && now < g.0 { g.0 } else { now };
        if g.0 != ms {
            let mut x = ms ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (&now as *const _ as u64);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *g = (ms, x % SPACE, 0);
        }
        let n = (g.1 + g.2) % SPACE;
        g.2 += 1;
        (ms, n)
    };
    let rand4: String = (0..4).rev().map(|i| base36((n / 36u64.pow(i)) % 36)).collect();
    format!("{prefix}{}{rand4}", base36(ms))
}

pub fn make_container_id() -> String {
    make_id("c_")
}

pub fn make_tab_id() -> String {
    make_id("t_")
}

// ---------- 树操作 ----------

pub fn empty_container(id: String) -> ContainerNode {
    ContainerNode { id, tabs: Vec::new(), active_tab_id: String::new(), tab_history: Vec::new() }
}

pub fn default_workspace() -> WorkspaceState {
    let id = make_container_id();
    WorkspaceState {
        root: LayoutNode::Container(empty_container(id.clone())),
        active_container_id: id,
        maximized_container_id: None,
    }
}

pub fn find_container<'a>(node: &'a LayoutNode, id: &str) -> Option<&'a ContainerNode> {
    match node {
        LayoutNode::Container(c) => (c.id == id).then_some(c),
        LayoutNode::Split(s) => find_container(&s.a, id).or_else(|| find_container(&s.b, id)),
    }
}

pub fn find_container_mut<'a>(node: &'a mut LayoutNode, id: &str) -> Option<&'a mut ContainerNode> {
    match node {
        LayoutNode::Container(c) => (c.id == id).then_some(c),
        LayoutNode::Split(s) => {
            if find_container(&s.a, id).is_some() {
                find_container_mut(&mut s.a, id)
            } else {
                find_container_mut(&mut s.b, id)
            }
        }
    }
}

/// 所有 container，先 a 后 b（= 屏幕上左/上先于右/下）
pub fn collect_containers(node: &LayoutNode) -> Vec<&ContainerNode> {
    fn walk<'a>(n: &'a LayoutNode, out: &mut Vec<&'a ContainerNode>) {
        match n {
            LayoutNode::Container(c) => out.push(c),
            LayoutNode::Split(s) => {
                walk(&s.a, out);
                walk(&s.b, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

pub fn collect_all_tab_ids(node: &LayoutNode) -> Vec<String> {
    collect_containers(node).iter().flat_map(|c| c.tabs.iter().map(|t| t.id.clone())).collect()
}

/// 放大态的唯一不变量：**「我在看哪个」必须等于「键盘输入去哪」**。
///
/// maximized 决定谁铺满、其余隐藏，active 决定键盘输入进谁 —— 两者不等时就出现「操作一个
/// 看不见的 pane」。TS 版以前有 4 条路径各自破坏它，所以在出口统一收口（`Workspace` 的每个方法最后都过这里）。
///
/// 两步的顺序不能反：关掉「被放大 container 的最后一个 tab」会同时让 container 消失、
/// active 跳到别的 container，此时正确行为是退出放大（①），而不是把新落脚的那个放大（②）。
pub fn normalize(ws: &mut WorkspaceState) {
    let Some(max_id) = ws.maximized_container_id.clone() else { return };
    // ① 被放大的 container 已不存在 → 退出放大
    if find_container(&ws.root, &max_id).is_none() {
        ws.maximized_container_id = None;
        return;
    }
    // ② 焦点移到了别的 container → 放大跟着焦点走；焦点落在不存在的 id 上时退出放大
    if max_id != ws.active_container_id {
        ws.maximized_container_id =
            find_container(&ws.root, &ws.active_container_id).map(|_| ws.active_container_id.clone());
    }
}

/// 恢复一个已有会话要敲的命令。**只在这里拼**（测试会扫源码，别处手拼会红）
pub fn resume_cmd(session_id: &str, tool: Option<&str>) -> String {
    if tool == Some("codex") {
        format!("codex resume {session_id}")
    } else {
        format!("claude -r {session_id}")
    }
}

/// resume tab 的 initCommand。前面的 clear 是为了不让 shell 自己的 banner 留在会话上方
pub fn resume_init_command(session_id: &str, tool: Option<&str>) -> String {
    format!("clear && {}", resume_cmd(session_id, tool))
}

/// 新建 AI 会话的 initCommand（`openNewSession`）
pub fn new_session_command(tool: Option<&str>) -> String {
    if tool == Some("codex") { "codex".into() } else { "claude".into() }
}

/// 把 shell / new tab 升级成 resume tab（用户在纯 shell 里手打了 claude，后端从 pid 文件认出了会话）。
/// **必须连 initCommand 一起写**（见 workspace-types.ts 同名函数的注释）；已知会话起始目录时 cwd 改用它（#190）。
pub fn bind_session_to_pane_tab(t: &PaneTab, session_id: &str, short_id: &str, label: Option<&str>, session_cwd: Option<&str>) -> PaneTab {
    PaneTab {
        kind: TabKind::Resume,
        session_id: Some(session_id.into()),
        session_short_id: Some(short_id.into()),
        cwd: session_cwd.filter(|c| !c.is_empty()).map(String::from).unwrap_or_else(|| t.cwd.clone()),
        init_command: Some(resume_init_command(session_id, None)),
        label: match label {
            Some(l) if !l.trim().is_empty() => l.to_string(),
            _ => format!("[{short_id}]"),
        },
        ..t.clone()
    }
}

/// 修掉存档里的坏 tab：kind=resume、有 sessionId、却没有 initCommand。只在读持久化状态时跑一次。
/// 返回是否改了东西。
pub fn repair_resume_tabs(ws: &mut WorkspaceState) -> bool {
    fn fix(n: &mut LayoutNode, changed: &mut bool) {
        match n {
            LayoutNode::Container(c) => {
                for t in &mut c.tabs {
                    if t.kind == TabKind::Resume && t.init_command.is_none() {
                        if let Some(sid) = t.session_id.clone().filter(|s| !s.is_empty()) {
                            t.init_command = Some(resume_init_command(&sid, None));
                            *changed = true;
                        }
                    }
                }
            }
            LayoutNode::Split(s) => {
                fix(&mut s.a, changed);
                fix(&mut s.b, changed);
            }
        }
    }
    let mut changed = false;
    fix(&mut ws.root, &mut changed);
    changed
}

/// 「关闭其他」/「关闭右侧」要关哪些 tab（显示顺序）。锚点是**右键的那个 tab**；找不到锚点返回空。
pub fn tabs_to_close(tabs: &[PaneTab], anchor_id: &str, scope: CloseScope) -> Vec<String> {
    let Some(idx) = tabs.iter().position(|t| t.id == anchor_id) else { return Vec::new() };
    match scope {
        CloseScope::Others => tabs.iter().filter(|t| t.id != anchor_id).map(|t| t.id.clone()).collect(),
        CloseScope::Right => tabs[idx + 1..].iter().map(|t| t.id.clone()).collect(),
    }
}

/// 「打开中」段的顺序：所有在 tab 里开着的会话 id，按屏幕上的空间顺序（先 a 后 b），去重
pub fn opened_order(ws: &WorkspaceState) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in collect_containers(&ws.root) {
        for t in &c.tabs {
            if t.kind != TabKind::Resume {
                continue;
            }
            let Some(sid) = &t.session_id else { continue };
            if !out.contains(sid) {
                out.push(sid.clone());
            }
        }
    }
    out
}

/// 删掉一个 container；split 只剩一边时用那一边顶上。整棵树都删空了返回 None
pub fn remove_container(node: LayoutNode, id: &str) -> Option<LayoutNode> {
    match node {
        LayoutNode::Container(c) => (c.id != id).then_some(LayoutNode::Container(c)),
        LayoutNode::Split(s) => {
            let SplitNode { dir, ratio, a, b } = s;
            match (remove_container(*a, id), remove_container(*b, id)) {
                (Some(a), Some(b)) => Some(LayoutNode::Split(SplitNode { dir, ratio, a: Box::new(a), b: Box::new(b) })),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            }
        }
    }
}

/// 把 id 这个 container 换成 `f(它)` 的结果（用来把它包进一个新 split）
pub fn replace_container(node: LayoutNode, id: &str, f: &mut dyn FnMut(ContainerNode) -> LayoutNode) -> LayoutNode {
    match node {
        LayoutNode::Container(c) if c.id == id => f(c),
        LayoutNode::Container(c) => LayoutNode::Container(c),
        LayoutNode::Split(s) => LayoutNode::Split(SplitNode {
            dir: s.dir,
            ratio: s.ratio,
            a: Box::new(replace_container(*s.a, id, f)),
            b: Box::new(replace_container(*s.b, id, f)),
        }),
    }
}

fn first_container_id(node: &LayoutNode) -> &str {
    match node {
        LayoutNode::Container(c) => &c.id,
        LayoutNode::Split(s) => first_container_id(&s.a),
    }
}

/// split 的 id：`split-<a 子树第一个 container>-<b 子树第一个 container>`（和 layout_tree 一致）
pub fn split_id(s: &SplitNode) -> String {
    format!("split-{}-{}", first_container_id(&s.a), first_container_id(&s.b))
}

pub fn set_split_ratio(node: &mut LayoutNode, split: &str, ratio: f64) -> bool {
    match node {
        LayoutNode::Container(_) => false,
        LayoutNode::Split(s) => {
            if split_id(s) == split {
                s.ratio = ratio;
                return true;
            }
            set_split_ratio(&mut s.a, split, ratio) || set_split_ratio(&mut s.b, split, ratio)
        }
    }
}

// ---------- 布局矩形 ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitInfo {
    pub id: String,
    pub dir: Dir,
    /// 分割线本身（RESIZER_PX = 0，所以宽或高为 0）
    pub rect: Rect,
    /// 这个 split 占的整个区域（拖动时按它换算 ratio）
    pub outer_rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutResult {
    /// container id → 矩形，按屏幕空间顺序（先 a 后 b）。顺序有语义：几何导航平手时取先出现的
    pub containers: Vec<(String, Rect)>,
    pub splits: Vec<SplitInfo>,
}

const RESIZER_PX: f64 = 0.0;

pub fn layout_tree(node: &LayoutNode, rect: Rect) -> LayoutResult {
    match node {
        LayoutNode::Container(c) => {
            let mut r = LayoutResult::default();
            r.containers.push((c.id.clone(), rect));
            r
        }
        LayoutNode::Split(s) => {
            let (a_rect, b_rect, split_rect) = if s.dir == Dir::V {
                let usable = (rect.width - RESIZER_PX).max(0.0);
                let aw = usable * s.ratio;
                (
                    Rect { width: aw, ..rect },
                    Rect { x: rect.x + aw + RESIZER_PX, width: usable - aw, ..rect },
                    Rect { x: rect.x + aw, width: RESIZER_PX, ..rect },
                )
            } else {
                let usable = (rect.height - RESIZER_PX).max(0.0);
                let ah = usable * s.ratio;
                (
                    Rect { height: ah, ..rect },
                    Rect { y: rect.y + ah + RESIZER_PX, height: usable - ah, ..rect },
                    Rect { y: rect.y + ah, height: RESIZER_PX, ..rect },
                )
            };
            let mut a = layout_tree(&s.a, a_rect);
            let b = layout_tree(&s.b, b_rect);
            a.containers.extend(b.containers);
            a.splits.extend(b.splits);
            a.splits.push(SplitInfo { id: split_id(s), dir: s.dir, rect: split_rect, outer_rect: rect });
            a
        }
    }
}

/// 几何方向导航（⌥⌘方向键）：按中心点判方向；垂直轴上有重叠的优先，其次看边界距离。
/// 算法参考 bonsplit 的 findBestNeighbor（同 TS 版）。
impl LayoutResult {
    pub fn rect(&self, id: &str) -> Option<Rect> {
        self.containers.iter().find(|(c, _)| c == id).map(|(_, r)| *r)
    }
}

pub fn find_nearest_container(layout: &LayoutResult, active_id: &str, dir: Direction) -> Option<String> {
    let cur = &layout.rect(active_id)?;
    let (cx, cy) = (cur.x + cur.width / 2.0, cur.y + cur.height / 2.0);
    let mut best: Option<(bool, f64, &String)> = None;
    for (id, r) in layout.containers.iter() {
        if id == active_id {
            continue;
        }
        let (ox, oy) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        let in_dir = match dir {
            Direction::Left => ox < cx,
            Direction::Right => ox > cx,
            Direction::Up => oy < cy,
            Direction::Down => oy > cy,
        };
        if !in_dir {
            continue;
        }
        let (overlap, distance) = match dir {
            Direction::Left | Direction::Right => (
                ((cur.y + cur.height).min(r.y + r.height) - cur.y.max(r.y)).max(0.0),
                if dir == Direction::Left { cur.x - (r.x + r.width) } else { r.x - (cur.x + cur.width) },
            ),
            Direction::Up | Direction::Down => (
                ((cur.x + cur.width).min(r.x + r.width) - cur.x.max(r.x)).max(0.0),
                if dir == Direction::Up { cur.y - (r.y + r.height) } else { r.y - (cur.y + cur.height) },
            ),
        };
        let cand = (overlap > 0.0, distance, id);
        best = match best {
            None => Some(cand),
            Some(b) => {
                // 有重叠的优先；同等时距离小的优先；再相同时保留先遇到的（TS 的 sort 是稳定的）
                let better = (cand.0 && !b.0) || (cand.0 == b.0 && cand.1 < b.1);
                Some(if better { cand } else { b })
            }
        };
    }
    best.map(|(_, _, id)| id.clone())
}

// ---------- 旧格式迁移（localStorage `makit-open-tabs`）----------

fn flatten_old_pane_tree(node: &serde_json::Value, panes: &[serde_json::Value]) -> LayoutNode {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    match node.get("kind").and_then(|k| k.as_str()) {
        Some("leaf") => {
            let pane_id = s(node, "paneId").unwrap_or_default();
            let tab = match panes.iter().find(|p| s(p, "paneId").as_deref() == Some(pane_id.as_str())) {
                Some(p) => {
                    let short = s(p, "sessionShortId");
                    PaneTab {
                        id: pane_id.clone(),
                        kind: serde_json::from_value(p.get("kind").cloned().unwrap_or_default()).unwrap_or(TabKind::Shell),
                        cwd: s(p, "cwd").unwrap_or_default(),
                        init_command: s(p, "initCommand"),
                        session_id: s(p, "sessionId"),
                        label: short.as_ref().map(|x| format!("[{x}]")).unwrap_or_default(),
                        session_short_id: short,
                    }
                }
                None => PaneTab {
                    id: pane_id.clone(),
                    kind: TabKind::Shell,
                    cwd: "/".into(),
                    init_command: None,
                    session_id: None,
                    session_short_id: None,
                    label: String::new(),
                },
            };
            LayoutNode::Container(ContainerNode {
                id: make_container_id(),
                active_tab_id: tab.id.clone(),
                tabs: vec![tab],
                tab_history: Vec::new(),
            })
        }
        Some("split") => LayoutNode::Split(SplitNode {
            dir: if node.get("dir").and_then(|d| d.as_str()) == Some("h") { Dir::H } else { Dir::V },
            ratio: node.get("ratio").and_then(|r| r.as_f64()).unwrap_or(0.5),
            a: Box::new(flatten_old_pane_tree(node.get("a").unwrap_or(&serde_json::Value::Null), panes)),
            b: Box::new(flatten_old_pane_tree(node.get("b").unwrap_or(&serde_json::Value::Null), panes)),
        }),
        _ => LayoutNode::Container(empty_container(make_container_id())),
    }
}

/// 旧版 `makit-open-tabs`（每个顶层 tab 一棵 pane 树）→ 新格式；多个旧 tab 横向排开
pub fn migrate_workspace(old_tabs: &[serde_json::Value]) -> WorkspaceState {
    let tree = |t: &serde_json::Value| {
        let panes: Vec<serde_json::Value> = t.get("panes").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        flatten_old_pane_tree(t.get("root").unwrap_or(&serde_json::Value::Null), &panes)
    };
    match old_tabs {
        [] => default_workspace(),
        [t] => {
            let root = tree(t);
            let active_pane = t.get("activePaneId").and_then(|x| x.as_str()).unwrap_or("");
            let cs = collect_containers(&root);
            let active = cs
                .iter()
                .find(|c| c.tabs.iter().any(|tab| tab.id == active_pane))
                .or(cs.first())
                .map(|c| c.id.clone())
                .unwrap_or_default();
            WorkspaceState { root, active_container_id: active, maximized_container_id: None }
        }
        [first, rest @ ..] => {
            let mut combined = tree(first);
            for (i, t) in rest.iter().enumerate() {
                let i = (i + 1) as f64;
                combined = LayoutNode::Split(SplitNode { dir: Dir::V, ratio: i / (i + 1.0), a: Box::new(combined), b: Box::new(tree(t)) });
            }
            let active = collect_containers(&combined).first().map(|c| c.id.clone()).unwrap_or_default();
            WorkspaceState { root: combined, active_container_id: active, maximized_container_id: None }
        }
    }
}

/// 读持久化的布局：新格式 JSON 优先，其次旧格式 `makit-open-tabs`，都没有 / 坏了就是默认布局。
/// 读出来之后先 repair 再 normalize（和 useWorkspace 的初始值一致）。
pub fn load_workspace(workspace_json: Option<&str>, old_open_tabs_json: Option<&str>) -> WorkspaceState {
    let mut ws = workspace_json
        .and_then(|s| serde_json::from_str::<WorkspaceState>(s).ok())
        .or_else(|| {
            let old: Vec<serde_json::Value> = serde_json::from_str(old_open_tabs_json?).ok()?;
            (!old.is_empty()).then(|| migrate_workspace(&old))
        })
        .unwrap_or_else(default_workspace);
    repair_resume_tabs(&mut ws);
    normalize(&mut ws);
    ws
}

// ---------- Workspace：useWorkspace 的各个 handler ----------

/// 打开会话的结果：已经开着就切过去（不重复开），否则新开了一个 tab
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    Existing { container_id: String, tab_id: String },
    New { tab_id: String },
}

/// 带激活栈的工作区。所有改动走这里的方法，方法末尾统一 `settle()`（normalize + 激活栈）。
#[derive(Clone, Debug)]
pub struct Workspace {
    pub state: WorkspaceState,
    /// container 激活栈：关掉当前 container 后回到最近一次激活的那个
    activation: Vec<String>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new(default_workspace())
    }
}

impl Workspace {
    pub fn new(mut state: WorkspaceState) -> Self {
        normalize(&mut state);
        let mut ws = Self { state, activation: Vec::new() };
        ws.push_activation();
        ws
    }

    fn push_activation(&mut self) {
        let id = self.state.active_container_id.clone();
        if id.is_empty() {
            return;
        }
        if self.activation.last() != Some(&id) {
            self.activation.retain(|x| *x != id);
            self.activation.push(id);
        }
    }

    fn settle(&mut self) {
        normalize(&mut self.state);
        self.push_activation();
    }

    fn pick_next_container(&self, closed_id: &str) -> String {
        let cs = collect_containers(&self.state.root);
        for id in self.activation.iter().rev() {
            if id != closed_id && cs.iter().any(|c| c.id == *id) {
                return id.clone();
            }
        }
        cs.first().map(|c| c.id.clone()).unwrap_or_default()
    }

    pub fn active_container(&self) -> Option<&ContainerNode> {
        find_container(&self.state.root, &self.state.active_container_id)
    }

    pub fn active_tab(&self) -> Option<&PaneTab> {
        let c = self.active_container()?;
        c.tabs.iter().find(|t| t.id == c.active_tab_id)
    }

    /// 找 tab 在哪个 container
    pub fn locate_tab(&self, tab_id: &str) -> Option<(&ContainerNode, &PaneTab)> {
        collect_containers(&self.state.root)
            .into_iter()
            .find_map(|c| c.tabs.iter().find(|t| t.id == tab_id).map(|t| (c, t)))
    }

    /// 新 tab 的 cwd：container 当前 tab 的，没有就 "~"
    fn inherit_cwd(&self, container_id: &str) -> String {
        find_container(&self.state.root, container_id)
            .and_then(|c| c.tabs.iter().find(|t| t.id == c.active_tab_id))
            .map(|t| t.cwd.clone())
            .unwrap_or_else(|| "~".into())
    }

    pub fn set_active(&mut self, container_id: &str) {
        self.state.active_container_id = container_id.into();
        self.settle();
    }

    pub fn toggle_maximize(&mut self, container_id: &str) {
        if find_container(&self.state.root, container_id).is_none() {
            return;
        }
        let next = if self.state.maximized_container_id.as_deref() == Some(container_id) { None } else { Some(container_id.to_string()) };
        self.state.maximized_container_id = next;
        self.state.active_container_id = container_id.into();
        self.settle();
    }

    /// 点 tab：切过去并记进 tabHistory
    pub fn tab_click(&mut self, container_id: &str, tab_id: &str) {
        self.state.active_container_id = container_id.into();
        if let Some(c) = find_container_mut(&mut self.state.root, container_id) {
            c.tab_history.retain(|x| x != tab_id);
            let prev = c.active_tab_id.clone();
            c.tab_history.push(prev);
            c.active_tab_id = tab_id.into();
        }
        self.settle();
    }

    /// 关 tab。返回被关掉的 tab id（调用方负责杀 PTY）。container 空了就删掉；整棵树空了重建一个空 container。
    /// 关掉的是 active tab 时回到 tabHistory 里上一个还在的，没有就第一个。
    pub fn close_tab(&mut self, container_id: &str, tab_id: &str) -> Vec<String> {
        let Some(c) = find_container(&self.state.root, container_id) else { return Vec::new() };
        if !c.tabs.iter().any(|t| t.id == tab_id) {
            // TS 版这里照样返回 [tabId]（destroy 不存在的终端是空操作）；Rust 版不存在就不报
            return Vec::new();
        }
        let remaining: Vec<PaneTab> = c.tabs.iter().filter(|t| t.id != tab_id).cloned().collect();
        if remaining.is_empty() {
            let root = std::mem::replace(&mut self.state.root, LayoutNode::Container(empty_container(String::new())));
            match remove_container(root, container_id) {
                None => {
                    // 最后一个 tab 关掉：重建空工作区（和 makeDefaultWorkspace 一样，放大态也清掉）
                    self.state = default_workspace();
                }
                Some(root) => {
                    self.state.root = root;
                    if find_container(&self.state.root, &self.state.active_container_id).is_none() {
                        self.state.active_container_id = self.pick_next_container(container_id);
                    }
                }
            }
        } else if let Some(c) = find_container_mut(&mut self.state.root, container_id) {
            if c.active_tab_id == tab_id {
                let hist: Vec<&String> =
                    c.tab_history.iter().filter(|id| *id != tab_id && remaining.iter().any(|t| &t.id == *id)).collect();
                c.active_tab_id = hist.last().map(|s| s.to_string()).unwrap_or_else(|| remaining[0].id.clone());
            }
            c.tab_history.retain(|id| id != tab_id);
            c.tabs = remaining;
        }
        self.settle();
        vec![tab_id.to_string()]
    }

    /// 分屏（⌘D 左右 = Dir::V，⌘⇧D 上下 = Dir::H）：新 container 放一个 shell，cwd 继承当前 tab；退出放大。
    /// 返回新 tab id
    pub fn split(&mut self, container_id: &str, dir: Dir) -> String {
        let new_id = make_container_id();
        let tab = PaneTab {
            id: make_tab_id(),
            kind: TabKind::Shell,
            cwd: self.inherit_cwd(container_id),
            init_command: None,
            session_id: None,
            session_short_id: None,
            label: String::new(),
        };
        let tab_id = tab.id.clone();
        let new_c = ContainerNode { id: new_id.clone(), active_tab_id: tab_id.clone(), tabs: vec![tab], tab_history: Vec::new() };
        let root = std::mem::replace(&mut self.state.root, LayoutNode::Container(empty_container(String::new())));
        let mut new_c = Some(new_c);
        self.state.root = replace_container(root, container_id, &mut |c| {
            LayoutNode::Split(SplitNode {
                dir,
                ratio: 0.5,
                a: Box::new(LayoutNode::Container(c)),
                b: Box::new(LayoutNode::Container(new_c.take().expect("replaced only once"))),
            })
        });
        // container 不存在时树不变，焦点也不能指到一个不存在的 id 上
        if find_container(&self.state.root, &new_id).is_some() {
            self.state.active_container_id = new_id;
        }
        self.state.maximized_container_id = None;
        self.settle();
        tab_id
    }

    fn push_tab(&mut self, container_id: &str, tab: PaneTab) {
        if let Some(c) = find_container_mut(&mut self.state.root, container_id) {
            c.active_tab_id = tab.id.clone();
            c.tabs.push(tab);
        }
    }

    /// ⌘T：在 container 里新建 shell tab，cwd 继承当前 tab。返回 tab id
    pub fn new_shell_in(&mut self, container_id: &str) -> String {
        let tab = PaneTab {
            id: make_tab_id(),
            kind: TabKind::Shell,
            cwd: self.inherit_cwd(container_id),
            init_command: None,
            session_id: None,
            session_short_id: None,
            label: String::new(),
        };
        let id = tab.id.clone();
        self.state.active_container_id = container_id.into();
        self.push_tab(container_id, tab);
        self.settle();
        id
    }

    /// 摘出一个 tab（不关终端）：源 container 空了就删掉
    fn detach_tab(&mut self, src: &str, tab_id: &str) -> Option<PaneTab> {
        let c = find_container(&self.state.root, src)?;
        let tab = c.tabs.iter().find(|t| t.id == tab_id)?.clone();
        let remaining: Vec<PaneTab> = c.tabs.iter().filter(|t| t.id != tab_id).cloned().collect();
        if remaining.is_empty() {
            let root = std::mem::replace(&mut self.state.root, LayoutNode::Container(empty_container(String::new())));
            // 整棵树只剩这一个 container 时保持原样（TS：`removeContainer(...) ?? root`）
            let backup = root.clone();
            self.state.root = remove_container(root, src).unwrap_or(backup);
        } else if let Some(c) = find_container_mut(&mut self.state.root, src) {
            if c.active_tab_id == tab_id {
                c.active_tab_id = remaining[0].id.clone();
            }
            c.tabs = remaining;
        }
        Some(tab)
    }

    /// 拖 tab 到另一个 container 的指定位置（None = 末尾）
    pub fn move_tab(&mut self, src: &str, tab_id: &str, dest: &str, target_idx: Option<usize>) {
        if find_container(&self.state.root, dest).is_none() {
            return;
        }
        let Some(tab) = self.detach_tab(src, tab_id) else { return };
        let id = tab.id.clone();
        if let Some(d) = find_container_mut(&mut self.state.root, dest) {
            let idx = target_idx.unwrap_or(d.tabs.len()).min(d.tabs.len());
            d.tabs.insert(idx, tab);
            d.active_tab_id = id;
        }
        let cs = collect_containers(&self.state.root);
        self.state.active_container_id = if cs.iter().any(|c| c.id == dest) {
            dest.into()
        } else {
            cs.first().map(|c| c.id.clone()).unwrap_or_default()
        };
        self.settle();
    }

    /// 同 container 内重排：target_idx 是「插到原数组的哪个位置之前」
    pub fn reorder_tab(&mut self, container_id: &str, tab_id: &str, target_idx: usize) {
        if let Some(c) = find_container_mut(&mut self.state.root, container_id) {
            let Some(cur) = c.tabs.iter().position(|t| t.id == tab_id) else { return };
            let mut insert = target_idx;
            if insert > cur {
                insert -= 1;
            }
            if insert == cur {
                return;
            }
            let t = c.tabs.remove(cur);
            c.tabs.insert(insert.min(c.tabs.len()), t);
        }
        self.settle();
    }

    fn wrap_with_new_container(&mut self, target: &str, dir: Dir, side: Side, new_c: ContainerNode) {
        let root = std::mem::replace(&mut self.state.root, LayoutNode::Container(empty_container(String::new())));
        let mut new_c = Some(new_c);
        self.state.root = replace_container(root, target, &mut |t| {
            let n = LayoutNode::Container(new_c.take().expect("replaced only once"));
            let t = LayoutNode::Container(t);
            let (a, b) = if side == Side::Before { (n, t) } else { (t, n) };
            LayoutNode::Split(SplitNode { dir, ratio: 0.5, a: Box::new(a), b: Box::new(b) })
        });
    }

    /// 把一个 tab 拖到某个 pane 的边上分屏
    pub fn split_with_tab(&mut self, src: &str, tab_id: &str, target: &str, dir: Dir, side: Side) {
        let Some(tab) = self.detach_tab(src, tab_id) else { return };
        let new_id = make_container_id();
        let c = ContainerNode { id: new_id.clone(), active_tab_id: tab.id.clone(), tabs: vec![tab], tab_history: Vec::new() };
        self.wrap_with_new_container(target, dir, side, c);
        self.state.active_container_id = new_id;
        self.state.maximized_container_id = None;
        self.settle();
    }

    fn tab_from_spec(spec: &TabSpec) -> PaneTab {
        PaneTab {
            id: make_tab_id(),
            kind: spec.kind,
            cwd: spec.cwd.clone(),
            init_command: spec.init_command.clone(),
            session_id: spec.session_id.clone(),
            session_short_id: spec.session_short_id.clone(),
            label: spec.session_short_id.as_ref().map(|s| format!("[{s}]")).unwrap_or_default(),
        }
    }

    /// 把侧栏会话拖到某个 pane 的边上分屏。返回新 tab id
    pub fn split_with_session(&mut self, container_id: &str, dir: Dir, side: Side, spec: &TabSpec) -> String {
        let tab = Self::tab_from_spec(spec);
        let tab_id = tab.id.clone();
        let new_id = make_container_id();
        let c = ContainerNode { id: new_id.clone(), active_tab_id: tab_id.clone(), tabs: vec![tab], tab_history: Vec::new() };
        self.wrap_with_new_container(container_id, dir, side, c);
        self.state.active_container_id = new_id;
        self.state.maximized_container_id = None;
        self.settle();
        tab_id
    }

    /// 把会话加到某个 container 的 tab 条上。返回新 tab id
    pub fn drop_session(&mut self, container_id: &str, spec: &TabSpec) -> String {
        let tab = Self::tab_from_spec(spec);
        let id = tab.id.clone();
        self.state.active_container_id = container_id.into();
        self.push_tab(container_id, tab);
        self.settle();
        id
    }

    /// 打开（恢复）会话：已经开着就切过去，否则在当前 container 加一个 resume tab
    pub fn open_session(&mut self, session_id: &str, short_id: &str, cwd: &str, label: Option<&str>, tool: Option<&str>) -> Opened {
        let hit = collect_containers(&self.state.root).into_iter().find_map(|c| {
            c.tabs.iter().find(|t| t.session_id.as_deref() == Some(session_id)).map(|t| (c.id.clone(), t.id.clone()))
        });
        if let Some((cid, tid)) = hit {
            self.state.active_container_id = cid.clone();
            if let Some(c) = find_container_mut(&mut self.state.root, &cid) {
                c.active_tab_id = tid.clone();
            }
            self.settle();
            return Opened::Existing { container_id: cid, tab_id: tid };
        }
        let tab = PaneTab {
            id: make_tab_id(),
            kind: TabKind::Resume,
            cwd: cwd.into(),
            init_command: Some(resume_init_command(session_id, tool)),
            session_id: Some(session_id.into()),
            session_short_id: Some(short_id.into()),
            label: label.filter(|l| !l.is_empty()).map(String::from).unwrap_or_else(|| format!("[{short_id}]")),
        };
        let id = tab.id.clone();
        let active = self.state.active_container_id.clone();
        self.push_tab(&active, tab);
        self.settle();
        Opened::New { tab_id: id }
    }

    /// 新建 AI 会话（默认 claude，Codex 传 Some("codex")）。返回 tab id
    pub fn open_new_session(&mut self, cwd: &str, tool: Option<&str>) -> String {
        let tab = PaneTab {
            id: make_tab_id(),
            kind: TabKind::New,
            cwd: cwd.into(),
            init_command: Some(new_session_command(tool)),
            session_id: None,
            session_short_id: None,
            label: ts!("workspace.new_session_label").into(),
        };
        let id = tab.id.clone();
        let active = self.state.active_container_id.clone();
        self.push_tab(&active, tab);
        self.settle();
        id
    }

    /// 在当前 container 开一个 shell。返回 tab id
    pub fn open_shell(&mut self, cwd: &str) -> String {
        let tab = PaneTab {
            id: make_tab_id(),
            kind: TabKind::Shell,
            cwd: cwd.into(),
            init_command: None,
            session_id: None,
            session_short_id: None,
            label: String::new(),
        };
        let id = tab.id.clone();
        let active = self.state.active_container_id.clone();
        self.push_tab(&active, tab);
        self.settle();
        id
    }

    /// 改掉某个 tab 记着的启动目录（恢复被删目录之后 / PTY 校正了 cwd 之后）
    pub fn update_tab_cwd(&mut self, tab_id: &str, cwd: &str) {
        let cid = self.locate_tab(tab_id).map(|(c, _)| c.id.clone());
        if let Some(c) = cid.and_then(|cid| find_container_mut(&mut self.state.root, &cid)) {
            if let Some(t) = c.tabs.iter_mut().find(|t| t.id == tab_id) {
                t.cwd = cwd.into();
            }
        }
        self.settle();
    }

    /// shell / new tab 认领到会话（见 `bind_session_to_pane_tab`）
    pub fn bind_session_to_tab(&mut self, container_id: &str, tab_id: &str, session_id: &str, short_id: &str, label: Option<&str>, session_cwd: Option<&str>) {
        if let Some(c) = find_container_mut(&mut self.state.root, container_id) {
            if let Some(t) = c.tabs.iter_mut().find(|t| t.id == tab_id) {
                *t = bind_session_to_pane_tab(t, session_id, short_id, label, session_cwd);
            }
        }
        self.settle();
    }

    /// 拖分割线（调用方把 ratio 限制在 0.1–0.9）
    pub fn set_split_ratio(&mut self, split: &str, ratio: f64) {
        set_split_ratio(&mut self.state.root, split, ratio);
    }

    /// 第 n 个 pane（⌥⌘1–9，按屏幕空间顺序，从 0 数）
    pub fn nth_container(&self, n: usize) -> Option<String> {
        collect_containers(&self.state.root).get(n).map(|c| c.id.clone())
    }

    /// 当前 pane 里切第 n 个 tab（⌘1–9，从 0 数）
    pub fn activate_nth_tab(&mut self, n: usize) {
        let Some(c) = self.active_container() else { return };
        let Some(t) = c.tabs.get(n) else { return };
        let (cid, tid) = (c.id.clone(), t.id.clone());
        self.tab_click(&cid, &tid);
    }

    /// 当前 pane 里上一个 / 下一个 tab（循环）
    pub fn cycle_tab(&mut self, delta: i32) {
        let Some(c) = self.active_container() else { return };
        let n = c.tabs.len() as i32;
        if n == 0 {
            return;
        }
        let cur = c.tabs.iter().position(|t| t.id == c.active_tab_id).unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(n) as usize;
        let (cid, tid) = (c.id.clone(), c.tabs[next].id.clone());
        self.tab_click(&cid, &tid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str) -> PaneTab {
        PaneTab { id: id.into(), kind: TabKind::Shell, cwd: "~".into(), init_command: None, session_id: None, session_short_id: None, label: String::new() }
    }
    fn container(id: &str, tabs: &[&str]) -> LayoutNode {
        LayoutNode::Container(ContainerNode {
            id: id.into(),
            tabs: tabs.iter().map(|t| tab(t)).collect(),
            active_tab_id: tabs.first().map(|s| s.to_string()).unwrap_or_default(),
            tab_history: Vec::new(),
        })
    }
    fn split(a: LayoutNode, b: LayoutNode) -> LayoutNode {
        LayoutNode::Split(SplitNode { dir: Dir::H, ratio: 0.5, a: Box::new(a), b: Box::new(b) })
    }
    fn two_pane() -> LayoutNode {
        split(container("C1", &["t1"]), container("C2", &["t2"]))
    }
    fn norm(root: LayoutNode, active: &str, max: Option<&str>) -> Option<String> {
        let mut ws = WorkspaceState { root, active_container_id: active.into(), maximized_container_id: max.map(String::from) };
        normalize(&mut ws);
        ws.maximized_container_id
    }

    // ---- scripts/test-workspace-invariant.ts ----

    #[test]
    fn maximize_invariant() {
        assert_eq!(norm(two_pane(), "C1", Some("C1")).as_deref(), Some("C1"), "关放大 pane 里的一个 tab（还剩别的）→ 保持放大");
        assert_eq!(norm(two_pane(), "C2", Some("C1")).as_deref(), Some("C2"), "放大 C1 时 ⌥⌘→ 到 C2 → 放大跟到 C2");
        assert_eq!(norm(container("C2", &["t"]), "C2", Some("C1")), None, "放大的 container 整个消失 → 退出放大（① 先于 ②）");
        assert_eq!(norm(two_pane(), "", Some("C1")), None, "焦点是空串 → 退出放大，不把 maximized 指向空 id");
        assert_eq!(norm(two_pane(), "C2", None), None, "未放大 → 原样");
        assert_eq!(norm(two_pane(), "C2", Some("C2")).as_deref(), Some("C2"), "toggleMaximize 开");
        assert_eq!(
            norm(split(container("C1", &["a"]), container("Cnew", &["b"])), "Cnew", None),
            None,
            "split 后 max=null、焦点在新 pane → 不该被重新放大"
        );
        // 幂等
        let mut ws = WorkspaceState { root: two_pane(), active_container_id: "C2".into(), maximized_container_id: Some("C1".into()) };
        normalize(&mut ws);
        let once = ws.clone();
        normalize(&mut ws);
        assert_eq!(ws, once, "幂等：再跑一次不再变化");
    }

    #[test]
    fn maximized_field_absent_round_trips_as_absent() {
        let ws = WorkspaceState { root: two_pane(), active_container_id: "C2".into(), maximized_container_id: None };
        let json = serde_json::to_string(&ws).unwrap();
        assert!(!json.contains("maximizedContainerId"), "未放大时不凭空写出字段：{json}");
    }

    // ---- scripts/test-resume-tab.ts ----

    const SID: &str = "df178096-1111-2222-3333-444455556666";

    #[test]
    fn resume_commands() {
        assert_eq!(resume_cmd(SID, None), format!("claude -r {SID}"));
        assert_eq!(resume_cmd(SID, Some("codex")), format!("codex resume {SID}"));
        assert_eq!(resume_init_command(SID, None), format!("clear && claude -r {SID}"));
        assert_eq!(resume_init_command(SID, Some("codex")), format!("clear && codex resume {SID}"));
    }

    #[test]
    fn binding_a_shell_tab_makes_a_complete_resume_tab() {
        let shell = PaneTab { cwd: "/Users/x/proj".into(), ..tab("t_1") };
        let b = bind_session_to_pane_tab(&shell, SID, "df178096", Some("改个名字"), None);
        assert_eq!(b.kind, TabKind::Resume);
        assert_eq!(b.session_id.as_deref(), Some(SID));
        assert_eq!(b.session_short_id.as_deref(), Some("df178096"));
        assert_eq!(b.init_command, Some(format!("clear && claude -r {SID}")), "认领后必须带 initCommand");
        assert_eq!(b.label, "改个名字");
        assert_eq!(b.cwd, "/Users/x/proj");
        assert_eq!(b.id, "t_1");
        assert_eq!(bind_session_to_pane_tab(&shell, SID, "df178096", None, Some("/Users/x/makit")).cwd, "/Users/x/makit", "#190");
        assert_eq!(bind_session_to_pane_tab(&shell, SID, "df178096", None, Some("")).cwd, "/Users/x/proj", "空的会话目录不覆盖");
        assert_eq!(bind_session_to_pane_tab(&shell, SID, "df178096", None, None).label, "[df178096]");
        assert_eq!(bind_session_to_pane_tab(&shell, SID, "df178096", Some("   "), None).label, "[df178096]");
    }

    #[test]
    fn repair_fixes_only_broken_resume_tabs_even_deep_in_splits() {
        let broken = PaneTab {
            kind: TabKind::Resume,
            session_id: Some(SID.into()),
            session_short_id: Some("df178096".into()),
            label: "[df178096]".into(),
            ..tab("t_2")
        };
        let good = PaneTab { id: "t_3".into(), init_command: Some("clear && codex resume abc".into()), ..broken.clone() };
        let no_sid = PaneTab { id: "t_4".into(), session_id: None, ..broken.clone() };
        let mk = |t: &PaneTab| LayoutNode::Container(ContainerNode { id: format!("c{}", t.id), active_tab_id: t.id.clone(), tabs: vec![t.clone()], tab_history: vec![] });
        let mut ws = WorkspaceState {
            root: split(mk(&tab("t_1")), split(mk(&good), split(mk(&broken), mk(&no_sid)))),
            active_container_id: "ct_1".into(),
            maximized_container_id: None,
        };
        assert!(repair_resume_tabs(&mut ws));
        let get = |id: &str| ws_tab(&ws, id).init_command.clone();
        assert_eq!(get("t_2"), Some(format!("clear && claude -r {SID}")), "分屏深处的坏 tab 也被修");
        assert_eq!(get("t_3").as_deref(), Some("clear && codex resume abc"), "别把 codex 改成 claude");
        assert_eq!(get("t_1"), None, "shell tab 保持 None");
        assert_eq!(get("t_4"), None, "没有 sessionId 不瞎补");
        assert!(!repair_resume_tabs(&mut ws), "修过之后再跑没有改动");
    }

    fn ws_tab<'a>(ws: &'a WorkspaceState, id: &str) -> &'a PaneTab {
        collect_containers(&ws.root).into_iter().flat_map(|c| c.tabs.iter()).find(|t| t.id == id).unwrap()
    }

    /// 源码扫描：恢复命令只在这个文件里拼（TS 版原来有 5 处各拼一遍，第 6 条路漏了）
    #[test]
    fn resume_command_is_only_built_here() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "rs") { out.push(p) }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        let me = src.join("workspace").join("model.rs");
        let needle_a = ["\"claude", " -r "].concat();
        let needle_b = ["\"codex", " resume "].concat();
        let mut bad = Vec::new();
        for f in files.iter().filter(|f| **f != me) {
            for (i, line) in std::fs::read_to_string(f).unwrap().lines().enumerate() {
                if line.contains(&needle_a) || line.contains(&needle_b) {
                    bad.push(format!("{}:{}", f.display(), i + 1));
                }
            }
        }
        assert!(files.len() > 5, "没扫到源码");
        assert!(bad.is_empty(), "恢复命令要用 workspace::model::resume_init_command，别处手拼了：{bad:?}");
    }

    // ---- scripts/test-context-menu.ts（tabsToClose）----

    #[test]
    fn tabs_to_close_anchors_on_right_clicked_tab() {
        let tabs: Vec<PaneTab> = ["a", "b", "c", "d"].iter().map(|s| tab(s)).collect();
        assert_eq!(tabs_to_close(&tabs, "b", CloseScope::Others), ["a", "c", "d"]);
        assert_eq!(tabs_to_close(&tabs, "b", CloseScope::Right), ["c", "d"]);
        assert!(tabs_to_close(&tabs, "d", CloseScope::Right).is_empty());
        assert_eq!(tabs_to_close(&tabs, "a", CloseScope::Right), ["b", "c", "d"]);
        assert!(tabs_to_close(&[tab("a")], "a", CloseScope::Others).is_empty());
        assert!(tabs_to_close(&tabs, "zzz", CloseScope::Others).is_empty(), "锚点不在：不能退化成全关");
        assert!(tabs_to_close(&tabs, "zzz", CloseScope::Right).is_empty());
        assert!(tabs_to_close(&[], "a", CloseScope::Others).is_empty());
    }

    // ---- scripts/test-tree-nav.ts（openedOrder）----

    fn rtab(id: &str, sid: Option<&str>, kind: TabKind) -> PaneTab {
        PaneTab { kind, session_id: sid.map(String::from), ..tab(id) }
    }
    fn ws_of(root: LayoutNode) -> WorkspaceState {
        WorkspaceState { active_container_id: first_container_id(&root).into(), root, maximized_container_id: None }
    }
    fn cont(id: &str, tabs: Vec<PaneTab>) -> LayoutNode {
        LayoutNode::Container(ContainerNode { id: id.into(), active_tab_id: tabs.first().map(|t| t.id.clone()).unwrap_or_default(), tabs, tab_history: vec![] })
    }

    #[test]
    fn opened_order_is_spatial_and_deduped() {
        let r = |id: &str, s: &str| rtab(id, Some(s), TabKind::Resume);
        assert_eq!(opened_order(&ws_of(cont("c", vec![r("1", "a"), r("2", "b"), r("3", "c")]))), ["a", "b", "c"]);
        let two = LayoutNode::Split(SplitNode {
            dir: Dir::V,
            ratio: 0.5,
            a: Box::new(cont("l", vec![r("1", "left1"), r("2", "left2")])),
            b: Box::new(cont("r", vec![r("3", "right1")])),
        });
        assert_eq!(opened_order(&ws_of(two)), ["left1", "left2", "right1"]);
        let nested = LayoutNode::Split(SplitNode {
            dir: Dir::V,
            ratio: 0.5,
            a: Box::new(split(cont("tl", vec![r("1", "topLeft")]), cont("bl", vec![r("2", "bottomLeft")]))),
            b: Box::new(cont("r", vec![r("3", "right")])),
        });
        assert_eq!(opened_order(&ws_of(nested)), ["topLeft", "bottomLeft", "right"], "嵌套分屏也是深度优先的空间顺序");
        let mixed = cont("c", vec![rtab("1", None, TabKind::Shell), r("2", "real"), rtab("3", None, TabKind::New), rtab("4", None, TabKind::Resume)]);
        assert_eq!(opened_order(&ws_of(mixed)), ["real"]);
        let dup = split(cont("a", vec![r("1", "dup"), r("2", "x")]), cont("b", vec![r("3", "dup")]));
        assert_eq!(opened_order(&ws_of(dup)), ["dup", "x"]);
        assert!(opened_order(&ws_of(cont("c", vec![]))).is_empty());
    }

    // ---- useWorkspace 的 handler 行为 ----

    #[test]
    fn close_active_tab_goes_back_through_tab_history() {
        let mut w = Workspace::new(ws_of(container("C", &["a", "b", "c"])));
        w.tab_click("C", "c");
        w.tab_click("C", "b");
        assert_eq!(w.active_tab().unwrap().id, "b");
        assert_eq!(w.close_tab("C", "b"), ["b"]);
        assert_eq!(w.active_tab().unwrap().id, "c", "回到上一个用过的 tab");
        w.close_tab("C", "c");
        assert_eq!(w.active_tab().unwrap().id, "a", "历史里剩下的 a（c 之前点过 a 所在的初始 active）");
        // 关非 active 的 tab 不改 active
        let mut w = Workspace::new(ws_of(container("C", &["a", "b"])));
        w.close_tab("C", "b");
        assert_eq!(w.active_tab().unwrap().id, "a");
    }

    #[test]
    fn closing_last_tab_of_container_returns_to_last_activated_container() {
        // C1 | (C2 / C3)，依次激活 C2 → C3 → C1，然后在 C1 里关掉唯一的 tab
        let root = split(container("C1", &["a"]), split(container("C2", &["b"]), container("C3", &["c"])));
        let mut w = Workspace::new(ws_of(root));
        w.set_active("C2");
        w.set_active("C3");
        w.set_active("C1");
        w.close_tab("C1", "a");
        assert!(find_container(&w.state.root, "C1").is_none(), "空 container 被删掉");
        assert_eq!(w.state.active_container_id, "C3", "回到最近一次激活的那个，不是第一个");
    }

    #[test]
    fn closing_the_very_last_tab_rebuilds_an_empty_workspace() {
        let mut w = Workspace::new(ws_of(container("C", &["a"])));
        w.toggle_maximize("C");
        w.close_tab("C", "a");
        let cs = collect_containers(&w.state.root);
        assert_eq!(cs.len(), 1);
        assert!(cs[0].tabs.is_empty());
        assert_eq!(w.state.active_container_id, cs[0].id);
        assert_eq!(w.state.maximized_container_id, None);
    }

    #[test]
    fn closing_a_tab_in_the_maximized_pane_keeps_it_maximized() {
        let mut w = Workspace::new(ws_of(split(container("C1", &["a", "b"]), container("C2", &["c"]))));
        w.toggle_maximize("C1");
        w.close_tab("C1", "a");
        assert_eq!(w.state.maximized_container_id.as_deref(), Some("C1"));
    }

    #[test]
    fn split_inherits_cwd_focuses_new_pane_and_unmaximizes() {
        let mut root = container("C", &["a"]);
        if let LayoutNode::Container(c) = &mut root {
            c.tabs[0].cwd = "/Users/me/proj".into();
        }
        let mut w = Workspace::new(ws_of(root));
        w.toggle_maximize("C");
        let t = w.split("C", Dir::V);
        let (c, tab) = w.locate_tab(&t).unwrap();
        assert_eq!(tab.cwd, "/Users/me/proj");
        assert_eq!(tab.kind, TabKind::Shell);
        assert_eq!(w.state.active_container_id, c.id);
        assert_eq!(w.state.maximized_container_id, None);
        let LayoutNode::Split(s) = &w.state.root else { panic!("应该变成 split") };
        assert_eq!((s.dir, s.ratio), (Dir::V, 0.5));
        assert_eq!(split_id(s), format!("split-C-{}", c.id));
    }

    #[test]
    fn open_session_switches_to_existing_tab_instead_of_duplicating() {
        let mut w = Workspace::default();
        let Opened::New { tab_id } = w.open_session(SID, "df178096", "/p", None, None) else { panic!() };
        let (_, t) = w.locate_tab(&tab_id).unwrap();
        assert_eq!(t.init_command, Some(format!("clear && claude -r {SID}")));
        assert_eq!(t.label, "[df178096]");
        w.new_shell_in(&w.state.active_container_id.clone());
        let again = w.open_session(SID, "df178096", "/p", None, None);
        assert!(matches!(again, Opened::Existing { tab_id: ref id, .. } if *id == tab_id));
        assert_eq!(w.active_tab().unwrap().id, tab_id);
        assert_eq!(collect_all_tab_ids(&w.state.root).len(), 2);
    }

    #[test]
    fn move_and_reorder_tabs() {
        let mut w = Workspace::new(ws_of(split(container("C1", &["a", "b"]), container("C2", &["c"]))));
        w.move_tab("C1", "a", "C2", Some(0));
        let c2 = find_container(&w.state.root, "C2").unwrap();
        assert_eq!(c2.tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!((c2.active_tab_id.as_str(), w.state.active_container_id.as_str()), ("a", "C2"));
        assert_eq!(find_container(&w.state.root, "C1").unwrap().active_tab_id, "b", "源 container 的 active 换成剩下的第一个");
        w.move_tab("C1", "b", "C2", None);
        assert!(find_container(&w.state.root, "C1").is_none(), "源空了就删掉");
        w.reorder_tab("C2", "b", 0);
        let ids: Vec<String> = find_container(&w.state.root, "C2").unwrap().tabs.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        w.reorder_tab("C2", "b", 3);
        let ids: Vec<String> = find_container(&w.state.root, "C2").unwrap().tabs.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, ["a", "c", "b"], "插到末尾（原数组下标之后要减 1）");
    }

    #[test]
    fn split_with_tab_and_session() {
        let mut w = Workspace::new(ws_of(container("C1", &["a", "b"])));
        w.split_with_tab("C1", "b", "C1", Dir::V, Side::Before);
        let cs = collect_containers(&w.state.root);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].tabs[0].id, "b", "Before：新 container 在左");
        assert_eq!(w.state.active_container_id, cs[0].id);
        let spec = TabSpec { kind: TabKind::Resume, cwd: "/p".into(), init_command: Some(resume_init_command(SID, None)), session_id: Some(SID.into()), session_short_id: Some("df178096".into()) };
        let t = w.split_with_session("C1", Dir::H, Side::After, &spec);
        let (c, tab) = w.locate_tab(&t).unwrap();
        assert_eq!(tab.label, "[df178096]");
        assert_eq!(w.state.active_container_id, c.id);
        assert_eq!(collect_containers(&w.state.root).len(), 3);
    }

    #[test]
    fn layout_and_geometric_navigation() {
        // 左 C1 | 右 (上 C2 / 下 C3)
        let root = LayoutNode::Split(SplitNode {
            dir: Dir::V,
            ratio: 0.5,
            a: Box::new(container("C1", &["a"])),
            b: Box::new(split(container("C2", &["b"]), container("C3", &["c"]))),
        });
        let l = layout_tree(&root, Rect { x: 0.0, y: 0.0, width: 1000.0, height: 800.0 });
        assert_eq!(l.rect("C1").unwrap(), Rect { x: 0.0, y: 0.0, width: 500.0, height: 800.0 });
        assert_eq!(l.rect("C3").unwrap(), Rect { x: 500.0, y: 400.0, width: 500.0, height: 400.0 });
        assert_eq!(l.splits.len(), 2);
        assert_eq!(l.splits.last().unwrap().id, "split-C1-C2", "split id 取两边子树第一个 container");
        assert_eq!(find_nearest_container(&l, "C1", Direction::Right).as_deref(), Some("C2"), "距离相同取先遇到的（上面那个）");
        assert_eq!(find_nearest_container(&l, "C3", Direction::Left).as_deref(), Some("C1"));
        assert_eq!(find_nearest_container(&l, "C3", Direction::Up).as_deref(), Some("C2"));
        assert_eq!(find_nearest_container(&l, "C2", Direction::Up), None);
        assert_eq!(find_nearest_container(&l, "C1", Direction::Left), None);
    }

    #[test]
    fn loads_tauri_localstorage_json_and_migrates_old_format() {
        let json = r#"{"root":{"kind":"split","dir":"v","ratio":0.3,
            "a":{"kind":"container","id":"c_1","tabs":[{"id":"t_1","kind":"resume","cwd":"/p","initCommand":null,
                 "sessionId":"s1","sessionShortId":"s1","label":"x"}],"activeTabId":"t_1","tabHistory":[]},
            "b":{"kind":"container","id":"c_2","tabs":[],"activeTabId":""}},
            "activeContainerId":"c_2","maximizedContainerId":"c_1"}"#;
        let ws = load_workspace(Some(json), None);
        assert_eq!(ws_tab(&ws, "t_1").init_command.as_deref(), Some("clear && claude -r s1"), "读的时候修坏 tab");
        assert_eq!(ws.maximized_container_id.as_deref(), Some("c_2"), "读的时候 normalize：放大跟焦点");
        // 坏 JSON → 默认布局
        let d = load_workspace(Some("{坏"), None);
        assert_eq!(collect_containers(&d.root).len(), 1);
        // 旧格式：两个旧 tab 横向排开
        let old = r#"[{"root":{"kind":"leaf","paneId":"p1"},"panes":[{"paneId":"p1","kind":"resume","cwd":"/a","initCommand":"clear && claude -r s","sessionId":"s","sessionShortId":"s"}],"activePaneId":"p1"},
                      {"root":{"kind":"leaf","paneId":"p2"},"panes":[],"activePaneId":"p2"}]"#;
        let m = load_workspace(None, Some(old));
        let LayoutNode::Split(s) = &m.root else { panic!() };
        assert_eq!((s.dir, s.ratio), (Dir::V, 0.5));
        assert_eq!(ws_tab(&m, "p1").label, "[s]");
        assert_eq!(ws_tab(&m, "p2").cwd, "/", "找不到 pane 信息的 leaf 按 shell 兜底");
    }

    #[test]
    fn ids_have_the_ts_shape_and_do_not_collide() {
        let ids: Vec<String> = (0..500).map(|_| make_tab_id()).collect();
        assert!(ids.iter().all(|i| i.starts_with("t_") && i.len() >= 10));
        let mut s = ids.clone();
        s.sort();
        s.dedup();
        assert_eq!(s.len(), ids.len(), "同一毫秒里连造也不能撞");
        assert!(make_container_id().starts_with("c_"));
    }

    /// 回归：并行跑测试时 ids_have_… 偶发撞 id（旧实现是非单射哈希，36⁴ 空间里有生日碰撞）。
    /// 多线程同时造 2 万个，一个都不能撞
    #[test]
    fn ids_do_not_collide_across_threads() {
        let hs: Vec<_> = (0..8).map(|_| std::thread::spawn(|| (0..2500).map(|_| make_tab_id()).collect::<Vec<_>>())).collect();
        let mut all: Vec<String> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
    }

    #[test]
    fn cycle_and_nth_tab() {
        let mut w = Workspace::new(ws_of(container("C", &["a", "b", "c"])));
        w.cycle_tab(-1);
        assert_eq!(w.active_tab().unwrap().id, "c", "循环");
        w.cycle_tab(1);
        assert_eq!(w.active_tab().unwrap().id, "a");
        w.activate_nth_tab(1);
        assert_eq!(w.active_tab().unwrap().id, "b");
        w.activate_nth_tab(7);
        assert_eq!(w.active_tab().unwrap().id, "b", "越界不动");
    }
}

/// 自检（MAKIT_NATIVE_SELFTEST=layout）里的那个布局：左 | 中 | (右上 / 右下)，从右下 ⌥⌘← 必须到中间，
/// 不能越过它跳到最左（边界相邻、距离 0 的优先）。
#[cfg(test)]
mod nav_regression {
    use super::*;
    #[test]
    fn left_from_bottom_right_goes_to_the_adjacent_middle_pane() {
        let json = r#"{"root":{"kind":"split","dir":"v","ratio":0.4,
          "a":{"kind":"container","id":"c_a","tabs":[],"activeTabId":""},
          "b":{"kind":"container","id":"c_b","tabs":[],"activeTabId":""}},"activeContainerId":"c_b"}"#;
        let mut w = Workspace::new(serde_json::from_str(json).unwrap());
        w.split("c_b", Dir::V);
        let r = w.state.active_container_id.clone();
        w.split(&r, Dir::H);
        let l = layout_tree(&w.state.root, Rect { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 });
        assert_eq!(find_nearest_container(&l, &w.state.active_container_id, Direction::Left).as_deref(), Some("c_b"));
    }
}
