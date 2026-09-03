export type TabKind = "resume" | "new" | "shell";

export type PaneTab = {
  id: string;
  kind: TabKind;
  cwd: string;
  initCommand: string | null;
  sessionId: string | null;
  sessionShortId: string | null;
  label: string;
};

export type ContainerNode = {
  kind: "container";
  id: string;
  tabs: PaneTab[];
  activeTabId: string;
  tabHistory: string[];
};

export type LayoutNode =
  | ContainerNode
  | { kind: "split"; dir: "h" | "v"; ratio: number; a: LayoutNode; b: LayoutNode };

export type WorkspaceState = {
  root: LayoutNode;
  activeContainerId: string;
  // 非空时该 container 占满 workspace，其它隐藏；container 消失会被自动清空
  maximizedContainerId?: string | null;
};

export type SplitNode = Extract<LayoutNode, { kind: "split" }>;

// --- ID 生成 ---

export function makeContainerId(): string {
  return "c_" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

export function makeTabId(): string {
  return "t_" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
}

// --- 树操作 ---

export function findContainer(node: LayoutNode, containerId: string): ContainerNode | null {
  if (node.kind === "container") return node.id === containerId ? node : null;
  return findContainer(node.a, containerId) || findContainer(node.b, containerId);
}

// 放大态的唯一不变量：**「我在看哪个」必须等于「键盘输入去哪」**。
// maximizedContainerId 决定谁铺满、其余 display:none（WorkspaceView），
// activeContainerId 决定键盘输入进谁 —— 两者不等时就出现「操作一个看不见的
// pane」：你打的字进了一个隐藏的终端，屏幕上什么都不动。
//
// 这个不变量以前没有任何一处代码保证，于是有 4 条路径各自破坏它（关 tab、
// 拖 tab、⌥⌘方向键/数字导航、侧栏点一个已在别的 pane 打开的 session）。
// 由 useWorkspace 在 setWorkspace 的出口统一收口，而不是逐个 handler 补 ——
// 那样每加一个 handler 都要记得补一次，漏一个就复现，而 17 个 handler 里
// 已经漏了 2 个。
//
// 两步的顺序不能反：关掉「被放大 container 的最后一个 tab」会同时让 container
// 消失、active 跳到别的 container，此时正确行为是退出放大（①），而不是把新
// 落脚的那个放大（②）。
export function normalizeWorkspace(ws: WorkspaceState): WorkspaceState {
  const maxId = ws.maximizedContainerId;
  if (!maxId) return ws;
  // ① 被放大的 container 已不存在（最后一个 tab 关掉 / 拖走 / 合并）→ 退出放大
  if (!findContainer(ws.root, maxId)) return { ...ws, maximizedContainerId: null };
  // ② 焦点移到了别的 container → 放大跟着焦点走（专注模式内轮流切换）。
  // 焦点自己都落在一个不存在的 id 上时（handleMoveTab 有 `?? ""` 的兜底路径）退出
  // 放大，而不是把 maximized 也指向那个空 id —— 这个函数的产出必须是合法状态，
  // 把校验推给渲染层是把责任放错了地方。
  if (maxId !== ws.activeContainerId) {
    const target = findContainer(ws.root, ws.activeContainerId) ? ws.activeContainerId : null;
    return { ...ws, maximizedContainerId: target };
  }
  return ws;
}

// --- resume tab：命令 + 形状 ---

/// 恢复一个已有会话要敲的命令。**只在这里拼** —— 原来有 5 处各拼一遍
/// （App.tsx 4 处、useWorkspace.openSession 1 处），而第 6 条路
/// （bindSessionToTab）一处都没拼，见下面 bindSessionToPaneTab 的注释。
export function resumeCmd(sessionId: string, tool?: string): string {
  return tool === "codex" ? `codex resume ${sessionId}` : `claude -r ${sessionId}`;
}

/// resume tab 的 initCommand。前面那个 clear 是为了不让 shell 自己的 banner
/// 留在会话上下文的上方。
export function resumeInitCommand(sessionId: string, tool?: string): string {
  return `clear && ${resumeCmd(sessionId, tool)}`;
}

/// 把一个 shell / new tab 升级成 resume tab —— 用户在纯 shell 里手打了 `claude`，
/// 后端从 pid 文件认出了它是哪个 session。
///
/// **必须连 initCommand 一起写**：`kind` 只是 UI 元信息，生成 PTY 的时候根本不看它
/// （TerminalManager 只把 initCommand 交给后端）。少写这一个字段，tab 会同时坏两处：
///   1. 下次启动没有命令可跑，只起一个空 shell —— 「关掉 makit 后不会自动恢复」；
///   2. sessionId 却已经填上了，App 的 findSessionLocation 认为这个会话「已经开着」，
///      点侧栏只会聚焦到那个空 shell —— 「占用恢复的 tab」。
/// 两个症状是同一个漏掉的字段。
///
/// tool 不做参数、固定按 claude 拼：这条路的唯一数据源是
/// `~/.claude/sessions/<pid>.json`（见 lib.rs 的 resolve_bindings_in），
/// 那个文件只有 claude 自己会写，codex 不写。
///
/// 已知不足：tab 的 cwd 还是当初开 shell 时的目录。用户如果先 cd 到别处再敲
/// claude，恢复时会在错的目录下 `claude -r`（claude 的存储键含 cwd，会找不到会话）。
/// 修它需要后端把 claude 进程的真实 cwd 一起报上来，不在这次范围内。
export function bindSessionToPaneTab(
  t: PaneTab,
  sessionId: string,
  shortId: string,
  label?: string,
): PaneTab {
  return {
    ...t,
    kind: "resume",
    sessionId,
    sessionShortId: shortId,
    initCommand: resumeInitCommand(sessionId),
    // label 可选：session 的 jsonl 还没生成时 sessions 里查不到 meta，
    // stableGetTabTitle 会回退到 tab.label —— 这时 claude 自己写的会话名
    // 比 `[shortId]` 有用得多
    label: label && label.trim() ? label : `[${shortId}]`,
  };
}

/// 修掉已经躺在 localStorage 里的坏 tab：kind 是 resume、有 sessionId、却没有
/// initCommand。这种形状只有旧版 bindSessionToTab 会产出，所以同样按 claude 补。
///
/// 只在读取持久化状态时跑一次，**不并进 normalizeWorkspace** —— 那个函数每次
/// setWorkspace 都跑，而这是一次性的数据迁移，不是每次 set 都要维持的不变量。
export function repairResumeTabs(ws: WorkspaceState): WorkspaceState {
  let changed = false;
  function fix(node: LayoutNode): LayoutNode {
    if (node.kind !== "container") return { ...node, a: fix(node.a), b: fix(node.b) };
    return {
      ...node,
      tabs: node.tabs.map((t) => {
        if (t.kind !== "resume" || !t.sessionId || t.initCommand) return t;
        changed = true;
        return { ...t, initCommand: resumeInitCommand(t.sessionId) };
      }),
    };
  }
  const root = fix(ws.root);
  return changed ? { ...ws, root } : ws;
}

// --- tab 批量关闭 ---

export type CloseScope = "others" | "right";

/// 「关闭其他」/「关闭右侧」要关掉哪些 tab，按 tab 条上的**显示顺序**返回。
///
/// 锚点是**右键的那个 tab**，不是 activeTabId —— container 右键菜单的「关闭」
/// 一直用的是 `c.activeTabId`，于是在一个非激活的 tab 上右键点关闭，关掉的是
/// 别人。批量关闭把这个坑放大 N 倍，所以锚点必须由调用方显式传进来。
///
/// 找不到锚点时返回空数组而不是"全关"：调用方拿到的是一串 destroy 调用，
/// 空数组是唯一安全的降级。
export function tabsToClose(tabs: PaneTab[], anchorId: string, scope: CloseScope): string[] {
  const idx = tabs.findIndex((t) => t.id === anchorId);
  if (idx < 0) return [];
  if (scope === "others") return tabs.filter((t) => t.id !== anchorId).map((t) => t.id);
  return tabs.slice(idx + 1).map((t) => t.id);
}

export function collectContainers(node: LayoutNode, out: ContainerNode[] = []): ContainerNode[] {
  if (node.kind === "container") { out.push(node); return out; }
  collectContainers(node.a, out);
  collectContainers(node.b, out);
  return out;
}

/// 「打开中」段的顺序：所有在 tab 里开着的会话 id，按**屏幕上的空间顺序**。
///
/// 排序不用 mtime：这段对应的是用户屏幕上的窗口布局，「刚动过的排前面」会让同一屏的
/// 几个 pane 在侧栏里每次都换位置，切会话时反而找不到。`collectContainers` 先 a 后 b
/// 的遍历顺序天然就是左/上先于右/下，直接用。
///
/// 去重是为了 React key：同一条会话理论上不会开在两个 pane（openResumeTab 会切过去
/// 而不是再开一个），但坏掉的持久化状态里出现过，重复 key 会让这段渲染行为诡异。
export function openedOrder(ws: WorkspaceState): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const c of collectContainers(ws.root)) {
    for (const t of c.tabs) {
      if (t.kind !== "resume" || !t.sessionId || seen.has(t.sessionId)) continue;
      seen.add(t.sessionId);
      out.push(t.sessionId);
    }
  }
  return out;
}

