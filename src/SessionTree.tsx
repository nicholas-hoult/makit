import { memo, useEffect, useState, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";

import { relativeTime } from "./relativeTime";
import { isCmd } from "./keys";
import {
  projectCollapseKey, isProjectCollapsed, toggleProjectCollapsed, expandProjectForReveal,
  collapseOtherProjects,
} from "./projectCollapse";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { resumeCmd } from "./workspace-types";
import { moveSelection } from "./treeNav";
import { visibleSessionOrder } from "./treeOrder";
import { runState, runStateIcon, runStateClass, runStateTitle, anyAlive } from "./sessionStatus";

type SessionMeta = {
  session_id: string;
  short_id: string;
  cwd: string;
  last_cwd: string;
  git_branch: string;
  user_msg_count: number;
  first_user_msg: string;
  last_user_msg: string;
  mtime: number;
  mtime_display: string;
  humanize: string;
  is_worktree: boolean;
  git_root: string;
  display_name: string;
  name_source: string;
  running: boolean;
  status: string;
  waiting_for: string;
  pid: number;
  child_processes: { pid: number; ppid: number; command: string }[];
  archived: boolean;
  storage_folder: string;
  pty_id: string;
  tool: string;
};

type SortKey = "recent" | "count" | "firstMsg";

type Props = {
  sessions: SessionMeta[];
  pinnedSessions: Set<string>;
  activeSessionId: string | null;
  loading: boolean;
  query: string;
  onQueryChange: (q: string) => void;
  onSessionClick: (s: SessionMeta) => void;
  onSessionDragStart: (e: React.DragEvent, s: SessionMeta) => void;
  onTogglePin: (id: string) => void;
  onArchive: (id: string) => void;
  onRefresh: () => void;
  onSettings: () => void;
  refreshing: boolean;
  revealTrigger: number;
  clearFilterTrigger: number;
  /* 搜索框的 ref 由 App 持有：⌘⇧F 要做的是「展开侧栏 + 聚焦输入框」两件事，
     展开的 state 在 App 手里。不用 trigger 数字传信号是因为 collapsed 时本组件
     直接 return null，重挂载会让 effect 再跑一次、把焦点从终端抢走。 */
  searchRef: React.RefObject<HTMLInputElement | null>;
  collapsed: boolean;
  onCollapse: () => void;
  width: number;
  onResizeStart: (x: number) => void;
  onResizerHover?: (hovered: boolean) => void;
  showAttention?: boolean;
  onNewSessionInDir?: (cwd: string, tool?: string) => void;
  onNewShellInDir?: (cwd: string) => void;
  /// 右键「在左右/上下分屏打开」。可选：没传就在菜单里灰掉那两项。
  onOpenSessionInSplit?: (s: SessionMeta, dir: "h" | "v") => void;
  /// 在 tab 里开着的会话 id，按屏幕上的空间顺序（App 传 openedOrder(ws.workspace)）。
  /// 「打开中」段的唯一数据源。activeSessionId 只有一条，答不了「我手上开着哪几个」。
  openedSessionIds: string[];
  /// Esc 把焦点还给终端。可选：没传时 Esc 只清选中。
  onReturnFocus?: () => void;
};

const RECENT_PAGE = 50;

function basename(path: string): string {
  const parts = path.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || path;
}

type ProjectGroup = {
  name: string;
  gitRoot: string;
  sessions: SessionMeta[];
  hasActive: boolean;
  toolSet: string[];
};

function groupByGitRoot(list: SessionMeta[]): ProjectGroup[] {
  const map = new Map<string, SessionMeta[]>();
  for (const s of list) {
    const key = s.git_root || "";
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(s);
  }
  const groups: ProjectGroup[] = [];
  for (const [gitRoot, sessions] of map) {
    const name = gitRoot ? basename(gitRoot) : basename(sessions[0]?.cwd || "其他") || "其他";
    // 「组里有活的」这一问在三个地方要用：默认折不折（projectCollapse）、组的排序、
    // 以及要不要给这组画状态列。所以走 anyAlive 一个定义，别在渲染层再写一份。
    const hasActive = anyAlive(sessions);
    const sorted = [...sessions].sort((a, b) => b.mtime - a.mtime);
    const toolSet = [...new Set(sessions.map((s) => s.tool))];
    groups.push({ name, gitRoot, sessions: sorted, hasActive, toolSet });
  }
  groups.sort((a, b) => {
    if (a.hasActive !== b.hasActive) return a.hasActive ? -1 : 1;
    const aMax = Math.max(...a.sessions.map((s) => s.mtime));
    const bMax = Math.max(...b.sessions.map((s) => s.mtime));
    return bMax - aMax;
  });
  return groups;
}

export const SessionTree = memo(function SessionTree({
  sessions, pinnedSessions, activeSessionId, loading, query, onQueryChange,
  onSessionClick, onSessionDragStart,
  onTogglePin, onArchive, onRefresh, onSettings, refreshing, revealTrigger, clearFilterTrigger,
  searchRef, collapsed, onCollapse, width, onResizeStart, onResizerHover, onNewSessionInDir, onNewShellInDir,
  onOpenSessionInSplit, openedSessionIds, onReturnFocus,
}: Props) {
  const [sortKey, setSortKey] = useState<SortKey>(() => (localStorage.getItem("makit-tree-sort") as SortKey) || "recent");
  const [showPinnedOnly, setShowPinnedOnly] = useState(() => localStorage.getItem("makit-tree-pinned-only") === "true");
  const [showArchived, setShowArchived] = useState(() => localStorage.getItem("makit-tree-show-archived") === "true");
  const [showRunningOnly, setShowRunningOnly] = useState(() => localStorage.getItem("makit-tree-running-only") === "true");
  const [viewMode, setViewMode] = useState<"status" | "project">(
    () => (localStorage.getItem("makit-sidebar-view") as "status" | "project") || "status"
  );
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem("makit-proj-collapsed");
      return new Set(saved ? JSON.parse(saved) : []);
    } catch {
      return new Set();
    }
  });
  // 状态分组折没折。编码只需要一侧（在集合里 = 折叠），不像 projectCollapse 那样
  // 要存两侧 —— 项目组的默认值不是常量（组里有活跃进程时默认展开），状态组的
  // 默认值恒为「展开」，所以「显式展开」和「默认展开」没有区别，不需要区分。
  //
  // 键用组 id（"opened" / "attention" / …）而不是显示出来的标签：标签是会改文案的
  // （「打开中」刚改成「已打开」，「运行中」改成「后台运行」），拿标签当键会让每次
  // 改文案都静默把用户的折叠状态重置掉。
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem("makit-group-collapsed");
      return new Set<string>(saved ? JSON.parse(saved) : []);
    } catch { return new Set(); }
  });
  useEffect(() => {
    localStorage.setItem("makit-group-collapsed", JSON.stringify([...collapsedGroups]));
  }, [collapsedGroups]);
  function toggleGroup(id: string) {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }
  const [filterOpen, setFilterOpen] = useState(false);
  const [toolPicker, setToolPicker] = useState<{ cwd: string; tools: string[]; x: number; y: number } | null>(null);
  const [logoUrls, setLogoUrls] = useState<Record<string, string>>({});
  useEffect(() => {
    for (const tool of ["claude", "codex"]) {
      invoke<string>("get_tool_logo", { tool }).then((url) => {
        setLogoUrls((prev) => ({ ...prev, [tool]: url }));
      }).catch(() => {});
    }
  }, []);
  // 侧栏的两种右键菜单共用一个 state（外壳是 ContextMenu，全 app 一份）。
  // session 那条存整个 meta 而不是 id：它是这一帧列表里的对象，菜单里要用的
  // cwd / archived / tool 都在上面，再按 id 回查一遍没有意义。
  type TreeMenu =
    | { kind: "session"; x: number; y: number; session: SessionMeta }
    | { kind: "project"; x: number; y: number; colKey: string; name: string; cwd: string };
  const [contextMenu, setContextMenu] = useState<TreeMenu | null>(null);
  const [hoverCard, setHoverCard] = useState<{ x: number; y: number; session: SessionMeta } | null>(null);
  const [visibleRecent, setVisibleRecent] = useState(RECENT_PAGE);
  // 键盘选中。和 activeSessionId（当前打开的那条）是两个正交状态，会同时落在同一行上，
  // 所以视觉通道也必须分开（.selected 用 outline，.focused 用背景 + 左竖条）。
  // 「选中 ≠ 打开」正是原来做不了键盘导航的原因：单击直接打开，中间没有落脚点。
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const treeBodyRef = useRef<HTMLDivElement>(null);
  const asideRef = useRef<HTMLElement>(null);

  useEffect(() => { localStorage.setItem("makit-tree-sort", sortKey); }, [sortKey]);
  useEffect(() => { localStorage.setItem("makit-tree-pinned-only", String(showPinnedOnly)); }, [showPinnedOnly]);
  useEffect(() => { localStorage.setItem("makit-tree-show-archived", String(showArchived)); }, [showArchived]);
  useEffect(() => { localStorage.setItem("makit-tree-running-only", String(showRunningOnly)); }, [showRunningOnly]);
  useEffect(() => { localStorage.setItem("makit-sidebar-view", viewMode); }, [viewMode]);
  useEffect(() => {
    localStorage.setItem("makit-proj-collapsed", JSON.stringify([...collapsedProjects]));
  }, [collapsedProjects]);

  useEffect(() => {
    if (!clearFilterTrigger) return;
    setShowPinnedOnly(false);
    setShowArchived(false);
    setShowRunningOnly(false);
    setVisibleRecent(RECENT_PAGE);
    onQueryChange("");
  }, [clearFilterTrigger]);

  function sortFn(a: SessionMeta, b: SessionMeta): number {
    switch (sortKey) {
      case "recent": return b.mtime - a.mtime;
      case "count": return b.user_msg_count - a.user_msg_count;
      case "firstMsg": return (a.first_user_msg || "").localeCompare(b.first_user_msg || "");
      default: return 0;
    }
  }

  // 全局过滤：归档/仅运行/仅置顶/搜索
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions.filter((s) => {
      if (!showArchived && s.archived) return false;
      if (showRunningOnly && !s.running && s.status !== "waiting") return false;
      if (showPinnedOnly && !pinnedSessions.has(s.session_id)) return false;
      if (q) {
        const hit =
          s.display_name?.toLowerCase().includes(q) ||
          s.short_id.toLowerCase().includes(q) ||
          s.first_user_msg?.toLowerCase().includes(q) ||
          s.git_branch?.toLowerCase().includes(q) ||
          s.git_root?.toLowerCase().includes(q) ||
          s.cwd?.toLowerCase().includes(q);
        if (!hit) return false;
      }
      return true;
    });
  }, [sessions, query, showArchived, showRunningOnly, showPinnedOnly, pinnedSessions]);

  // 「在 tab 里开着」的会话 → 排位。用 Map 而不是 Set：这一组的排序键就是它。
  const openedIndex = useMemo(() => {
    const m = new Map<string, number>();
    openedSessionIds.forEach((id, i) => m.set(id, i));
    return m;
  }, [openedSessionIds]);

  // 分组：一条 session 只出现在它符合的**最靠前**那一组里，靠一个累积的 taken
  // 集合实现「先到先得」。不在每组各写一遍排除条件 —— 那样每加一组就要回头改所有
  // 下游的 filter，漏一个就是同一条 session 显示两遍（上一轮 divider 的漏洞就是
  // 这么来的：分隔条件写成两两组合，加了组之后漏掉一对）。
  //
  // 为什么「运行中」值得单独成一组，而不是像以前那样让它按 mtime 自然排位：
  // 实测本机 253 条 session 里只有 7 条进程还活着（2.8%），而排序只有 mtime
  // 这一个键 —— 于是 11 条**已经死掉的**历史排在 2 个活着的 session 前面
  // （实测排位 #1 #2 #6 #7 #8 #17 #18，中间夹的全是死的）。「进程还活着」是一个
  // 和 mtime 完全独立的维度，且差别是实质的：活的点进去立刻接着用，死的要
  // `claude -r` 重启、重载上下文。以前只把 waiting 提了上来，把 running 漏了。
  const groups = useMemo(() => {
    const taken = new Set<string>();
    // 紧急的两组固定按 mtime：它们是拿来盯的，「刚动过的在前」是唯一合理的顺序，
    // 用户挑的 sortKey（消息数 / 首条消息）对监控没有意义。历史组才尊重 sortKey。
    const byRecent = (a: SessionMeta, b: SessionMeta) => b.mtime - a.mtime;
    const take = (pred: (s: SessionMeta) => boolean, sorter: (a: SessionMeta, b: SessionMeta) => number) => {
      const out = filtered.filter((s) => !taken.has(s.session_id) && pred(s));
      for (const s of out) taken.add(s.session_id);
      return out.sort(sorter);
    };
    // 「打开中」插在链条最前面，于是「已打开的不在下面重复出现」由 taken 机制免费保证，
    // 不需要在每个下游组里各写一遍排除条件。
    //
    // 它排在「需要回应」前面不是因为更紧急，而是因为它是**导航面板**而不是监控项：
    // 用户一天几十次要在自己开着的 2~6 个会话之间切，这是侧栏最高频的动作，而
    // 「谁在等我」是被通知驱动的（有铃铛、有系统通知），不靠扫这一列发现。
    //
    // 排序按 openedIndex（= container/tab 顺序 = 屏幕布局），不按 mtime：见 openedOrder。
    return {
      opened: take((s) => openedIndex.has(s.session_id), (a, b) => openedIndex.get(a.session_id)! - openedIndex.get(b.session_id)!),
      attention: take((s) => s.status === "waiting", byRecent),
      running: take((s) => s.running, byRecent),
      pinned: take((s) => pinnedSessions.has(s.session_id), sortFn),
      others: take(() => true, sortFn),
    };
  }, [filtered, pinnedSessions, sortKey, openedIndex]);

  // 分页/reveal 只作用在历史组：前三组实测加起来十几条（waiting + 活着的 7 条 +
  // 收藏），分页它们没有意义，而历史组是 200+ 条的那一段。
  const history = groups.others;
  const historyVisible = useMemo(() => history.slice(0, visibleRecent), [history, visibleRecent]);

  // 项目视图也排除已打开的：「打开中」段两个视图都常驻置顶，视图切换只影响它下面
  // 那部分。这强化了「上面这段是常驻导航面板，下面才是仓库」的语义。
  const projectGroups = useMemo(() => {
    if (viewMode !== "project") return [];
    return groupByGitRoot(filtered.filter((s) => !openedIndex.has(s.session_id)));
  }, [filtered, viewMode, openedIndex]);

  // 带标签的分组，**渲染和键盘导航共用这一份**。
  //
  // 为什么要有这个中间层，而不是两边各写一遍组的顺序：折叠功能让「这一帧渲染了哪些
  // 行」不再等于「有哪些 session」，而 visibleOrder 的定义严格是前者。两边各写一遍
  // 必然漂 —— 折叠了一组却忘了从 visibleOrder 里摘掉，表现是 ↑↓ 走到一个不存在的行
  // 上、选中框凭空消失，而这种 bug 在 UI 上看不出来源。共用一份之后漂不了。
  //
  // opened 单独拿出来是因为它两个视图都常驻置顶（它是导航面板，不属于任何分组方式）。
  // 历史组不在这里：它没有标签、没有可点的表头，也不该有 —— 它是默认档，折叠它等于
  // 把整列清空。
  const openedGroup = useMemo(() => ({ id: "opened", label: "已打开", list: groups.opened }), [groups.opened]);
  const labeledGroups = useMemo(() => [
    { id: "attention", label: "需要回应", list: groups.attention, tone: "warn" as const },
    { id: "running", label: "后台运行", list: groups.running },
    { id: "pinned", label: "收藏", list: groups.pinned },
  ], [groups.attention, groups.running, groups.pinned]);

  // 键盘导航要求一个**显式的渲染顺序**。原来各组各自 map、没有全局顺序概念 ——
  // 这是当前代码做不到 ↑↓ 的结构原因，不是「忘了加 handler」。
  //
  // 顺序本身算在 treeOrder.ts 里（纯函数、可测），这里只负责把「这一帧的组」喂给它：
  // 折叠的分组（状态组和项目组都算）里的 session 连 <li> 都没有，历史组只喂
  // visibleRecent 那一页。所以 ↑↓ 走到分页边界会停住 —— 自动翻页会让「按住 ↓」变成
  // 无限滚动，用户彻底失去位置感；翻页仍归滚动和「加载更多」。
  const visibleOrder = useMemo(
    () =>
      visibleSessionOrder({
        opened: openedGroup,
        viewMode,
        labeled: labeledGroups,
        history: historyVisible,
        projects: projectGroups.map((g) => ({
          collapsed: isProjectCollapsed(collapsedProjects, projectCollapseKey(g.gitRoot, g.name), g.hasActive),
          sessions: g.sessions,
        })),
        collapsedGroups,
      }),
    [openedGroup, labeledGroups, historyVisible, projectGroups, viewMode, collapsedProjects, collapsedGroups],
  );

  // 选中的是 id，但 Enter / Space 要的是整个 meta。选中项一定在 filtered 里
  // （visibleOrder ⊆ filtered），所以这一份就够。
  const sessionById = useMemo(() => {
    const m = new Map<string, SessionMeta>();
    for (const s of filtered) m.set(s.session_id, s);
    return m;
  }, [filtered]);

  // reveal（⌘L）：先让 active session **真的被渲染出来**，再滚过去。
  //
  // 「渲染出来」在两个视图里是两件不同的事，原来只做了前一件：
  //   状态视图 —— 历史组分页，要翻到它那一页；
  //   项目视图 —— 它所在的项目组可能是折叠的，折叠的组连 <li> 都不渲染，
  //               querySelector 拿回 null，`el?.scrollIntoView()` 静默什么也不做。
  // 后者就是「⌘L 不会把左边关掉的文件夹展开去定位，必须自己点开项目目录」。
  //
  // 注意 hasActive 说的是「组里有 running/busy/waiting 的进程」，**和当前选中哪个无关** ——
  // 所以定位到一个已停止的 session 时，它所在的组默认就是折叠的，这条路必走。
  //
  // ⌘L 顺带把焦点收进侧栏并选中当前会话：⌘L 本来就是「定位到我在用的这条」，
  // 定位完接着按 ↑↓ 走是自然延续，否则用户还得先用鼠标点一下才能用键盘。
  useEffect(() => {
    if (!revealTrigger || !activeSessionId) return;
    setSelectedId(activeSessionId);
    asideRef.current?.focus();
    const idx = history.findIndex((s) => s.session_id === activeSessionId);
    if (idx >= visibleRecent) {
      setVisibleRecent(Math.ceil((idx + 1) / RECENT_PAGE) * RECENT_PAGE);
    }
    const group = projectGroups.find((g) => g.sessions.some((s) => s.session_id === activeSessionId));
    if (group) {
      const colKey = projectCollapseKey(group.gitRoot, group.name);
      setCollapsedProjects((prev) => expandProjectForReveal(prev, colKey, group.hasActive));
    }
    // 状态视图的分组也可折叠，同一个陷阱：目标在折叠的组里就没有 <li>，下面那个
    // querySelector 会一直拿到 null，重试 10 帧之后静默放弃。
    //
    // 这里直接展开、不像 expandProjectForReveal 那样区分「默认展开」和「显式展开」：
    // 状态组的默认值恒为展开，集合里有它就一定是用户自己折的，删掉即是展开。
    const holder = [openedGroup, ...labeledGroups].find(
      (g) => collapsedGroups.has(g.id) && g.list.some((s) => s.session_id === activeSessionId)
    );
    if (holder) {
      setCollapsedGroups((prev) => {
        const next = new Set(prev);
        next.delete(holder.id);
        return next;
      });
    }
    // 展开/翻页都是 setState，一个 rAF 可能早于 React 提交这次渲染。
    // 重试到元素出现为止（和 App.tsx 的 revealSidebarSession 同一套写法）。
    let tries = 0;
    let raf = 0;
    const tick = () => {
      const el = treeBodyRef.current?.querySelector(".tree-session.focused") as HTMLElement | null;
      if (el) {
        el.scrollIntoView({ block: "center", behavior: "smooth" });
        return;
      }
      if (tries++ < 10) raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [revealTrigger]);

  // 滚动加载：滚到底部 200px 内自动加载下一页
  useEffect(() => {
    const body = treeBodyRef.current;
    if (!body) return;
    function onScroll() {
      if (!body) return;
      if (body.scrollTop + body.clientHeight + 200 >= body.scrollHeight) {
        if (visibleRecent < history.length) {
          setVisibleRecent((v) => Math.min(v + RECENT_PAGE, history.length));
        }
      }
    }
    body.addEventListener("scroll", onScroll);
    return () => body.removeEventListener("scroll", onScroll);
  }, [history.length, visibleRecent]);

  // 工具选择器的 Esc。它不走 ContextMenu 组件（菜单项要放 logo，MenuItem 只有纯文字），
  // 于是"点外面关闭"抄到了（backdrop），Esc 漏了 —— 别的菜单全都能 Esc 关掉，
  // 只有这一个不行。捕获阶段 + stopPropagation 的理由同 ContextMenu：不挡的话
  // Esc 会连带被有焦点的输入框那类 handler 一起吃掉。
  useEffect(() => {
    if (!toolPicker) return;
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      setToolPicker(null);
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [toolPicker]);

  // 折叠时整条侧栏隐藏（拖窗/红绿黄占位由全局 .app-titlebar 负责）
  if (collapsed) return null;

  // 生效中的筛选/排序，摊给控制行的 chip。每渲染重算一遍就够：最多 4 个字符串，
  // 比一个 useMemo 的依赖数组还便宜。顺序按「对看到的结果影响从大到小」：
  // 先是藏掉了什么，再是怎么排的。
  //
  // viewMode 刻意**不在**这里：chip 的 × 语义是「移除这个筛选」，而视图模式没有
  // 「移除」只有「切换」—— 拿 × 表达"切回按状态"既错义又容易误触（点一下就把
  // 视图打回默认并写进 localStorage）。它归旁边那个常驻的分段控件。
  const activeFilters: { label: string; clear: () => void }[] = [];
  if (showRunningOnly) activeFilters.push({ label: "仅运行中", clear: () => setShowRunningOnly(false) });
  if (showPinnedOnly) activeFilters.push({ label: "只看收藏", clear: () => setShowPinnedOnly(false) });
  if (showArchived) activeFilters.push({ label: "含已归档", clear: () => setShowArchived(false) });
  if (sortKey !== "recent") {
    activeFilters.push({
      label: sortKey === "count" ? "按消息数" : "按首条消息",
      clear: () => setSortKey("recent"),
    });
  }

  // 侧栏两个菜单的菜单项。每次渲染重算：菜单开着的时候后台在刷 sessions，
  // 「收藏 / 取消收藏」这种带状态的文案得跟着现在的状态走。
  function treeMenuItems(m: TreeMenu): MenuItem[] {
    if (m.kind === "project") {
      const allKeys = projectGroups.map((g) => projectCollapseKey(g.gitRoot, g.name));
      return [
        { label: "在 Finder 中显示", onClick: () => { invoke("open_path", { path: m.cwd, reveal: true }).catch(() => {}); } },
        { label: "复制路径", onClick: () => { navigator.clipboard.writeText(m.cwd).catch(() => {}); } },
        { sep: true },
        { label: "新建 session", disabled: !onNewSessionInDir, onClick: () => onNewSessionInDir?.(m.cwd) },
        { label: "新建 shell", disabled: !onNewShellInDir, onClick: () => onNewShellInDir?.(m.cwd) },
        { sep: true },
        // 只有一个组时这项没有意义（点了什么也不变），灰掉而不是让它假装能用。
        {
          label: "只看这个项目",
          disabled: allKeys.length <= 1,
          onClick: () => setCollapsedProjects((prev) => collapseOtherProjects(prev, allKeys, m.colKey)),
        },
      ];
    }
    const s = m.session;
    // last_cwd 优先：会话跑起来之后可能 cd 走了，用户右键要去的是它**现在**在的目录。
    const cwd = s.last_cwd || s.cwd;
    return [
      { label: pinnedSessions.has(s.session_id) ? "取消收藏" : "收藏", onClick: () => onTogglePin(s.session_id) },
      { label: s.archived ? "取消归档" : "归档", onClick: () => onArchive(s.session_id) },
      { sep: true },
      { label: "在左右分屏打开", disabled: !onOpenSessionInSplit, onClick: () => onOpenSessionInSplit?.(s, "v") },
      { label: "在上下分屏打开", disabled: !onOpenSessionInSplit, onClick: () => onOpenSessionInSplit?.(s, "h") },
      { sep: true },
      { label: "在 Finder 中显示", onClick: () => { invoke("open_path", { path: cwd, reveal: true }).catch(() => {}); } },
      { label: "复制路径", onClick: () => { navigator.clipboard.writeText(cwd).catch(() => {}); } },
      { label: "复制 ID", onClick: () => { navigator.clipboard.writeText(s.session_id).catch(() => {}); } },
      {
        label: "复制恢复命令",
        onClick: () => {
          navigator.clipboard.writeText(`cd ${cwd} && ${resumeCmd(s.session_id, s.tool)}`).catch(() => {});
        },
      },
    ];
  }

  // 选中项要滚进视野。setState 之后这一帧还没提交，rAF 里才拿得到那一行
  // （和上面 reveal 同一套理由）。block: "nearest" 而不是 "center"：按住 ↓ 时
  // 每一步都把行拖到正中会让整列不停跳动。
  function scrollRowIntoView(id: string) {
    requestAnimationFrame(() => {
      const el = treeBodyRef.current?.querySelector(`[data-session-id="${id}"]`) as HTMLElement | null;
      el?.scrollIntoView({ block: "nearest" });
    });
  }

  function selectAndScroll(id: string | null) {
    setSelectedId(id);
    if (id) scrollRowIntoView(id);
  }

  // 键盘导航。**绝不挂 window 捕获**：终端里 ↑↓ 是 shell 历史，抢了它等于废掉
  // makit 最核心的输入体验。所以这套只在侧栏内有焦点时生效（aside 的 tabIndex=-1
  // + React 冒泡），和项目里其他全局快捷键（⌘R/⌘L/⌘I 一律走捕获，因为 xterm.js
  // 会在自己的 textarea 上 stopPropagation）**刻意相反** —— 那些是全局动作，
  // 这个本质是局部的。
  function onTreeKeyDown(e: React.KeyboardEvent) {
    // 搜索框里除了 ↓ 一个键都不碰：Space 是空格、Enter 归输入框、←/→ 是移动光标。
    // ↓ 是唯一例外，它是「从搜索进入列表」的入口。
    const target = e.target as HTMLElement;
    if (target === searchRef.current) {
      if (e.key !== "ArrowDown" || e.metaKey || e.altKey) return;
      e.preventDefault();
      asideRef.current?.focus();
      selectAndScroll(visibleOrder[0] ?? null);
      return;
    }
    // 侧栏里的原生控件（筛选面板的 select / checkbox、视图切换和 chip 按钮）自己
    // 就要用方向键和空格 —— 把它们的键抢过来会让「排序下拉框按 ↓ 选不了下一项」
    // 「空格勾不上复选框」。会话行是 <li>，不在这个名单里。
    if (target.tagName === "INPUT" || target.tagName === "SELECT" || target.tagName === "TEXTAREA") return;
    if (target.closest("button")) return;
    // ⌘/⌥ 组合归全局快捷键（它们走捕获阶段，本来就先于这里跑），只放行 ⌘Enter。
    if (e.altKey) return;
    if (e.metaKey && e.key !== "Enter") return;

    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      selectAndScroll(moveSelection(visibleOrder, selectedId, e.key === "ArrowDown" ? "down" : "up"));
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setSelectedId(null);
      setHoverCard(null);
      onReturnFocus?.();
      return;
    }
    const s = selectedId ? sessionById.get(selectedId) : undefined;
    if (!s) return;
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.metaKey) onOpenSessionInSplit?.(s, "v");
      else if (e.shiftKey) onOpenSessionInSplit?.(s, "h");
      else onSessionClick(s);
      return;
    }
    if (e.key === " ") {
      e.preventDefault();
      const row = treeBodyRef.current?.querySelector(`[data-session-id="${s.session_id}"]`) as HTMLElement | null;
      if (!row) return;
      // 位置取选中行的矩形，不是鼠标坐标 —— 键盘操作时鼠标可能在屏幕另一头。
      const rect = row.getBoundingClientRect();
      setHoverCard((prev) => prev?.session.session_id === s.session_id
        ? null
        : { x: rect.right + 4, y: Math.min(rect.top, window.innerHeight - 300), session: s });
      return;
    }
    // ←/→ 折叠/展开选中项所在的组。两个视图里语义一致（← 收起、→ 展开），只是
    // 「组」的所指不同：项目视图是项目组，状态视图是状态组。不拿它去兜别的动作
    // （比如收起侧栏），那会让同一个键在两个视图里干两件事。
    // 选中项落在历史组时是明确的 no-op：那一档没有表头，也不可折叠。
    const wantCollapsed = e.key === "ArrowLeft";
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      if (viewMode === "project") {
        const g = projectGroups.find((pg) => pg.sessions.some((x) => x.session_id === s.session_id));
        if (!g) return;
        e.preventDefault();
        const colKey = projectCollapseKey(g.gitRoot, g.name);
        // 已经是目标状态就别白翻一次 state（→ 在已展开的组上是 no-op，不是"再展开一层"）
        if (isProjectCollapsed(collapsedProjects, colKey, g.hasActive) === wantCollapsed) return;
        setCollapsedProjects((prev) => toggleProjectCollapsed(prev, colKey, g.hasActive));
        return;
      }
      const g = [openedGroup, ...labeledGroups].find((lg) => lg.list.some((x) => x.session_id === s.session_id));
      if (!g) return;
      e.preventDefault();
      if (collapsedGroups.has(g.id) === wantCollapsed) return;
      toggleGroup(g.id);
      // ← 收起之后**故意不清 selectedId**，尽管选中行已经不渲染了：
      //   - moveSelection 已经处理「current 不在 order 里」→ 回到第一条，不会卡死；
      //   - 留着它，紧接着按 → 还能按同一条路找回这个组并展开。清掉的话 → 就成了
      //     no-op，键盘用户收起来之后再也打不开，只能去点鼠标。
      // 行不渲染时 .selected 那圈描边自然也不画，所以没有"幽灵选中框"的问题。
    }
  }

  /**
   * `showStatus` 由所在的组统一给（见 sessionStatus.ts 的 anyAlive）：整组都是
   * 已停止时不画状态列，把那 18px 还给标题。注意调用点必须包一层箭头函数 ——
   * 直接 `list.map(renderSession)` 会把数组下标当第二个参数传进来。
   */
  function renderSession(s: SessionMeta, showStatus: boolean) {
    const isPinned = pinnedSessions.has(s.session_id);
    const title = s.display_name || s.first_user_msg || `[${s.short_id}]`;
    const projectName = basename(s.git_root || s.cwd);
    const rs = runState(s);
    return (
      <li
        key={s.session_id}
        data-session-id={s.session_id}
        className={
          "tree-session " + runStateClass(rs) +
          (s.session_id === activeSessionId ? " focused" : "") +
          (s.session_id === selectedId ? " selected" : "")
        }
        // 点了就是选了：鼠标和键盘落在同一个选中态上，点完一行接着按 ↑↓ 是连续的。
        onClick={() => { setSelectedId(s.session_id); onSessionClick(s); }}
        onContextMenu={(e) => { e.preventDefault(); setContextMenu({ kind: "session", x: e.clientX, y: e.clientY, session: s }); }}
        onMouseEnter={(e) => {const mode = localStorage.getItem("makit-hover-mode") || "always";
          if (mode === "off") return;
          if (mode === "cmd" && !isCmd(e)) return;
          const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
          if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current);
          hoverTimerRef.current = setTimeout(() => setHoverCard({ x: rect.right + 4, y: Math.min(rect.top, window.innerHeight - 300), session: s }), mode === "cmd" ? 0 : 400);
        }}
        onMouseLeave={() => {
          if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current);
          hoverTimerRef.current = setTimeout(() => setHoverCard(null), 200);
        }}
        draggable
        onDragStart={(e) => onSessionDragStart(e, s)}
      >
        {showStatus && (
          <span className={"tree-status-icon " + runStateClass(rs)} title={runStateTitle(rs)}>
            {runStateIcon(rs)}
          </span>
        )}
        {isPinned && <span className="tree-pin">★</span>}
        <div className="tree-session-content">
          <div className="tree-session-row1">
            <span className="tree-session-title">{title}</span>
            {/* 只有「等待审批」值得挤占标题的横向空间 —— 它是需要人动手的那一档。
                原来 busy 显示「工作中」、running 显示「空闲」：前者和脉动的青色圆点
                说的是同一件事，后者更是每一条闲置 session 都挂一个灰色药丸，
                信息量为零却每行都在和标题抢宽度。两者都删掉，状态交给圆点。 */}
            {s.status === "waiting" && <span className="tree-status-badge waiting">等待审批</span>}
            <span className="tree-session-time" title={s.mtime_display}>
              {relativeTime(s.mtime * 1000)}
            </span>
          </div>
          <div className="tree-session-row2">
            {logoUrls[s.tool]
              ? <img src={logoUrls[s.tool]} className="tree-session-tool-logo" alt={s.tool} title={s.tool} />
              : s.tool === "codex" && <span className="tree-tool-badge codex">CX</span>
            }
            {/* busy 的文字说明降到 row2 —— 这一行本来就有 ~90px 空闲，不和标题争 */}
            {s.status === "busy" && <span className="tree-session-busy">工作中</span>}
            <span className="tree-session-sub">{projectName}</span>
            <span className="tree-session-sub">·</span>
            <span className="tree-session-sub">[{s.short_id}]</span>
            {s.git_branch && <>
              <span className="tree-session-sub">·</span>
              <span className="tree-session-branch">⑂{s.git_branch}</span>
            </>}
          </div>
        </div>
      </li>
    );
  }

  // 一组 = 可选的一行 10px 标签 + 一个 <ul>，组之间靠 margin 分隔。
  //
  // 为什么不再用 <li className="tree-flat-divider"> 那根 1px 线：分隔条件是
  // 「前一组非空 && 后一组非空」的两两组合，漏了「需要回应 ↔ 其余」这一对 ——
  // 没有收藏的时候（常态）waiting 的 session 和第 500 条历史之间零分隔，
  // 第一优先级和最低优先级共用一个视觉层级。改成每组自己一个 <ul>，边界由
  // 结构保证，漏不掉；间距代替线，也和整列「不画逐行发丝线」是同一套语言。
  // 带 id 的组可折叠（点标签整行）。历史组传的是空 label + 无 id：它没有表头可点，
  // 也不该有 —— 它是默认档，折叠它等于把整列清空。
  function renderGroup(g: { id?: string; label: string; list: SessionMeta[]; tone?: "warn" }) {
    if (g.list.length === 0) return null;
    const canCollapse = !!g.id && !!g.label;
    const isCollapsed = canCollapse && collapsedGroups.has(g.id!);
    const showStatus = anyAlive(g.list);
    return (
      <section key={g.id ?? g.label} className="tree-group">
        {g.label && (
          <div
            className={"tree-group-label" + (g.tone ? " " + g.tone : "") + (canCollapse ? " clickable" : "")}
            onClick={canCollapse ? () => toggleGroup(g.id!) : undefined}
            title={canCollapse ? (isCollapsed ? `展开「${g.label}」` : `折叠「${g.label}」`) : undefined}
          >
            {canCollapse && <span className={"tree-group-arrow" + (isCollapsed ? "" : " open")}>›</span>}
            <span>{g.label}</span>
            {/* 折叠起来之后计数是这一组唯一剩下的信息，所以它必须一直在 */}
            <span className="tree-group-count">{g.list.length}</span>
          </div>
        )}
        {!isCollapsed && (
          <ul className="tree-session-list">
            {g.list.map((s) => renderSession(s, showStatus))}
          </ul>
        )}
      </section>
    );
  }

  function renderProjectView() {
    return (
      <>
        {projectGroups.map((group) => {
          const colKey = projectCollapseKey(group.gitRoot, group.name);
          const isCollapsed = isProjectCollapsed(collapsedProjects, colKey, group.hasActive);
          return (
            <section key={colKey} className="tree-section tree-project-group">
              <div
                className={"tree-project-header" + (isCollapsed ? " collapsed" : "")}
                onClick={() => {
                  setCollapsedProjects((prev) => toggleProjectCollapsed(prev, colKey, group.hasActive));
                }}
                onContextMenu={(e) => {
                  // 右键不走上面那个 onClick（contextmenu 和 click 是两个事件），
                  // 所以这里只需要挡掉系统菜单 —— 组的折叠状态不该被右键顺手改掉。
                  e.preventDefault();
                  setContextMenu({
                    kind: "project",
                    x: e.clientX,
                    y: e.clientY,
                    colKey,
                    name: group.name,
                    cwd: group.gitRoot || group.sessions[0]?.cwd || "~",
                  });
                }}
              >
                <span className={"tree-project-arrow" + (isCollapsed ? "" : " open")}>›</span>
                {group.hasActive && <span className="tree-project-active-dot" title="有活跃 session" />}
                <span className="tree-project-name">{group.name}</span>
                <span className="tree-section-count">{group.sessions.length}</span>
                {(onNewSessionInDir || onNewShellInDir) && (
                  <button
                    className="tree-project-add"
                    title={`在 ${group.name} 新建 session（⌘点击新建 shell）`}
                    onClick={(e) => {
                      e.stopPropagation();
                      const cwd = group.gitRoot || group.sessions[0]?.cwd || "~";
                      if (isCmd(e)) { onNewShellInDir?.(cwd); return; }
                      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
                      setToolPicker({ cwd, tools: ["claude", "codex"], x: rect.right + 4, y: rect.top });
                    }}
                  >+</button>
                )}
              </div>
              {!isCollapsed && (
                <ul className="tree-session-list">
                  {/* hasActive 就是 anyAlive(group.sessions)，不用再算一遍 */}
                  {group.sessions.map((s) => renderSession(s, group.hasActive))}
                </ul>
              )}
            </section>
          );
        })}
        {/* 也要看「打开中」段：全部匹配项都开着的时候项目组是空的，
            但列表并不空 —— 这时报「无 session」是撒谎。 */}
        {projectGroups.length === 0 && groups.opened.length === 0 && (
          <div className="tree-empty">{loading ? "加载中…" : (query ? "无匹配 session" : "无 session")}</div>
        )}
      </>
    );
  }

  return (
    <>
    {/* tabIndex=-1 让 aside 能接焦点（但不进 Tab 序列），键盘导航的作用域就是它。
        onKeyDown 走冒泡而不是捕获：见 onTreeKeyDown 的注释。 */}
    <aside className="session-tree" style={{ width }} ref={asideRef} tabIndex={-1} onKeyDown={onTreeKeyDown}>
      <div className="tree-header">
        <div className="tree-search-wrap">
          <input
            ref={searchRef}
            className="tree-search"
            placeholder="搜索… (⌘⇧F)"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
          />
          {query && <button className="tree-search-clear" onClick={() => onQueryChange("")}>×</button>}
        </div>
      </div>

      {/* 控制行：左边「这个列表怎么组织」，右边「现在被怎么筛了」。
          视图切换从 header 里那个 select 换成常驻的分段控件 ——
          它是**视图模式**不是筛选：用户会来回切（找某个项目下的会话 → 按项目，
          看谁在跑 → 按状态），所以必须一眼看得见自己在哪个视图、一次点击切换。
          换成分段控件之后 header 里只剩搜索框，能吃满整条宽度（原来那个 select
          按最长选项 + 箭头占约 90px，把 280px 侧栏里的搜索框压到 ~170px）。 */}
      <div className="tree-list-controls">
        <div className="tree-view-toggle" role="group" aria-label="分组方式">
          {([["status", "状态"], ["project", "项目"]] as const).map(([mode, label]) => (
            <button
              key={mode}
              className={"tree-view-toggle-btn" + (viewMode === mode ? " active" : "")}
              onClick={() => setViewMode(mode)}
              title={mode === "status" ? "按状态：需要回应 / 收藏 / 其余" : "按项目分组"}
            >
              {label}
            </button>
          ))}
        </div>
        {activeFilters.length > 0 && (
          <div className="tree-active-filters">
            {activeFilters.map((f) => (
              <button key={f.label} className="tree-filter-chip" onClick={f.clear} title="点击移除">
                {f.label}<span className="tree-filter-chip-x">×</span>
              </button>
            ))}
          </div>
        )}
        {/* 漏斗从 footer 搬上来：筛选的**输入**（这个面板）和**输出**（左边的 chip）
            原来分居屏幕两端，改筛选要跑到底部、看结果要回到顶部。同一行之后面板
            向下弹、紧贴着它要影响的那个列表。 */}
        <button
          className={"tree-icon-btn tree-controls-filter" + (showPinnedOnly || showArchived || showRunningOnly ? " active" : "")}
          onClick={() => setFilterOpen((v) => !v)}
          title="筛选"
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M2 3h12l-4.5 6v4l-3 1V9L2 3z"/></svg>
        </button>
        {filterOpen && (
          <>
            <div className="tree-filter-backdrop" onClick={() => setFilterOpen(false)} />
            <div className="tree-filter-panel">
              {/* 这里不放「分组」：视图切换已经常驻在控制行左边了，
                  一件事两个入口比一个放错的入口更糟 */}
              <div className="tree-filter-row">
                <span className="tree-filter-label">排序</span>
                <select className="tree-filter-select" value={sortKey} onChange={(e) => setSortKey(e.target.value as SortKey)}>
                  <option value="recent">最近活动</option>
                  <option value="count">消息数</option>
                  <option value="firstMsg">首条消息</option>
                </select>
              </div>
              <label className="tree-filter-check">
                <input type="checkbox" checked={showRunningOnly} onChange={(e) => setShowRunningOnly(e.target.checked)} />
                仅运行中
              </label>
              <label className="tree-filter-check">
                <input type="checkbox" checked={showPinnedOnly} onChange={(e) => setShowPinnedOnly(e.target.checked)} />
                只看收藏
              </label>
              <label className="tree-filter-check">
                <input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} />
                显示已归档
              </label>
            </div>
          </>
        )}
      </div>

      <div className="tree-body" ref={treeBodyRef}>
        {/* 「已打开」两个视图都常驻置顶：它是导航面板，不属于任何一种分组方式。
            一个 tab 都没开时 renderGroup 的 length===0 分支让它自然消失。

            名字从「打开中」改成「已打开」：分组表达的是**位置**（在我的 tab 里），
            而「…中」这个后缀让它和下面的「运行中」读起来像同一类东西 —— 两个都像
            正在进行的活动，于是并列成互斥组之后就暗示了「打开中的没在运行」。
            这两个维度其实正交，见 sessionStatus.ts。 */}
        {renderGroup(openedGroup)}
        {viewMode === "status" ? (
          <>
            {/* 需要回应 → 后台运行 → 收藏 → 历史。前三组带标签，历史不带（它是默认档）。
                标签不用 warn 色：实测 7 条活着的 session 里有 4 条已经空转 2.5~5 天，
                整组染成警示色是虚报。busy/idle 的区别由每行的状态点承担，组标签只
                负责「我手上有几个在跑」这个环境信息。

                名字从「运行中」改成「后台运行」：这一组的真实成员条件是
                `running && !已打开 && !waiting` —— 也就是「进程活着，但没在你眼前」。
                叫「运行中」会让人以为上面「已打开」那组的都没在运行，而实际上最常见的
                情况恰恰是又开着又在跑。改完之后两个组名不再是同一类词，也不再互相
                否定：一个说位置，一个说「活着且不在这儿」。 */}
            {/* 不能套一层 <div>：组间距靠 `.tree-group + .tree-group` 这个兄弟选择器，
                中间插任何元素都会把相邻关系断掉。key 放在 renderGroup 返回的 section 上。 */}
            {labeledGroups.map(renderGroup)}
            {renderGroup({ label: "", list: historyVisible })}
            {historyVisible.length < history.length && (
              <div className="tree-load-more" onClick={() => setVisibleRecent((v) => Math.min(v + RECENT_PAGE, history.length))}>
                加载更多（{history.length - historyVisible.length} 个）
              </div>
            )}
            {filtered.length === 0 && (
              <div className="tree-empty">{loading ? "加载中…" : (query || showPinnedOnly || showRunningOnly ? "无匹配 session" : "无 session")}</div>
            )}
          </>
        ) : (
          renderProjectView()
        )}
      </div>

      <div className="tree-footer">
        <button
          className={"tree-icon-btn" + (refreshing ? " spinning" : "")}
          onClick={onRefresh}
          disabled={refreshing}
          title="刷新 (⌘R)"
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M8 3a5 5 0 014.9 4h-2l3 3 3-3h-2A7 7 0 008 1v2zm0 10a5 5 0 01-4.9-4h2l-3-3-3 3h2a7 7 0 0011.7 4l-1.4-1.4A5 5 0 018 13z"/></svg>
        </button>
        <button className="tree-icon-btn" onClick={onSettings} title="设置">
          {/* 实心 16 网格齿轮，和左边筛选/刷新同一套。原来这里用的是 24 网格 stroke-2 的
              描边齿轮缩到 12px 渲染：线宽掉到 1px 以下，8 个齿和内圈糊成一团噪点，
              而且和相邻两个实心图标明显不是一个图标家族。 */}
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M9.405 1.05c-.413-1.4-2.397-1.4-2.81 0l-.1.34a1.464 1.464 0 0 1-2.105.872l-.31-.17c-1.283-.698-2.686.705-1.987 1.987l.169.311c.446.82.023 1.841-.872 2.105l-.34.1c-1.4.413-1.4 2.397 0 2.81l.34.1a1.464 1.464 0 0 1 .872 2.105l-.17.31c-.698 1.283.705 2.686 1.987 1.987l.311-.169a1.464 1.464 0 0 1 2.105.872l.1.34c.413 1.4 2.397 1.4 2.81 0l.1-.34a1.464 1.464 0 0 1 2.105-.872l.31.17c1.283.698 2.686-.705 1.987-1.987l-.169-.311a1.464 1.464 0 0 1 .872-2.105l.34-.1c1.4-.413 1.4-2.397 0-2.81l-.34-.1a1.464 1.464 0 0 1-.872-2.105l.17-.31c.698-1.283-.705-2.686-1.987-1.987l-.311.169a1.464 1.464 0 0 1-2.105-.872l-.1-.34zM8 10.93a2.929 2.929 0 1 1 0-5.86 2.929 2.929 0 0 1 0 5.858z"/></svg>
        </button>
      </div>
    </aside>
    <div
      className="resizer"
      onMouseDown={(e) => onResizeStart(e.clientX)}
      onMouseEnter={() => onResizerHover?.(true)}
      onMouseLeave={() => onResizerHover?.(false)}
      title="拖拽调整宽度"
    />
    {toolPicker && (
      <>
        {/* 用 .context-menu 的外壳（和 ContextMenu 组件同一套样式），但内容是自己的：
            每项要放 tool 的 logo，ContextMenu 的 MenuItem 只有纯文字标签。
            它是从 + 按钮弹出的、位置由按钮算好，不需要鼠标坐标那套边界修正。 */}
        <div className="context-menu-backdrop" onClick={() => setToolPicker(null)} />
        <div className="context-menu" style={{ left: toolPicker.x, top: toolPicker.y }}>
          {toolPicker.tools.map((tool) => (
            <button key={tool} className="tool-picker-btn" onClick={() => { onNewSessionInDir?.(toolPicker.cwd, tool); setToolPicker(null); }}>
              {logoUrls[tool]
                ? <img src={logoUrls[tool]} className="tool-picker-logo" alt={tool} />
                : <span className="tool-picker-fallback">{tool === "codex" ? "⬡" : "◆"}</span>
              }
              <span>{tool === "claude" ? "Claude" : "Codex"}</span>
            </button>
          ))}
        </div>
      </>
    )}
    {contextMenu && (
      <ContextMenu
        x={contextMenu.x}
        y={contextMenu.y}
        items={treeMenuItems(contextMenu)}
        onClose={() => setContextMenu(null)}
      />
    )}
    {hoverCard && (() => {
      const s = hoverCard.session;
      const hTitle = s.display_name || s.first_user_msg || `[${s.short_id}]`;
      const hProcs = s.child_processes?.filter((p: any) => {
        const head = (p.command.split(/\s+/)[0] || "").split("/").pop() || "";
        return !["sh", "bash", "zsh", "ps", "claude", "caffeinate"].includes(head);
      }) || [];
      const cwdSame = !s.last_cwd || s.last_cwd === s.cwd;
      const rows: [string, string][] = [
        ["ID", s.session_id],
        ["启动 cwd", s.cwd],
        ...(!cwdSame ? [["当前 cwd", s.last_cwd] as [string, string]] : []),
        ...(s.git_root && s.git_root !== s.cwd ? [["项目", s.git_root] as [string, string]] : []),
        ...(s.git_branch ? [["分支", s.git_branch] as [string, string]] : []),
        ["消息", `${s.user_msg_count} 条`],
        ...(s.first_user_msg ? [["首话题", s.first_user_msg.slice(0, 80)] as [string, string]] : []),
        ...(s.last_user_msg && s.last_user_msg !== s.first_user_msg ? [["末话题", s.last_user_msg.slice(0, 80)] as [string, string]] : []),
        ...(s.running ? [["PID", String(s.pid)] as [string, string]] : []),
        ...(hProcs.length > 0 ? [["子进程", hProcs.map((p: any) => `${(p.command.split(/\s+/)[0] || "").split("/").pop()}(${p.pid})`).join(", ")] as [string, string]] : []),
        ["时间", s.mtime_display],
      ];
      return (
        <div
          className="tree-hover-card"
          style={{ left: hoverCard.x, top: hoverCard.y, display: "block" }}
          onMouseEnter={() => { if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current); }}
          onMouseLeave={() => setHoverCard(null)}
        >
          <div className="hover-card-title">{hTitle}</div>
          <table className="hover-card-table">
            <tbody>
              {rows.map(([label, val]) => (
                <tr key={label} className="hover-card-row" onClick={() => navigator.clipboard.writeText(val)} title="点击复制">
                  <td>{label}</td>
                  <td>{val}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    })()}
    </>
  );
});
