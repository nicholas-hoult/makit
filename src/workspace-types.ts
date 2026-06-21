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

export function collectContainers(node: LayoutNode, out: ContainerNode[] = []): ContainerNode[] {
  if (node.kind === "container") { out.push(node); return out; }
  collectContainers(node.a, out);
  collectContainers(node.b, out);
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

function getSplitId(node: LayoutNode): string {
  if (node.kind === "container") return node.id;
  const aId = node.a.kind === "container" ? node.a.id : getSplitId(node.a);
  const bId = node.b.kind === "container" ? node.b.id : getSplitId(node.b);
  return `split-${aId}-${bId}`;
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