export function collectAllTabIds(node: LayoutNode): string[] {
  const ids: string[] = [];
  for (const c of collectContainers(node)) {
    for (const t of c.tabs) ids.push(t.id);
  }
  return ids;
}

export function updateContainer(node: LayoutNode, containerId: string, updater: (c: ContainerNode) => LayoutNode): LayoutNode {
  if (node.kind === "container") {
    return node.id === containerId ? updater(node) : node;
  }
  return { ...node, a: updateContainer(node.a, containerId, updater), b: updateContainer(node.b, containerId, updater) };
}

export function removeContainer(node: LayoutNode, containerId: string): LayoutNode | null {
  if (node.kind === "container") return node.id === containerId ? null : node;
  const a = removeContainer(node.a, containerId);
  const b = removeContainer(node.b, containerId);
  if (a && b) return { ...node, a, b };
  return a ?? b ?? null;
}

export function replaceContainer(node: LayoutNode, containerId: string, replacement: LayoutNode): LayoutNode {
  if (node.kind === "container") return node.id === containerId ? replacement : node;
  return { ...node, a: replaceContainer(node.a, containerId, replacement), b: replaceContainer(node.b, containerId, replacement) };
}

export function setSplitRatio(node: LayoutNode, targetId: string, ratio: number): LayoutNode {
  if (node.kind === "container") return node;
  const nodeId = getSplitId(node);
  if (nodeId === targetId) return { ...node, ratio };
  return { ...node, a: setSplitRatio(node.a, targetId, ratio), b: setSplitRatio(node.b, targetId, ratio) };
}

// split 的 id 必须与 layoutTree 生成的一致：都取子树里"第一个 container"的 id
// （layoutTree 用 Object.keys(subtree.containers)[0]，即最左/最上的 container）。
// 之前这里对 split 子节点递归拼接 split 字符串，导致嵌套分屏时 id 不匹配、拖动无效。
function firstContainerId(node: LayoutNode): string {
  return node.kind === "container" ? node.id : firstContainerId(node.a);
}

function getSplitId(node: LayoutNode): string {
  if (node.kind === "container") return node.id;
  return `split-${firstContainerId(node.a)}-${firstContainerId(node.b)}`;
}

// --- 高级操作 ---

export function splitContainer(root: LayoutNode, containerId: string, dir: "h" | "v", newContainer: ContainerNode): LayoutNode {
  return updateContainer(root, containerId, (c) => ({
    kind: "split",
    dir,
    ratio: 0.5,
    a: c,
    b: newContainer,
  }));
}

export function addTabToContainer(root: LayoutNode, containerId: string, tab: PaneTab): LayoutNode {
  return updateContainer(root, containerId, (c) => ({
    ...c,
    tabs: [...c.tabs, tab],
    activeTabId: tab.id,
  }));
}

export function closeTab(root: LayoutNode, containerId: string, tabId: string): { root: LayoutNode | null; destroyedTabIds: string[] } {
  const container = findContainer(root, containerId);
  if (!container) return { root, destroyedTabIds: [] };
  const remaining = container.tabs.filter((t) => t.id !== tabId);
  if (remaining.length === 0) {
    const newRoot = removeContainer(root, containerId);
    return { root: newRoot, destroyedTabIds: [tabId] };
  }
  let nextActive = container.activeTabId;
  if (container.activeTabId === tabId) {
    const history = (container.tabHistory || []).filter((id) => id !== tabId && remaining.some((t) => t.id === id));
    nextActive = history.length > 0 ? history[history.length - 1] : remaining[0].id;
  }
  const newHistory = (container.tabHistory || []).filter((id) => id !== tabId);
  const updated = updateContainer(root, containerId, () => ({
    ...container,
    tabs: remaining,
    activeTabId: nextActive,
    tabHistory: newHistory,
  }));
  return { root: updated, destroyedTabIds: [tabId] };
}

export function moveTab(root: LayoutNode, srcContainerId: string, tabId: string, destContainerId: string): { root: LayoutNode | null; removedSrcContainer: boolean } {
  const src = findContainer(root, srcContainerId);
  const dest = findContainer(root, destContainerId);
  if (!src || !dest) return { root, removedSrcContainer: false };
  const tab = src.tabs.find((t) => t.id === tabId);
  if (!tab) return { root, removedSrcContainer: false };
  const srcRemaining = src.tabs.filter((t) => t.id !== tabId);
  let result: LayoutNode = root;
  if (srcRemaining.length === 0) {
    result = removeContainer(result, srcContainerId)!;
    if (!result) return { root: null, removedSrcContainer: true };
  } else {
    const nextActive = src.activeTabId === tabId ? srcRemaining[0].id : src.activeTabId;
    result = updateContainer(result, srcContainerId, () => ({ ...src, tabs: srcRemaining, activeTabId: nextActive }));
  }
  result = addTabToContainer(result, destContainerId, tab);
  return { root: result, removedSrcContainer: srcRemaining.length === 0 };
}

// --- Rect 计算（复用 layoutTree 思路）---

export type Rect = { x: number; y: number; width: number; height: number };
export type SplitInfo = { id: string; dir: "h" | "v"; rect: Rect; outerRect: Rect; node: SplitNode };
export type LayoutResult = { containers: Record<string, Rect>; splits: SplitInfo[] };

const RESIZER_PX = 0;

export function layoutTree(node: LayoutNode, rect: Rect): LayoutResult {
  if (node.kind === "container") {
    return { containers: { [node.id]: rect }, splits: [] };
  }
  let aRect: Rect, bRect: Rect, splitRect: Rect;
  if (node.dir === "v") {
    const usable = Math.max(0, rect.width - RESIZER_PX);
    const aWidth = usable * node.ratio;
    aRect = { x: rect.x, y: rect.y, width: aWidth, height: rect.height };
    splitRect = { x: rect.x + aWidth, y: rect.y, width: RESIZER_PX, height: rect.height };
    bRect = { x: rect.x + aWidth + RESIZER_PX, y: rect.y, width: usable - aWidth, height: rect.height };
  } else {
    const usable = Math.max(0, rect.height - RESIZER_PX);
    const aHeight = usable * node.ratio;
    aRect = { x: rect.x, y: rect.y, width: rect.width, height: aHeight };
    splitRect = { x: rect.x, y: rect.y + aHeight, width: rect.width, height: RESIZER_PX };
    bRect = { x: rect.x, y: rect.y + aHeight + RESIZER_PX, width: rect.width, height: usable - aHeight };
  }
  const a = layoutTree(node.a, aRect);
  const b = layoutTree(node.b, bRect);
  const splitId = "split-" + Object.keys(a.containers)[0] + "-" + Object.keys(b.containers)[0];
  return {
    containers: { ...a.containers, ...b.containers },
    splits: [
      ...a.splits,
      ...b.splits,
      { id: splitId, dir: node.dir, rect: splitRect, outerRect: rect, node },
    ],
  };
}

// --- 几何方向导航（对标产品/bonsplit 风格：边界相邻 + 重叠优先） ---

export type Direction = "up" | "down" | "left" | "right";

/**
 * 在 layout 结果中，找指定方向上最佳邻居 container。
 * 算法（参考 对标产品/bonsplit findBestNeighbor）：
 *   1. 过滤：候选边界在当前 container 指定方向外侧
 *   2. 评分：垂直轴重叠量（overlap）+ 边界距离（distance）
 *   3. 排序：overlap 大优先，相同取 distance 小
 */
export function findNearestContainer(
  layout: LayoutResult,
  activeId: string,
  direction: Direction
): string | null {
  const rects = layout.containers;
  const cur = rects[activeId];
  if (!cur) return null;

  type Candidate = { id: string; overlap: number; distance: number };
  const candidates: Candidate[] = [];

  // 用中心点判断方向（不受 resizer 间隙影响）
  const cx = cur.x + cur.width / 2;
  const cy = cur.y + cur.height / 2;

  for (const [id, r] of Object.entries(rects)) {
    if (id === activeId) continue;
    const ox = r.x + r.width / 2;
    const oy = r.y + r.height / 2;

    let inDirection = false;
    switch (direction) {
      case "left":  inDirection = ox < cx; break;
      case "right": inDirection = ox > cx; break;
      case "up":    inDirection = oy < cy; break;
      case "down":  inDirection = oy > cy; break;
    }
    if (!inDirection) continue;

    let overlap: number;
    let distance: number;
    if (direction === "left" || direction === "right") {
      overlap = Math.max(0, Math.min(cur.y + cur.height, r.y + r.height) - Math.max(cur.y, r.y));
      distance = direction === "left" ? (cur.x - (r.x + r.width)) : (r.x - (cur.x + cur.width));
    } else {
      overlap = Math.max(0, Math.min(cur.x + cur.width, r.x + r.width) - Math.max(cur.x, r.x));
      distance = direction === "up" ? (cur.y - (r.y + r.height)) : (r.y - (cur.y + cur.height));
    }

    candidates.push({ id, overlap, distance });
  }

  if (candidates.length === 0) return null;

  // 有重叠的优先（无重叠 = 不在同一"行/列"），同等情况按距离排
  candidates.sort((a, b) => {
    const aHas = a.overlap > 0 ? 1 : 0;
    const bHas = b.overlap > 0 ? 1 : 0;
    if (aHas !== bHas) return bHas - aHas;
    return a.distance - b.distance;
  });

  return candidates[0].id;
}

// --- 迁移旧数据 ---

type OldTabState = {
  id: string;
  kind: TabKind;
  projectRoot: string;
  cwd: string;
  label: string;
  sessionId: string | null;
  sessionShortId: string | null;
  initCommand: string | null;
  startedAt: number;
  panes: Array<{
    paneId: string;
    kind: TabKind;
    cwd: string;
    initCommand: string | null;
    sessionId: string | null;
    sessionShortId: string | null;
  }>;
  root: any;
  activePaneId: string;
};

function flattenOldPaneTree(node: any, panes: OldTabState["panes"]): LayoutNode {
  if (!node) return { kind: "container", id: makeContainerId(), tabs: [], activeTabId: "", tabHistory: [] };
  if (node.kind === "leaf") {
    const pane = panes.find((p) => p.paneId === node.paneId);
    const tab: PaneTab = pane
      ? { id: pane.paneId, kind: pane.kind, cwd: pane.cwd, initCommand: pane.initCommand, sessionId: pane.sessionId, sessionShortId: pane.sessionShortId, label: pane.sessionShortId ? `[${pane.sessionShortId}]` : "" }
      : { id: node.paneId, kind: "shell", cwd: "/", initCommand: null, sessionId: null, sessionShortId: null, label: "" };
    return { kind: "container", id: makeContainerId(), tabs: [tab], activeTabId: tab.id, tabHistory: [] };
  }
  if (node.kind === "split") {
    return { kind: "split", dir: node.dir, ratio: node.ratio, a: flattenOldPaneTree(node.a, panes), b: flattenOldPaneTree(node.b, panes) };
  }
  return { kind: "container", id: makeContainerId(), tabs: [], activeTabId: "", tabHistory: [] };
}

export function migrateWorkspace(oldTabs: OldTabState[]): WorkspaceState {
  if (oldTabs.length === 0) {
    const id = makeContainerId();
    return { root: { kind: "container", id, tabs: [], activeTabId: "", tabHistory: [] }, activeContainerId: id };
  }
  if (oldTabs.length === 1) {
    const t = oldTabs[0];
    const root = flattenOldPaneTree(t.root, t.panes);
    const containers = collectContainers(root);
    const active = containers.find((c) => c.tabs.some((tab) => tab.id === t.activePaneId));
    return { root, activeContainerId: active?.id ?? containers[0]?.id ?? "" };
  }
  // 多个旧 tab → 横向排列
  let combined: LayoutNode = flattenOldPaneTree(oldTabs[0].root, oldTabs[0].panes);
  for (let i = 1; i < oldTabs.length; i++) {
    const next = flattenOldPaneTree(oldTabs[i].root, oldTabs[i].panes);
    combined = { kind: "split", dir: "v", ratio: i / (i + 1), a: combined, b: next };
  }
  const containers = collectContainers(combined);
  return { root: combined, activeContainerId: containers[0]?.id ?? "" };
}

export function isWorkspaceState(raw: any): raw is WorkspaceState {
  return raw && typeof raw === "object" && raw.root && typeof raw.activeContainerId === "string";
}
