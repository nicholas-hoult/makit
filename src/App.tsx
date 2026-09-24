import { useCallback, useEffect, useMemo, useRef, useState, startTransition } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { homeDir } from "@tauri-apps/api/path";
import { confirm as confirmDialog, message as messageDialog } from "@tauri-apps/plugin-dialog";
import { TerminalView } from "./Terminal";
import { terminalManager } from "./TerminalManager";
import { zoomAction } from "./fontZoom";
import { WorkspaceView } from "./WorkspaceView";
import { useWorkspace } from "./useWorkspace";
import { findContainer, collectContainers, layoutTree, findNearestContainer, resumeCmd, resumeInitCommand, tabsToClose, openedOrder } from "./workspace-types";
import type { Direction } from "./workspace-types";
import { CommandPalette, PaletteItem } from "./CommandPalette";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { isCmd } from "./keys";
import { STATUS_LABEL, waitingLabel } from "./sessionStatus";
import { truncate, deriveSessionTabLabel } from "./sessionTitle";
import { PANE_SPEC_MIME } from "./paneDrop";
import { formatSearchCount, type SearchProgress } from "./searchCount";
import { SessionTree } from "./SessionTree";
import { useNotifications } from "./useNotifications";
import { applyRunningMeta, type RunningMeta } from "./running-merge";
import { installScrollActivity } from "./scrollActivity";
import { shortenHome } from "./homePath";
import {
  PANE_ICON_SETS, paneIconsFor, savedPaneIconSetId, savePaneIconSetId,
} from "./paneIcons";
import { NotificationPanel } from "./NotificationPanel";
import {
  BUILTIN_THEMES, applyTheme, savedThemeId, isLightTheme,
  listImportedThemes, importItermcolors, removeImportedTheme,
} from "./theme";
import type { ThemeSource } from "./theme";
import "./SessionTree.css";
import "./App.css";

// 性能诊断已移除 rAF 循环（它本身消耗性能）
// 卡顿定位改为手动：在可疑操作前后 console.time/timeEnd

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
  child_processes: ProcessInfo[];
  archived: boolean;
  storage_folder: string;
  pty_id: string;
  tool: string;
};

// display_name 从哪来（后端 name_source）。空字符串 = 后端没有比首条消息更好的名字，
// 此时 display_name 也是空的，徽标整个不渲染，所以这里不用给 "" 配文案。
const NAME_SOURCE_HINT: Record<string, string> = {
  rename: "用户 rename",
  "custom-title": "Claude 写入的标题",
  "agent-name": "Claude 写入的标题",
  first_msg: "首条消息（codex 会话）",
};

type ProcessInfo = {
  pid: number;
  ppid: number;
  command: string;
};

type ConversationMessage = {
  role: "user" | "assistant" | string;
  text: string;
  timestamp: string;
  cwd: string;
  git_branch: string;
  tool_uses: string[];
};

type SortKey = "recent" | "count" | "firstMsg";

const SORT_OPTIONS: { key: SortKey; label: string }[] = [
  { key: "recent", label: "最近活动" },
  { key: "count", label: "消息数" },
  { key: "firstMsg", label: "首条消息" },
];

type SubGroup = {
  cwd: string;       // 逻辑 key（"main" / "wt:name"）
  realCwd: string;   // 真实路径（用于新建 session/shell）
  label: string;
  isWorktree: boolean;
  sessions: SessionMeta[];
  latestMtime: number;
};

type ProjectGroup = {
  root: string;
  subGroups: SubGroup[];           // 启动于此（按 start_cwd 归属）
  visitedSessions: SessionMeta[];   // 曾活跃于此（last_cwd 落在此项目，但 start_cwd 不在）
  sessionCount: number;
  latestMtime: number;
};

type TabKind = "resume" | "new" | "shell";

// 单个终端实例（= 1 个 PTY）。tab 内多 pane 时每个 pane 一份。
type PaneState = {
  paneId: string;              // PTY 后端 id；唯一
  kind: TabKind;
  cwd: string;
  initCommand: string | null;
  sessionId: string | null;
  sessionShortId: string | null;
};




function basename(path: string): string {
  const parts = path.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || path;
}

type ProjectStatus = { kind: "waiting" | "busy" | "idle"; label: string } | null;

function deriveProjectStatus(p: ProjectGroup): ProjectStatus {
  let waiting = 0;
  let busy = 0;
  let idle = 0;
  for (const sub of p.subGroups) {
    for (const s of sub.sessions) {
      if (!s.running) continue;
      if (s.status === "waiting") waiting += 1;
      else if (s.status === "busy") busy += 1;
      else idle += 1;
    }
  }
  if (waiting > 0) return { kind: "waiting", label: "Needs input" };
  if (busy > 0) return { kind: "busy", label: "Running" };
  if (idle > 0) return { kind: "idle", label: "Idle" };
  return null;
}

function deriveProjectBranch(p: ProjectGroup): string {
  for (const sub of p.subGroups) {
    for (const s of sub.sessions) {
      if (s.git_branch) return s.git_branch;
    }
  }
  return "";
}

function summarizeProcess(p: ProcessInfo): string {
  const parts = p.command.split(/\s+/);
  const head = parts[0] || "";
  const exe = head.split("/").pop() || head;
  return `${exe}(${p.pid})`;
}

function shouldShowProcess(p: ProcessInfo): boolean {
  const cmd = p.command;
  if (!cmd) return false;
  const head = (cmd.split(/\s+/)[0] || "").split("/").pop() || "";
  // 隐藏 noise：shell / claude 自身 / ps / caffeinate（claude 防休眠用）
  const hide = ["sh", "bash", "zsh", "ps", "claude", "caffeinate"];
  if (hide.includes(head)) return false;
  return true;
}

function sortSessions(list: SessionMeta[], key: SortKey, pinned?: Set<string>): SessionMeta[] {
  const copy = [...list];
  copy.sort((a, b) => {
    // pinned 永远排最前
    if (pinned) {
      const ap = pinned.has(a.session_id) ? 1 : 0;
      const bp = pinned.has(b.session_id) ? 1 : 0;
      if (ap !== bp) return bp - ap;
    }switch (key) {
      case "recent": return b.mtime - a.mtime;
      case "count": return b.user_msg_count - a.user_msg_count;
      case "firstMsg": return (a.first_user_msg || "").localeCompare(b.first_user_msg || "");
      default: return 0;
    }
  });
  return copy;
}

function ensureProject(map: Map<string, ProjectGroup>, root: string): ProjectGroup {
  let p = map.get(root);
  if (!p) {
    p = { root, subGroups: [], visitedSessions: [], sessionCount: 0, latestMtime: 0 };
    map.set(root, p);
  }
  return p;
}

function groupByProject(list: SessionMeta[]): ProjectGroup[] {
  const projMap = new Map<string, ProjectGroup>();
  const seen = new Set<string>();
  const dedup = list.filter((s) => {
    if (seen.has(s.session_id)) return false;
    seen.add(s.session_id);
    return true;
  });
  for (const s of dedup) {
    const projRoot = s.git_root || s.cwd || "(unknown)";
    const wtName: string | null = null;

    const proj = ensureProject(projMap, projRoot);
    proj.sessionCount += 1;
    if (s.mtime > proj.latestMtime) proj.latestMtime = s.mtime;

    const subKey = wtName ? `wt:${wtName}` : "main";
    let sub = proj.subGroups.find((g) => g.cwd === subKey);
    if (!sub) {
      sub = {
        cwd: subKey,
        realCwd: wtName ? `${projRoot}/.worktrees/${wtName}` : projRoot,
        label: wtName || "主仓",
        isWorktree: !!wtName,
        sessions: [],
        latestMtime: 0,
      };
      proj.subGroups.push(sub);
    }
    sub.sessions.push(s);
    if (s.mtime > sub.latestMtime) sub.latestMtime = s.mtime;
  }
  const projects = Array.from(projMap.values());
  for (const p of projects) {
    p.subGroups.sort((a, b) => {
      if (a.isWorktree !== b.isWorktree) return a.isWorktree ? 1 : -1;
      return b.latestMtime - a.latestMtime;
    });
    p.visitedSessions.sort((a, b) => b.mtime - a.mtime);
  }
  projects.sort((a, b) => b.latestMtime - a.latestMtime);
  return projects;
}


type NewPaneSpec = Omit<PaneState, "paneId">;



// 命令字面量只在 workspace-types 里拼一处 —— 见那边 bindSessionToPaneTab 的注释：
// 曾经有 5 处各拼一遍，而第 6 条路（tab 认领 session）漏了，tab 就成了
// 「声称能恢复却不带命令」的坏形状
function sessionResumeCmd(s: SessionMeta): string {
  return resumeCmd(s.session_id, s.tool);
}

function sessionResumeInit(s: SessionMeta): string {
  return resumeInitCommand(s.session_id, s.tool);
}

// 拖动时把"新 pane 该长什么样"序列化进 dataTransfer
// session 卡片 / resume tab → kind=resume + initCommand=`clear && <resume-cmd>`
// new tab → kind=new + initCommand=`claude` 或留 null（按 cwd 起新会话）
// shell tab → kind=shell + initCommand=null（纯 shell）
function encodePaneSpec(spec: NewPaneSpec): string {
  return JSON.stringify(spec);
}

// 拖动时显示的浮层标签：把传进来的 label 渲染成离屏 div 给 setDragImage
// 默认 WKWebView 对小元素截图常常空白；显式 setDragImage 才稳
function setTabDragImage(e: React.DragEvent, label: string) {
  const ghost = document.createElement("div");
  ghost.className = "drag-ghost";
  ghost.textContent = label;
  // 必须在屏外但仍参与布局；display:none / visibility:hidden 会让 setDragImage 失效
  ghost.style.position = "fixed";
  ghost.style.top = "-9999px";
  ghost.style.left = "-9999px";
  document.body.appendChild(ghost);
  try {
    e.dataTransfer.setDragImage(ghost, 12, 12);
  } catch {}
  // 拖动开始后移除（下一帧 OS 已经截图完，可以清掉）
  requestAnimationFrame(() => ghost.remove());
}

/**
 * 落点提示：在目标 pane 中央浮出一张「图标 + 名字」的牌，0.7s 自己淡掉
 * （⌥⌘方向键切 pane、⌥⌘数字、通知跳转都用它）。
 *
 * 图标和名字排在**同一行**、图标在前：两者回答的是同一个问题的两个精度 ——
 * "落在第几个 pane"（形状，扫一眼）和"这个 pane 装的是什么"（名字，需要确认时读）。
 * 上一版竖排成 ⌘Tab 那种"图标在上、名字在下"，两个东西中间留了空，读起来是两块；
 * 一行内 icon → name 是从粗到细的一条阅读顺序，也就是一张牌。
 *
 * 用真 DOM 而不是伪元素：伪元素的 content 只能放文本，两个伪元素也没法排成一行
 * （谁都不知道另一个多宽）。真节点换来 flex 一行、图标字号独立于文字、以及图标能
 * 自己做进场动画。用完即 remove，不留任何 class 状态。
 *
 * paneIndex 由调用方给（就是 collectContainers 里的下标，也就是 ⌥⌘N 的 N-1）——
 * 不在这里按 DOM 顺序反推：那是另一条独立的顺序来源，一旦和 collectContainers 分叉，
 * 图标和 ⌥⌘N 就会静默错位。
 */
function flashContainer(containerId: string, paneIndex: number) {
  requestAnimationFrame(() => {
    const wrapper = document.querySelector(`[data-container-id="${containerId}"]`);
    const cv = wrapper?.querySelector(".container-view") as HTMLElement | null;
    if (!cv) return;
    // 连按方向键时把上一张牌直接扔掉重来（不是等它播完），提示要跟得上手速
    cv.querySelector(":scope > .pane-flash")?.remove();

    const card = document.createElement("div");
    card.className = "pane-flash";

    // 图标每次现读设置，不缓存：0.7s 的装饰，一次 localStorage 读可以忽略，
    // 而缓存就要多一条"设置改了要通知这里"的线。
    // 超过 9 个 pane 就没有对应的数字键了，图标跟着留空 —— 宁可不给，也不给一个
    // 和 ⌥⌘N 对不上的图标（"不显示"那套同理，整套是空数组）
    const icon = paneIconsFor(savedPaneIconSetId())[paneIndex];
    if (icon) {
      const iconEl = document.createElement("span");
      iconEl.className = "pane-flash-icon";
      iconEl.textContent = icon;
      card.appendChild(iconEl);
    }

    // 名字直接从 active tab 的标签上取：那就是这个 pane 的名字，不必把 workspace 数据
    // 一路传进来。textContent 是完整标题（tab 上被 CSS 截掉的那截也在里面）。
    const label = cv.querySelector(".container-tab.active .container-tab-label")?.textContent?.trim();
    const nameEl = document.createElement("span");
    nameEl.className = "pane-flash-name";
    nameEl.textContent = label || "pane";
    card.appendChild(nameEl);

    cv.appendChild(card);
    // 只认这张牌自己那条动画：图标的进场动画也会把 animationend 冒泡上来
    card.addEventListener("animationend", (e) => {
      if ((e as AnimationEvent).animationName === "pane-flash-in") card.remove();
    });
  });
}

// 事件是否落在"真正的文本框"里 —— 用来给那些和 Cocoa 文本编辑键冲突的快捷键让路。
// 典型的是 ⌘← ⌘→ ⌘↑ ⌘↓：在 macOS 文本框里它们是行首/行尾/文档首/文档尾，
// 抢过来当切 tab 用的话，侧栏搜索框、⌘F 终端搜索框、⌘K 命令面板输入框里
// 光标就再也跳不到行首了。
//
// 例外是 xterm 自己那个隐藏 textarea（`.xterm-helper-textarea`）：它是 DOM 上的
// textarea，但语义上是终端，不是文本框 —— 在终端里按 ⌘← 本来也没有原生行为，
// 所以那里必须让快捷键生效，否则焦点在终端时（最常见的情形）整条键位形同不存在。
function isNativeTextInput(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  if (el.classList?.contains("xterm-helper-textarea")) return false;
  return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable === true;
}




// 可拖拽宽度 hook：宽度持久化到 localStorage，min/max 钳制。
// 返回 { width, isDragging, startDrag(startX) }；startDrag 应在分隔条 onMouseDown 里调用。
function useResizable(key: string, defaultW: number, min: number, max: number) {
  const [w, setW] = useState<number>(() => {
    const stored = localStorage.getItem(key);
    const v = stored ? parseInt(stored, 10) : NaN;
    return Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : defaultW;
  });
  const [isDragging, setIsDragging] = useState(false);
  useEffect(() => {
    localStorage.setItem(key, String(w));
  }, [w, key]);
  function startDrag(startX: number) {
    const startW = w;
    setIsDragging(true);
    document.body.classList.add("resizing");
    function onMove(e: MouseEvent) {
      setW(Math.min(max, Math.max(min, startW + (e.clientX - startX))));
    }
    function onUp() {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.body.classList.remove("resizing");
      setIsDragging(false);
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.style.cursor = "ew-resize";
    document.body.style.userSelect = "none";
  }
  return { width: w, isDragging, startDrag };
}

/**
 * 覆盖层关闭后把键盘焦点交回终端。
 *
 * 会抢 DOM 焦点的覆盖层（通知面板打开时 focus 自己的 drawer、⌘F 的搜索框、设置面板
 * 里的输入控件）关掉之后，被 focus 的那个节点连同焦点一起消失，`activeElement` 退回
 * `<body>`：刚才那个 pane 的终端既不闪光标也收不到键盘 —— 看着就是"聚焦的那个窗口没了"。
 * ⌘K 命令面板一直在自己的 onClose 里手动补这一下，其余几处漏了。
 *
 * 用 effect 的 cleanup 而不是逐个 onClose：关闭路径有好几条（⌘I 再按一次 / Esc /
 * 点面板外 / 点某一行跳转），而"open 从 true 变 false"只有一处。
 * 依赖只写 open —— restore 由调用方保证是稳定引用，否则它变化时会在覆盖层还开着的
 * 时候把焦点抢回终端。
 */
function useRestoreFocusOnClose(open: boolean, restore: () => void) {
  useEffect(() => {
    if (!open) return;
    return restore;
  }, [open]);
}

function App() {
  const ws = useWorkspace();
  const searchInputRef = useRef<HTMLInputElement>(null);
  const revealRef = useRef<(tab: { sessionId: string | null; cwd: string }) => void>(() => {});
  const [filterPopoverOpen, setFilterPopoverOpen] = useState(false);
  const [sortPopoverOpen, setSortPopoverOpen] = useState(false);
  const [themePopoverOpen, setThemePopoverOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [, forceSettingsRender] = useState(0);
  const [terminalSearchOpen, setTerminalSearchOpen] = useState(false);
  const [terminalSearchTerm, setTerminalSearchTerm] = useState("");
  const [searchProgress, setSearchProgress] = useState<SearchProgress | null>(null);
  const terminalSearchInputRef = useRef<HTMLInputElement>(null);
  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  // 路径显示时把 home 缩写成 ~（#192：原来写死成开发者本机的 home）
  const [home, setHome] = useState("");
  useEffect(() => { homeDir().then(setHome).catch(() => {}); }, []);
  const [selectedRoot, setSelectedRoot] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [refreshDone, setRefreshDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  // 启动目录被删的 pane：非空即表示恢复对话框开着。
  // 记的是 **pane** 而不是 session —— 触发者是「某个面板起不来」，恢复完也要回到那个面板去起。
  // sessionId 可能为 null（tab 还没绑上 session），那种情况下只有「重建原目录」可用。
  const [recoverTarget, setRecoverTarget] = useState<{ paneId: string; cwd: string; sessionId: string | null } | null>(null);
  const [recoverPath, setRecoverPath] = useState("");
  // 挂一次就不重挂的 PTY 回调要判断「现在是不是已经有一个框开着」
  const recoverTargetRef = useRef(recoverTarget);
  recoverTargetRef.current = recoverTarget;
  const [recoverError, setRecoverError] = useState<string | null>(null);
  const [recoverBusy, setRecoverBusy] = useState(false);
  const [sortKey, setSortKey] = useState<SortKey>("recent");
  const [detailSession, setDetailSession] = useState<SessionMeta | null>(null);
  const [detailMessages, setDetailMessages] = useState<ConversationMessage[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [detailReversed, setDetailReversed] = useState(true);
  const [copiedSessionId, setCopiedSessionId] = useState<string | null>(null);
  const [notifPanelOpen, setNotifPanelOpen] = useState(false);
  const [notifPanelLeft, setNotifPanelLeft] = useState(104);
  const bellBtnRef = useRef<HTMLButtonElement>(null);
  const appRef = useRef<HTMLElement>(null);
  // 正在归档中的 session id（用 ref 立即生效，避免 running-changed 覆盖归档状态）
  const archivingRef = useRef(new Set<string>());
  const flashWindow = useCallback(() => {
    const el = appRef.current;
    if (!el) return;
    el.classList.remove("app-notify-flash");
    requestAnimationFrame(() => requestAnimationFrame(() => el.classList.add("app-notify-flash")));
  }, []);

  /**
   * 把键盘焦点交回「当前 pane 的 active tab」的终端。
   *
   * 读 ref 而不是 `ws.getActiveContainer()`：点通知里的某一行会先切 tab（`handleTabClick`）
   * 再关面板，闭包里的 workspace 还是切之前的，直接读会 focus 到旧 tab 上。
   * rAF 是等 React 把新的 active tab 渲染出来 —— 和 ⌘K 那边同一个理由。
   */
  const workspaceRef = useRef(ws.workspace);
  workspaceRef.current = ws.workspace;
  const wsApiRef = useRef(ws);
  wsApiRef.current = ws;
  // PTY 那些回调挂一次就不再重挂（deps 空），里面要读最新的 sessions
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  /**
   * 给「还没绑上 session 的 shell / new tab」补绑：pty_id → session_id。
   *
   * 为什么不能只靠下面 `[sessions]` 那个 effect：那条路的数据源是 `list_sessions`，扫的是
   * `~/.claude/projects/**\/*.jsonl`，而那个文件要等用户发出第一条消息才存在。所以
   * 「新建 shell、手打 claude」的 tab 在发消息之前根本不在 sessions 里，匹配不到，标题
   * 一直停在「新会话」；⌘R 走的也是 `list_sessions`，刷新同样无效；resume 出来的 tab
   * 早就有 jsonl，所以看着"是好的"。绑定需要的事实（pty_id + sessionId）其实在 claude
   * 启动那一刻就写进了 `~/.claude/sessions/<pid>.json` —— `resolve_pty_bindings` 直接读它。
   *
   * 没有待绑定 tab 就一次 IPC 都不发（后端也一次 ps 都不跑）；2s 节流是因为
   * `running-changed` 在会话活跃时每 500ms 一发，而一个"永远绑不上"的纯 shell tab
   * 会让每一发都白扫一遍 sessions 目录。
   */
  const lastBindAttemptRef = useRef(0);
  const tryBindPtyTabs = useCallback((force = false) => {
    const now = Date.now();
    if (!force && now - lastBindAttemptRef.current < 2000) return;
    const containers = collectContainers(workspaceRef.current.root);
    const pending: { containerId: string; tabId: string }[] = [];
    for (const c of containers) {
      for (const t of c.tabs) {
        if ((t.kind === "shell" || t.kind === "new") && !t.sessionId) {
          pending.push({ containerId: c.id, tabId: t.id });
        }
      }
    }
    if (pending.length === 0) return;
    lastBindAttemptRef.current = now;
    invoke<{ pty_id: string; session_id: string; short_id: string; name: string }[]>(
      "resolve_pty_bindings",
      { ptyIds: pending.map((p) => p.tabId) },
    )
      .then((bindings) => {
        for (const b of bindings) {
          const target = pending.find((p) => p.tabId === b.pty_id);
          if (target) {
            wsApiRef.current.bindSessionToTab(
              target.containerId,
              target.tabId,
              b.session_id,
              b.short_id,
              b.name,
              sessionsRef.current.find((s) => s.session_id === b.session_id)?.cwd,
            );
          }
        }
      })
      .catch(() => {});
  }, []);

  const focusActiveTerminal = useCallback(() => {
    requestAnimationFrame(() => {
      const { root, activeContainerId } = workspaceRef.current;
      const c = findContainer(root, activeContainerId);
      if (c?.activeTabId) terminalManager.focus(c.activeTabId);
    });
  }, []);
  // 滚动条的"滚动时显形"：一个全局捕获监听管所有容器（含用完就卸载的命令面板 /
  // 通知抽屉 / 右键菜单），不必逐个组件挂 —— 详见 scrollActivity.ts
  useEffect(() => installScrollActivity(), []);

  useRestoreFocusOnClose(paletteOpen, focusActiveTerminal);
  useRestoreFocusOnClose(notifPanelOpen, focusActiveTerminal);
  useRestoreFocusOnClose(terminalSearchOpen, focusActiveTerminal);

  /**
   * 搜索条上的「3/17」计数。
   *
   * 订阅必须在用户输入之前就挂好 —— 输入框的 onChange 会同步调 searchNext，
   * 而 xterm 的结果事件也是同步发的，晚一步就收不到第一次搜索的结果。所以这个
   * effect 依赖的是「搜索条开着」而不是「有搜索词」。
   *
   * 切 tab 时先清空：上一个终端的计数留在条上会指向一个已经不在搜的终端。
   */
  const searchTabId = ws.getActiveContainer()?.activeTabId ?? null;
  useEffect(() => {
    setSearchProgress(null);
    if (!terminalSearchOpen || !searchTabId) return;
    return terminalManager.onSearchResults(searchTabId, setSearchProgress);
  }, [terminalSearchOpen, searchTabId]);
  useRestoreFocusOnClose(settingsOpen, focusActiveTerminal);

  /**
   * 从别的 app / 通知横幅切回来时把焦点还给终端。
   *
   * 和上面几个覆盖层是同一类问题的另一个入口：窗口失焦期间焦点可能已经不在任何
   * 元素上（`activeElement` 退回 `<body>`），窗口再拿到焦点时浏览器不会替你补 ——
   * 于是切回来光标不闪、打字没反应，得先点一下终端。点系统通知横幅回来最容易踩到。
   *
   * 只在"焦点其实没落在任何输入控件上"时接手，否则会抢：切走前正在侧栏搜索框里打字，
   * 切回来当然应该还在那个框里。终端自己的 helper textarea 是例外 —— 它虽然是
   * textarea，但对它再 focus 一次是幂等的，顺带让 xterm 重新画出光标。
   */
  useEffect(() => {
    function onWindowFocus() {
      const el = document.activeElement;
      const isTerminalInput = el instanceof HTMLElement && el.classList.contains("xterm-helper-textarea");
      const inField =
        el instanceof HTMLElement &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
      if (inField && !isTerminalInput) return;
      focusActiveTerminal();
    }
    window.addEventListener("focus", onWindowFocus);
    return () => window.removeEventListener("focus", onWindowFocus);
  }, [focusActiveTerminal]);

  // 当前 active tab 对应的 session id（需在 useNotifications 之前计算）
  const activeSessionId = useMemo(() => {
    const c = ws.getActiveContainer();
    const tab = c?.tabs.find((t) => t.id === c.activeTabId);
    return tab?.sessionId ?? null;
  }, [ws.workspace]);

  /**
   * 「这个 session 是不是正摆在你眼前的那个 pane」—— 通知抑制的唯一判据。
   *
   * 抑制的正当理由只有一条：你已经看见了，横幅和铃铛都是多余的。所以判据必须
   * 是"看得见"，而原来那套（`activeSessionId === s.session_id && document.hasFocus()`）
   * 比"看得见"宽：
   *   ① 漏了 maximize —— 别的 container 被最大化时，activeContainer 自己是隐藏的
   *      （⌥⌘方向键能在最大化状态下把 activeContainerId 挪到一个看不见的 pane 上）；
   *   ② `document.hasFocus()` 只答"窗口有没有焦点"，答不了"哪个 pane"。
   * 这里三个条件一起判：焦点 pane 的 active tab 是它 + 它没被 maximize 藏起来 + 窗口有焦点。
   *
   * 依赖 ws.workspace 而不是 activeSessionId：maximizedContainerId 也是判据之一。
   */
  const isSessionOnScreen = useCallback((sessionId: string) => {
    if (!document.hasFocus()) return false;
    const { root, activeContainerId, maximizedContainerId } = ws.workspace;
    if (maximizedContainerId && maximizedContainerId !== activeContainerId) return false;
    const c = findContainer(root, activeContainerId);
    if (!c) return false;
    const tab = c.tabs.find((t) => t.id === c.activeTabId);
    return !!tab && tab.sessionId === sessionId;
  }, [ws.workspace]);

  const { notifications, unreadCount, markRead, markReadBySession, markAllRead, clearOne, clearAll,
    systemEnabled, setSystemEnabled,
    notifyApproval, setNotifyApproval, notifyUser, setNotifyUser,
    permission, requestSystemPermission, sendTestNotification } =
    useNotifications(sessions, isSessionOnScreen, flashWindow);

  // 切 tab 时自动标记该 session 的通知为已读：用户已经在看它了
  useEffect(() => {
    if (activeSessionId) markReadBySession(activeSessionId);
  }, [activeSessionId]);

  // 置顶收藏：localStorage 持久化
  const [pinnedSessions, setPinnedSessions] = useState<Set<string>>(() => {
    try {
      const raw = localStorage.getItem("makit-pinned-sessions");
      return raw ? new Set(JSON.parse(raw)) : new Set();
    } catch { return new Set(); }
  });
  useEffect(() => {
    try {
      localStorage.setItem("makit-pinned-sessions", JSON.stringify([...pinnedSessions]));
    } catch {}
  }, [pinnedSessions]);

  // 工作区里的三种右键菜单共用一个 state。存的是 id 而不是算好的菜单项数组 ——
  // 菜单开着的这段时间里后台事件会改 workspace / sessions，点下去时要按**那时**
  // 的状态办事，而不是右键那一刻捕获的闭包。
  type PaneMenu =
    | { kind: "container"; x: number; y: number; containerId: string }
    | { kind: "tab"; x: number; y: number; containerId: string; tabId: string }
    | { kind: "terminal"; x: number; y: number; tabId: string; selection: string };
  const [paneMenu, setPaneMenu] = useState<PaneMenu | null>(null);

  function togglePin(sessionId: string) {
    setPinnedSessions((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      return next;
    });
  }

  // 主题切换：源色 → 推导全部 UI 变量（见 theme.ts）；applyTheme 负责注入 + 持久化
  const [theme, setTheme] = useState<string>(savedThemeId);
  const [importedThemes, setImportedThemes] = useState<ThemeSource[]>(listImportedThemes);
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // pane 落点图标用哪一套。和主题并列而不是塞进主题里：主题在本项目的定义是
  // 一份 .itermcolors 等量的色彩数据，导入的色板里不可能有图标字段（见 paneIcons.ts）
  const [paneIconSet, setPaneIconSet] = useState<string>(savedPaneIconSetId);

  // 导入 .itermcolors：读文件 → 解析 → 存为可选主题 → 立即切换过去
  async function handleImportItermcolors(file: File) {
    try {
      const theme = importItermcolors(file.name, await file.text());
      setImportedThemes(listImportedThemes());
      setTheme(theme.id);
    } catch (e) {
      // 同样不能用 window.alert，理由见 handleToggleArchive 的注释
      void messageDialog(e instanceof Error ? e.message : String(e), { title: "导入失败", kind: "error" });
    }
  }

  // 布局状态：左栏（项目列）/ 中栏（sessions）独立可折叠，持久化到 localStorage
  const [projectListCollapsed, setProjectListCollapsed] = useState<boolean>(
    () => localStorage.getItem("makit-project-list-collapsed") === "1"
  );
  useEffect(() => {
    localStorage.setItem("makit-project-list-collapsed", projectListCollapsed ? "1" : "0");
  }, [projectListCollapsed]);

  // 左/中栏宽度可拖拽，min/max 防止拖崩布局；宽度持久化到 localStorage
  const projectList = useResizable("makit-project-list-width", 280, 180, 480);
  const [resizerHovered, setResizerHovered] = useState(false);

  async function load(silent = false) {
    if (!silent) setLoading(true);
    setError(null);
    const start = Date.now();
    try {
      const data = await invoke<SessionMeta[]>("list_sessions", { cwdMode: "smart" });
      startTransition(() => setSessions(data));
    } catch (e) {
      setError(String(e));
    } finally {
      if (!silent) {
        const elapsed = Date.now() - start;
        const minDisplay = 500;
        if (elapsed < minDisplay) {
          setTimeout(() => setLoading(false), minDisplay - elapsed);
        } else {
          setLoading(false);
        }
      }
    }
  }

  useEffect(() => {
    // 延迟 150ms：让 xterm/WebGL 先完成初始化，避免启动时卡一帧
    const id = setTimeout(() => load(true), 150);
    return () => clearTimeout(id);
  }, []);

  // 监听后端 watcher 事件，实时同步 session 状态
  useEffect(() => {
    let unlisten1: (() => void) | undefined;
    let unlisten2: (() => void) | undefined;

    // sessions/ 变化 → 轻量合并运行状态（不重扫 projects/）
    listen<void>("running-changed", () => {
      invoke<RunningMeta[]>("list_running_sessions").then((runningList) => {
        startTransition(() => {
          setSessions((prev) => {
            const map = new Map(runningList.map((r) => [r.session_id, r]));
            return prev.map((s) => {
              // ref 守卫：正在归档中或已归档，不被运行状态覆盖
              if (s.archived || archivingRef.current.has(s.session_id)) return s;
              return applyRunningMeta(s, map.get(s.session_id));
            });
          });
        });
      }).catch(() => {});
      // sessions/ 目录变了 —— 可能是刚有 claude 起来。这是新建 tab 补绑标题的主入口：
      // 它不依赖 jsonl，所以在用户发第一条消息之前就能绑上（见 tryBindPtyTabs 注释）
      tryBindPtyTabs();
    }).then((fn) => { unlisten1 = fn; });

    // projects/ 变化 → 只解析变化的那几个 jsonl 再合并进 sessions。
    //
    // 这里原来是个空函数，注释说"新 session 由 focus 时的 load() 捕获" —— 但代码里
    // 从来没有 focus → load 的接线，load() 只有启动和 ⌘R 两个调用点。于是新建会话
    // 进不了 sessions，链条整条断在这：sessions 里没有 → 下面 [sessions] 那个
    // bindSessionToTab 的 effect 匹配不到 pty_id → tab.sessionId 一直是 null →
    // stableGetTabTitle 只能回退到 tab.label（"新会话"）。手动 ⌘R 才恢复。
    //
    // 不能在这里直接 load()：projects/ 是递归监听，每追加一条消息就触发一次，
    // 而 list_sessions 要全量扫 220 个文件 / 771MB —— 那才是当初接成空函数的真实理由。
    let mergeTimer: number | null = null;
    const pendingPaths = new Set<string>();

    listen<string[]>("sessions-changed", (ev) => {
      const paths = ev.payload;
      if (!paths || paths.length === 0) return;
      for (const p of paths) pendingPaths.add(p);
      // 后端已经 debounce 500ms，这里再做一次 800ms 尾部合并：session 活跃时
      // 事件是每 500ms 一发，每发都要解析整个 jsonl + 每个 running session 一次
      // `ps -p`（取 pty_id）。标题同步这件事对 1.3s 的延迟完全不敏感。
      if (mergeTimer !== null) return;
      mergeTimer = window.setTimeout(() => {
        mergeTimer = null;
        const batch = [...pendingPaths];
        pendingPaths.clear();
        invoke<SessionMeta[]>("list_sessions_by_paths", { paths: batch, cwdMode: "smart" })
          .then((updated) => {
            if (updated.length === 0) return;
            startTransition(() => {
              setSessions((prev) => {
                const byId = new Map(prev.map((s) => [s.session_id, s]));
                for (const u of updated) {
                  // ref 守卫：归档正在写盘的窗口内不覆盖（和 running-changed 同一条）
                  if (archivingRef.current.has(u.session_id)) continue;
                  const old = byId.get(u.session_id);
                  byId.set(u.session_id, {
                    ...u,
                    // 增量命令没跑 `ps -eo` 全表（见 Rust 侧注释），子进程沿用旧值
                    child_processes: old ? old.child_processes : u.child_processes,
                  });
                }
                // 新 session 的 mtime 最新，重排保持列表"最近在前"的不变量
                return [...byId.values()].sort((a, b) => b.mtime - a.mtime);
              });
            });
          })
          .catch(() => {});
      }, 800);
    }).then((fn) => { unlisten2 = fn; });

    // 启动补绑：tab.id 是持久化的，也就是 MAKIT_PTY_ID —— 上一轮没绑成的 tab，只要它的
    // claude 还活着（逃逸进程／app 重启前就在跑），这一发就能把标题补上，不用等新事件。
    // force：不受 2s 节流约束，否则会被紧随其后的 running-changed 抢掉配额。
    tryBindPtyTabs(true);

    return () => {
      unlisten1?.();
      unlisten2?.();
      if (mergeTimer !== null) clearTimeout(mergeTimer);
    };
  }, [tryBindPtyTabs]);


  // macOS 系统全屏：toggle body 类，CSS 据此清掉装饰线（顶栏底部分隔线 / tab 顶边线）
  useEffect(() => {
    const win = getCurrentWindow();
    let unlistenResize: (() => void) | null = null;
    let unlistenScale: (() => void) | null = null;
    const sync = async () => {
      try {
        const fs = await win.isFullscreen();
        document.body.classList.toggle("app-fullscreen", fs);
      } catch {}
    };
    sync();
    win.onResized(sync).then((fn) => { unlistenResize = fn; });
    win.onScaleChanged(sync).then((fn) => { unlistenScale = fn; });
    return () => { unlistenResize?.(); unlistenScale?.(); };
  }, []);

  useEffect(() => {
    if (sessions.length === 0) return;
    // Shell tab 自动绑定 session：如果有 running session 的 pty_id 匹配某个 shell tab
    const runningSessions = sessions.filter((s) => s.running && s.pty_id);
    if (runningSessions.length > 0) {
      const containers = collectContainers(ws.workspace.root);
      for (const c of containers) {
        for (const t of c.tabs) {
          if ((t.kind === "shell" || t.kind === "new") && !t.sessionId) {
            const match = runningSessions.find((s) => s.pty_id === t.id);
            if (match) {
              ws.bindSessionToTab(c.id, t.id, match.session_id, match.short_id, undefined, match.cwd);
            }
          }
        }
      }
    }
  }, [sessions]);


  // 全局 ⌘ 按下状态：用于终端链接 underline 显示（仅按住 ⌘ 时识别为链接）。
  // 原来 Control 也算进来了 —— 于是终端里按 ⌃R 反查历史、⌃C 中断的一瞬间，
  // 满屏路径全部变成下划线闪一下。⌃ 在终端里是数据，跟「点链接」无关（见 keys.ts）。
  useEffect(() => {
    function onDown(e: KeyboardEvent) {
      if (e.key === "Meta") document.body.classList.add("cmd-pressed");
    }
    function onUp(e: KeyboardEvent) {
      if (e.key === "Meta") document.body.classList.remove("cmd-pressed");
    }
    function onBlur() { document.body.classList.remove("cmd-pressed"); }
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  // body.window-focused 决定 active pane 要不要亮 accent 顶线（见 App.css）
  useEffect(() => {
    const sync = (focused: boolean) =>
      document.body.classList.toggle("window-focused", focused);
    // 初值必须自己取。window-focus-changed 只在焦点"变化"时 emit，而应用启动时窗口
    // 本来就是聚焦的 —— 那一下没有事件，class 就一直缺着，active pane 的高亮在你
    // 第一次点出窗口再点回来之前完全不显示。
    sync(document.hasFocus());
    let unlisten: (() => void) | undefined;
    listen<boolean>("window-focus-changed", ({ payload: focused }) => {
      sync(focused);
    }).then((fn) => { unlisten = fn; });
    // DOM 事件兜底：Rust 侧的 WindowEvent::Focused 在窗口创建时可能早于 webview
    // 注册 listener，光靠 Tauri 事件会漏掉第一次
    const onFocus = () => sync(true);
    const onWinBlur = () => sync(false);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onWinBlur);
    return () => {
      unlisten?.();
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onWinBlur);
    };
  }, []);

  // 屏蔽 macOS WKWebView 默认右键菜单（Look Up / Translate / Search with Baidu...），
  // 顺带在终端里换上我们自己的那份。
  //
  // 原来这里是无条件 preventDefault，理由写的是「终端选中文本时这个菜单很碍事」——
  // 碍事的是 Look Up / Translate 那几项，不是"有个菜单"本身。所以现在改成：先照旧
  // 掐掉系统菜单，再看落点在不在某个终端里、而且**有没有选中内容**，都有才弹自己的。
  //
  // 「没选中就不弹」不是偷懒：终端里对"这个终端"能做的两件事（粘贴 / 清屏）都给不了
  // 确定结果，2026-08-23 在真窗口逐层埋点的结论是 ——
  //   粘贴：`navigator.clipboard.readText()` **时好时坏**。同一天两次点击，一次读到
  //     29 字符并成功写进 PTY，一次直接 NotAllowedError。WKWebView 对读剪贴板的
  //     授权判定不稳，一个会随机弹「读剪贴板失败」的菜单项不如没有。
  //     （writeText 一直好使，所以「复制」那条没事。）
  //   清屏：`\f`(^L) 送达是成功的（invoke ok、ptyReady true），但它只是**递给前台
  //     程序的一个请求**，程序完全可以不理 —— 送达 ≠ 会清屏。要做对得清 xterm 自己的
  //     buffer + scrollback（iTerm2 ⌘K 的语义），那是另一件事，不是修这个。
  // 两项都删了，没选中时一项不剩，弹个空框更糟。上面那句 preventDefault 留着 ——
  // 系统的 Look Up / Translate 照旧掐掉，这跟弹不弹我们自己的菜单是两回事。
  //
  // 挂 window 而不是给终端元素加 onContextMenu：xterm 的 DOM 由 addon 生成、
  // 层级不固定，我们唯一稳定的锚点是 Terminal.tsx 写在宿主 div 上的 data-pane-id。
  // 这个 handler 在冒泡阶段、window 上，所以 React 组件里的 onContextMenu
  // （tab、pane）都比它先跑；那两条路会 stopPropagation，到不了这里。
  useEffect(() => {
    function handler(e: MouseEvent) {
      e.preventDefault();
      const host = (e.target as HTMLElement | null)?.closest?.("[data-pane-id]");
      const paneId = host?.getAttribute("data-pane-id");
      if (!paneId) return;
      // 选中内容在**右键那一刻**读掉存下来：菜单一弹出、焦点离开终端，
      // 后续任何点击都可能清掉 xterm 的 selection，到点菜单项时就读不到了。
      // 这是这个 state 里唯一一个存快照而不是存 id 的字段，因为它描述的正是
      // "右键那一刻选中的是什么"。
      const selection = terminalManager.getSelection(paneId);
      if (!selection) return;
      setPaneMenu({ kind: "terminal", tabId: paneId, x: e.clientX, y: e.clientY, selection });
    }
    window.addEventListener("contextmenu", handler);
    return () => window.removeEventListener("contextmenu", handler);
  }, []);


  // Cmd+B / Cmd+\ / Cmd+Shift+B → 切换侧栏（SessionTree）
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!isCmd(e)) return;
      const k = e.key.toLowerCase();
      if (k === "b") {
        e.preventDefault();
        e.stopPropagation();
        setProjectListCollapsed((c) => !c);
        return;
      }
    }
    // 捕获阶段：焦点在终端里时 xterm 会 stopPropagation，冒泡阶段收不到（见 ⌘R 那处注释）
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+W 关闭：先关 active pane，最后一个 pane 才关 tab
  // Cmd+D = 垂直 split（左右），Cmd+Shift+D = 水平 split（上下），Cmd+W = 关闭
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!e.metaKey) return;
      const k = e.key.toLowerCase();
      if (k === "k") {
        e.preventDefault();
        e.stopPropagation();
        setPaletteOpen((v) => !v);
        return;
      }
      if (k === "f" && e.shiftKey) {
        // ⌘⇧F: 聚焦侧栏搜索框（session 列表）
        e.preventDefault();
        e.stopPropagation();
        setProjectListCollapsed(false);
        // 侧栏折叠时 SessionTree 整个 return null，输入框此刻还不在 DOM 里，
        // 得等这次 state flush 后的下一帧再 focus。select() 是为了让已有的
        // 关键词被下一次输入整体替换 —— ⌘⇧F 是「重新搜」，不是「接着上次输」
        requestAnimationFrame(() => {
          const el = searchInputRef.current;
          el?.focus();
          el?.select();
        });
        return;
      }
      if (k === "f") {
        // ⌘F: 当前 tab 终端内搜索
        e.preventDefault();
        e.stopPropagation();
        setTerminalSearchOpen(true);
        requestAnimationFrame(() => terminalSearchInputRef.current?.focus());
        return;
      }
      if (k === "w") {
        e.preventDefault();
        e.stopPropagation();
        const c = ws.getActiveContainer();
        if (c && c.activeTabId) {
          closeTabWithChildren(c.id, c.activeTabId);
        }
        return;
      }
      if (k === "d") {
        e.preventDefault();
        e.stopPropagation();
        ws.handleSplit(ws.workspace.activeContainerId, e.shiftKey ? "h" : "v");
        return;
      }
      if (k === "t") {
        e.preventDefault();
        e.stopPropagation();
        ws.handleNewShell(ws.workspace.activeContainerId);
        return;
      }
      if (k === "l") {
        e.preventDefault();
        e.stopPropagation();
        const c = ws.getActiveContainer();
        const tab = c?.tabs.find((t) => t.id === c.activeTabId);
        if (tab) revealRef.current({ sessionId: tab.sessionId, cwd: tab.cwd });
        return;
      }
      // ⌘1~⌘9 = 当前 pane 的第 N 个 tab；⌥⌘1~⌥⌘9 = 第 N 个 pane。
      // 修饰键就是"轴"的标记：⌘ 走 tab 轴（和 ⌘[ ⌘] 一套），⌥⌘ 走 pane 轴（和 ⌥⌘方向键、⌥⌘↩ 一套）。
      // 原来 ⌘1~9 是切 pane —— 但 Safari / Terminal.app / iTerm2 / VS Code 里 ⌘1-9 一律是
      // "第 N 个 tab"，而人大多数时候并不分屏，⌘2 十有八九是想跳第二个 tab。
      //
      // 用 e.code 而不是 e.key：macOS 上 ⌥1 产出的是 "¡" 不是 "1"，按 e.key 匹配会让整条
      // ⌥⌘ 分支永远不触发（和下面 BracketLeft 那处是同一个坑）。
      const digit = /^Digit([1-9])$/.exec(e.code);
      if (digit) {
        e.preventDefault();
        e.stopPropagation();
        const idx = parseInt(digit[1], 10) - 1;
        if (e.altKey) {
          const containers = collectContainers(ws.workspace.root);
          if (containers[idx]) {
            ws.setActive(containers[idx].id);
            flashContainer(containers[idx].id, idx);
          }
        } else {
          const c = ws.getActiveContainer();
          if (c?.tabs[idx]) ws.handleTabClick(c.id, c.tabs[idx].id);
        }
        return;
      }
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [ws.workspace]);

  // Cmd+Option+方向键（tmux 风格几何导航）：
  // 所有4个方向都基于 container 中心点的几何位置匹配，选最近邻
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!isCmd(e) || !e.altKey) return;
      const k = e.key.toLowerCase();
      // ⌘⌥↩ 最大化/还原当前 container
      if (k === "enter" || k === "return") {
        e.preventDefault();
        e.stopPropagation();
        if (ws.workspace.activeContainerId) ws.toggleMaximize(ws.workspace.activeContainerId);
        return;
      }
      if (!["arrowleft", "arrowright", "arrowup", "arrowdown"].includes(k)) return;
      e.preventDefault();

      const activeId = ws.workspace.activeContainerId;
      const containers = collectContainers(ws.workspace.root);
      // 单 container 无需导航
      if (containers.length <= 1) return;

      // 用 layoutTree 计算几何位置（使用 workspace DOM 元素的实际尺寸）
      const wsEl = document.querySelector(".workspace-view") as HTMLElement | null;
      const w = wsEl?.clientWidth ?? 1000;
      const h = wsEl?.clientHeight ?? 600;
      const layout = layoutTree(ws.workspace.root, { x: 0, y: 0, width: w, height: h });

      const dirMap: Record<string, Direction> = {
        arrowleft: "left",
        arrowright: "right",
        arrowup: "up",
        arrowdown: "down",
      };
      const target = findNearestContainer(layout, activeId, dirMap[k]);
      if (target) {
        ws.setActive(target);
        flashContainer(target, containers.findIndex((c) => c.id === target));
      }
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [ws.workspace]);

  // 切同 pane 内的上/下一个 tab。四组等价键位：
  //   ⌘← ⌘→ ⌘↑ ⌘↓  —— 主键位（iTerm2 同款）。和 pane 轴的 ⌥⌘方向键用同一组方向键，
  //                     只差一个 ⌥ —— 修饰键本身就是"轴"，方向键就是"往哪走"，
  //                     两条轴不用分别记。↑ = 上一个、↓ = 下一个，是 ← → 的别名
  //                     （tab 是一维的，但四个箭头都给上，手放哪都能按）
  //   ⌘[ ⌘]        —— 保留：Chrome / Safari 惯例
  //   ⌘⇧[ ⌘⇧]      —— Safari / Terminal.app 惯例（macOS 上 Shift+[ = {，所以也检测 { }）
  //   ⌃Tab ⌃⇧Tab   —— 浏览器 / VS Code / iTerm2 的通用肌肉记忆
  //
  // 括号只认 ⌘ 不认 ⌃：⌃[ 在终端里就是 ESC。这个 handler 挂在捕获阶段，
  // 原来 `e.metaKey || e.ctrlKey` 会先把 ⌃[ 吃掉并 preventDefault，
  // 于是 vim 里按 ⌃[ 退出插入模式变成了切 tab。
  // Tab 反过来只认 ⌃ 不认 ⌘：⌘Tab 是 macOS 应用切换器，抢不到也不该抢。
  // 方向键额外要给文本框让路（见 isNativeTextInput）：⌘←/→/↑/↓ 在 macOS 文本框里
  // 是行首/行尾/文档首尾，捕获阶段一律 preventDefault 会让搜索框的光标跳转失效。
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (e.altKey) return;
      const bracket = e.metaKey
        ? (e.code === "BracketLeft" || e.key === "[" || e.key === "{" ? "prev"
          : e.code === "BracketRight" || e.key === "]" || e.key === "}" ? "next" : null)
        : null;
      const arrow = e.metaKey && !e.shiftKey && !isNativeTextInput(e.target)
        ? (e.code === "ArrowLeft" || e.code === "ArrowUp" ? "prev"
          : e.code === "ArrowRight" || e.code === "ArrowDown" ? "next" : null)
        : null;
      const ctrlTab = e.ctrlKey && !e.metaKey && e.code === "Tab"
        ? (e.shiftKey ? "prev" : "next")
        : null;
      const isPrev = bracket === "prev" || arrow === "prev" || ctrlTab === "prev";
      const isNext = bracket === "next" || arrow === "next" || ctrlTab === "next";
      if (!isPrev && !isNext) return;
      e.preventDefault();
      // 方向键要额外 stopPropagation：xterm 会把 ⌘← 当普通方向键往 PTY 写 `ESC[D`，
      // 于是切 tab 的同时 shell 里的光标也跟着动了。括号 / ⌃Tab 不需要 —— xterm 不认。
      if (arrow) e.stopPropagation();
      const c = findContainer(ws.workspace.root, ws.workspace.activeContainerId);
      if (!c || c.tabs.length <= 1) return;
      const curIdx = c.tabs.findIndex((t) => t.id === c.activeTabId);
      const nextIdx = isPrev
        ? (curIdx - 1 + c.tabs.length) % c.tabs.length
        : (curIdx + 1) % c.tabs.length;
      ws.handleTabClick(c.id, c.tabs[nextIdx].id);
    }
    window.addEventListener("keydown", handler, true);return () => window.removeEventListener("keydown", handler, true);
  }, [ws.workspace]);

  // Cmd+I = 通知中心
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!isCmd(e) || e.shiftKey || e.altKey) return;
      if (e.key !== "i" && e.key !== "I") return;
      e.preventDefault();
      setNotifPanelOpen((v) => !v);
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+, = 设置（#187）：齿轮挪进侧栏「显示选项」菜单后多一步点击，补上 macOS 通用的设置快捷键
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!isCmd(e) || e.shiftKey || e.altKey || e.key !== ",") return;
      e.preventDefault();
      setSettingsOpen(true);
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+R = 刷新 session 列表（Tauri WKWebView 不原生支持 Cmd+R reload，手动拦截）
  //
  // 必须挂在**捕获阶段**并 stopPropagation：xterm.js 的 keydown 监听在自己的
  // .xterm-helper-textarea 上，处理完会调 cancel()（preventDefault + stopPropagation），
  // 所以焦点在终端里（也就是绝大多数时候）时，冒泡阶段的 window 监听根本收不到事件。
  // 实测：焦点在 .xterm-helper-textarea 上派发 ⌘R/⌘L，window 捕获收到、冒泡收不到。
  // 这和通知面板方向键、⌘I 是同一个坑，全项目的全局快捷键统一走捕获。
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (isCmd(e) && (e.key === "r" || e.key === "R")) {
        e.preventDefault();
        e.stopPropagation();
        load();
      }
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+L 聚焦到左树当前 session（展开侧栏 + 滚动到高亮 session）
  // 同上：捕获阶段 + stopPropagation
  const [revealTrigger, setRevealTrigger] = useState(0);
  const [clearFilterTrigger, setClearFilterTrigger] = useState(0);
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (isCmd(e) && (e.key === "l" || e.key === "L")) {
        e.preventDefault();
        e.stopPropagation();
        setProjectListCollapsed(false);
        setRevealTrigger((n) => n + 1);
      }
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+= / Cmd+Shift+Plus / Cmd+- / Cmd+0（含小键盘变体）：缩放当前 tab 的终端字号
  //
  // 依赖必须是 [ws.workspace] 不能是 []：handler 里读的 ws.getActiveContainer()
  // 闭包住的是 ws，依赖为空的话它永远停在首次渲染那一刻的布局树上——切了 tab
  // 之后 ⌘+ 会去缩上一个 pane。切 tab 那个 effect（本文件 findContainer 那段）
  // 就是这么写的，这里照抄同一个依赖数组。
  //
  // 挂捕获阶段：跟 ⌘R / ⌘L / ⌘I 同一个理由，焦点在终端里时 xterm 会在冒泡阶段
  // 之前 stopPropagation，冒泡阶段的 window 监听收不到。
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      const action = zoomAction(e);
      if (!action) return;
      const c = ws.getActiveContainer();
      if (!c?.activeTabId) return;
      e.preventDefault();
      e.stopPropagation(); // 否则 xterm 会把 ⌘= 当普通字符往 PTY 写
      terminalManager.zoom(c.activeTabId, action);
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [ws.workspace]);

  // 侧栏「打开中」段的数据源：在 tab 里开着的会话，按屏幕上的空间顺序。
  const openedSessionIds = useMemo(() => openedOrder(ws.workspace), [ws.workspace]);

  const archivedCount = useMemo(
    () => sessions.filter((s) => s.archived).length,
    [sessions]
  );

  // O(1) session 查找 map（替代 sessions.find 的 O(n) 遍历）
  const sessionsMap = useMemo(() => {
    const m = new Map<string, SessionMeta>();
    for (const s of sessions) m.set(s.session_id, s);
    return m;
  }, [sessions]);
  // ref 始终指向最新 map，供 stable callback 读取（不触发 WorkspaceView 重渲染）
  const sessionsMapRef = useRef(sessionsMap);
  sessionsMapRef.current = sessionsMap;

  /**
   * 关一个 tab 的唯一入口。
   *
   * `pty_kill` 只能杀到「还挂在这个 pty 进程组下」的进程；claude 起的工具进程一旦
   * setsid 就脱钩了，`killpg` 和 `pgrep -P` 都够不着，只能靠 session 侧记下来的
   * `child_processes`（`list_sessions` 里用 `MAKIT_SESSION_ID` 环境变量捞回来的那批）
   * 逐个 kill。
   *
   * 这一步原来只写在 × 按钮的 onTabClose 里，而关 tab 一共有 4 条路径 —— ⌘W、
   * 右键菜单「关闭」、× 按钮、归档时批量关。于是**用 ⌘W 关掉的 tab，逃逸的子进程
   * 一个都没被杀**（⌘W 恰好是最常用的那条）。和 #171 是同一个错误形状：一件
   * 「每次关 tab 都必须做」的事挂在了 N 条调用路径里的 1 条上，所以这里也收成一个口。
   */
  function closeTabWithChildren(containerId: string, tabId: string) {
    const c = findContainer(ws.workspace.root, containerId);
    const tab = c?.tabs.find((t) => t.id === tabId);
    const pids = tab?.sessionId
      ? sessionsMapRef.current.get(tab.sessionId)?.child_processes?.map((p) => p.pid)
      : undefined;
    if (pids?.length) invoke("kill_pids", { pids }).catch(() => {});
    ws.handleTabClose(containerId, tabId);
  }

  /// 批量关闭。倒序关是**必需的**，不是风格问题：closeTabWithChildren 读的是
  /// `ws.workspace.root`（这一帧的闭包，多次 setState 之后不会更新），而 tabId
  /// 是稳定的，所以每次都能查到；但如果正序关，前面的 tab 消失会让 activeTabId
  /// 在剩下的 tab 之间跳来跳去，中间态多一次没必要的终端 focus/fit。
  function closeTabsWithChildren(containerId: string, tabIds: string[]) {
    for (const id of [...tabIds].reverse()) closeTabWithChildren(containerId, id);
  }

  function copyToClipboard(text: string, okMsg: string) {
    navigator.clipboard.writeText(text).then(
      () => {
        setToast(okMsg);
        setTimeout(() => setToast(null), 1400);
      },
      () => {
        setToast("复制失败");
        setTimeout(() => setToast(null), 2000);
      },
    );
  }

  /// 工作区三种右键菜单的菜单项。
  ///
  /// 每次渲染重新算，而 paneMenu 里存的是 id —— 所以点下去时看到的是**当下**的
  /// workspace / sessions，不是右键那一刻的快照。菜单开着的几秒里后台事件照样在跑。
  function paneMenuItems(m: PaneMenu): MenuItem[] {
    if (m.kind === "terminal") {
      // 终端菜单只做「对这段选中文本做什么」。没选中时不弹（上面那个 handler 就
      // return 了），所以这里不用管 selection 为空的情况。
      return [
        { label: "复制", onClick: () => copyToClipboard(m.selection, "已复制") },
        {
          label: "搜索选中内容",
          onClick: () => {
            setTerminalSearchTerm(m.selection);
            setTerminalSearchOpen(true);
            // 延后一帧：计数订阅挂在「搜索条开着」的 effect 里（见上面那段注释），
            // 同步调 searchNext 的话 xterm 同步发出的结果事件没人接，计数是空的。
            requestAnimationFrame(() => terminalManager.searchNext(m.tabId, m.selection));
          },
        },
      ];
    }

    const c = findContainer(ws.workspace.root, m.containerId);
    if (!c) return [];

    if (m.kind === "container") {
      return [
        { label: "左右分屏", onClick: () => ws.handleSplit(m.containerId, "v") },
        { label: "上下分屏", onClick: () => ws.handleSplit(m.containerId, "h") },
        { label: "新终端", onClick: () => ws.handleNewShell(m.containerId) },
        { sep: true },
        // 明确写「当前」：这个菜单是在 pane 上右键弹的，没有"某个 tab"可指。
        // 想关别的 tab 请在那个 tab 上右键 —— 那条路才是 kind: "tab"。
        { label: "关闭当前 tab", onClick: () => closeTabWithChildren(m.containerId, c.activeTabId) },
      ];
    }

    const tab = c.tabs.find((t) => t.id === m.tabId);
    if (!tab) return [];
    const others = tabsToClose(c.tabs, m.tabId, "others");
    const toRight = tabsToClose(c.tabs, m.tabId, "right");
    const meta = tab.sessionId ? sessionsMap.get(tab.sessionId) : undefined;
    // 会话的真实 cwd 优先：用户可能在终端里 cd 走了，这时"在 Finder 打开"
    // 该开他现在待的地方，而不是当初起 pty 的目录。
    const cwd = meta?.last_cwd || meta?.cwd || tab.cwd;

    return [
      { label: "关闭", onClick: () => closeTabWithChildren(m.containerId, m.tabId) },
      {
        label: "关闭其他",
        disabled: others.length === 0,
        onClick: () => closeTabsWithChildren(m.containerId, others),
      },
      {
        label: "关闭右侧",
        disabled: toRight.length === 0,
        onClick: () => closeTabsWithChildren(m.containerId, toRight),
      },
      { sep: true },
      {
        // 只有一个 tab 时禁用：handleSplitWithTab 会先把只剩空壳的源 container
        // 摘掉，然后再去找那个已经不存在的 container 当分屏目标 —— tab 会凭空消失。
        // 而且一个 tab 拆出去还是"一个 pane 一个 tab"，这项本来也没有意义。
        label: "移到左右分屏",
        disabled: c.tabs.length <= 1,
        onClick: () => ws.handleSplitWithTab(m.containerId, m.tabId, m.containerId, "v", "after"),
      },
      {
        label: "移到上下分屏",
        disabled: c.tabs.length <= 1,
        onClick: () => ws.handleSplitWithTab(m.containerId, m.tabId, m.containerId, "h", "after"),
      },
      { sep: true },
      { label: "在 Finder 中显示", onClick: () => { invoke("open_path", { path: cwd, reveal: true }).catch(() => {}); } },
      { label: "复制路径", onClick: () => copyToClipboard(cwd, "已复制路径") },
      {
        label: "复制恢复命令",
        disabled: !tab.sessionId,
        onClick: () => {
          if (!tab.sessionId) return;
          copyToClipboard(`cd ${cwd} && ${resumeCmd(tab.sessionId, meta?.tool)}`, "已复制命令");
        },
      },
    ];
  }

  const filteredSessions = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return sessions;
    return sessions.filter((s) => {
      const hay = [
        s.cwd,
        s.last_cwd,
        s.git_branch,
        s.first_user_msg,
        s.last_user_msg,
        s.short_id,
        s.session_id,
        s.display_name,
      ]
        .join(" ")
        .toLowerCase();
      return hay.includes(q);
    });
  }, [sessions, query]);

  const projects = useMemo(() => groupByProject(filteredSessions), [filteredSessions]);

  // 全局标题栏文本：项目名 · 分支 · session 短 ID（active container 视角）
  const titlebarText = useMemo(() => {
    const c = ws.getActiveContainer();
    const tab = c?.tabs.find((t) => t.id === c.activeTabId);
    if (!tab) return "makit";
    if (tab.sessionId) {
      const meta = sessionsMap.get(tab.sessionId);
      if (meta) {
        const proj = basename(meta.git_root || meta.cwd);
        const parts = [proj];
        if (meta.git_branch) parts.push(meta.git_branch);
        return parts.join(" · ");
      }
    }
    return basename(tab.cwd);
  }, [ws.workspace, sessionsMap]);

  useEffect(() => {
    if (projects.length === 0) return;
    if (!selectedRoot || !projects.find((p) => p.root === selectedRoot)) {
      setSelectedRoot(projects[0].root);
    }
  }, [projects, selectedRoot]);

  const activeProject = useMemo(() => {
    if (!selectedRoot) return projects[0] ?? null;
    return projects.find((p) => p.root === selectedRoot) ?? projects[0] ?? null;
  }, [projects, selectedRoot]);


  // 状态聚合：每个项目有几个 running session
  const projectRunningCount = useMemo(() => {
    const m = new Map<string, number>();
    for (const s of sessions) {
      if (!s.running) continue;
      const root = s.git_root || s.cwd;
      m.set(root, (m.get(root) || 0) + 1);
    }
    return m;
  }, [sessions]);

  function makeNonce() {
    return Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  }

  // 给 session tab 生成显示名：display_name（rename / claude 写的标题）→ first_user_msg → short_id
  // short_id 那档实际到不了：后端 user_count==0 就 return None，所以 first_user_msg 必非空。留着当护栏。
  // 扫所有 tab 所有 pane，看某个 session_id 是否已经在某个 resume pane 里跑着
  // 用于：拖动 / 点击恢复时的去重——同一 session 同时跑两份 PTY 会争抢 jsonl 文件，必须防
  // 返回 null = 还没开；否则返回它所在的 tabId + paneId
  // 终端 → 侧栏定位：切到 session 所属 project + 展开两栏 + scrollIntoView
  // 如果 session 被过滤掉（搜索/归档/置顶），智能清除对应过滤
  function revealSidebarSession(tab: { sessionId: string | null; cwd: string } | null | undefined) {
    if (!tab) return;

    // 清除所有过滤（包括 SessionTree 内部的）
    setQuery("");
    setClearFilterTrigger((n) => n + 1);
    setProjectListCollapsed(false);

    if (!tab.sessionId) {
      const root = tab.cwd;
      if (root && root !== selectedRoot) setSelectedRoot(root);
      return;
    }
    const meta = sessionsMap.get(tab.sessionId ?? "");
    const cwd = meta?.cwd || tab.cwd;
    const root = meta?.git_root || cwd;
    if (root && root !== selectedRoot) setSelectedRoot(root);

    setRevealTrigger((n) => n + 1);
    setToast("已定位 session");
    setTimeout(() => setToast(null), 1500);

    // project 切换 + 展开后，session 卡片可能尚未渲染，多次重试
    let tries = 0;
    const sid = tab.sessionId;
    function tick() {
      const el = document.querySelector(`[data-session-id="${sid}"]`) as HTMLElement | null;
      if (el) {
        el.scrollIntoView({ behavior: "smooth", block: "center" });
        return;
      }
      if (tries++ < 10) requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
  }
  revealRef.current = (t) => revealSidebarSession(t);

  function findSessionLocation(sid: string): { containerId: string; tabId: string } | null {
    const containers = collectContainers(ws.workspace.root);
    for (const c of containers) {
      for (const t of c.tabs) {
        if (t.kind === "resume" && t.sessionId === sid) {
          return { containerId: c.id, tabId: t.id };
        }
      }
    }
    return null;
  }








  // 在项目/worktree 上开「新会话」：自动跑 claude
  function openClaudeNewTab(sub: SubGroup, projectRoot: string) {
    ws.openNewSession(sub.realCwd || projectRoot);
  }

  // 在项目/worktree 上开纯 shell（不跑 claude）
  function openShellTab(sub: SubGroup, projectRoot: string) {
    ws.openShell(sub.realCwd || projectRoot);
  }

  // 恢复一条已有 session：自动跑 claude -r <id>
  // 去重：除了同 id tab，还要查所有 tab 的 panes（拖拽后 session 可能落到任意 tab 的 split pane 里）
  // 冲突检测：如果外部已有 claude 进程在跑这个 session，提醒用户
  async function ensureSessionPath(s: SessionMeta) {
    if (!s.cwd || !s.storage_folder) return;
    try {
      await invoke("ensure_session_symlink", { sessionId: s.session_id, cwd: s.cwd, storageFolder: s.storage_folder });
    } catch {}
  }

  function spawnResumeTab(s: SessionMeta, resumeCwd: string) {
    ws.openSession(s.session_id, s.short_id, resumeCwd, deriveSessionTabLabel(s), s.tool);
  }

  async function openResumeTab(s: SessionMeta, _projectRoot: string) {
    // 先看是否已在 workspace 打开（在 app 内运行中也算）→ 切 container + tab
    const existing = findSessionLocation(s.session_id);
    if (existing) {
      ws.handleTabClick(existing.containerId, existing.tabId);
      return;
    }
    // running 的 session：检查是否在 app 内部的某个 shell tab 里跑着
    // 如果是 → 不报冲突，直接在当前 container 打开 resume tab（PTY 已在跑）
    if (s.running && s.pid) {
      setToast(`该 session 正在运行中（PID ${s.pid}），不能重复启动`);
      setTimeout(() => setToast(null), 3000);
      return;
    }
    // 启动目录在不在，这里**刻意不查** —— 由 pty_spawn 那道闸统一判（见
    // `terminalManager.onCwdMissing` 的注释）。在这里再预检一遍是第二套机制，
    // 而且它修不了最常见的那条路：重启后从持久化 workspace 恢复的 tab 根本不经过这个函数。
    await ensureSessionPath(s);
    spawnResumeTab(s, s.cwd);
  }

  // 目录被删之后的两条恢复路。共同点：**都只是让钥匙对上**。
  // 会话数据一个字节都没丢 —— claude 找会话的唯一方式是去
  // `~/.claude/projects/<encode(realpath(cwd))>/<id>.jsonl` 找，目录一删你就算不出那个键，
  // 而 transcript 还老老实实躺在原处。详见 lib.rs 的 `recover_session_cwd_in`。
  async function runRecover(mode: "recreate" | "relink") {
    const t = recoverTarget;
    if (!t) return;
    // session meta 到点击这一刻再查：目录没了的 pane 可能在 app 刚启动、sessions 还没扫完时
    // 就已经 spawn 失败弹了框。用户读完对话框再点，那时列表早加载好了。
    const s = sessions.find((x) => x.session_id === t.sessionId);
    if (mode === "relink" && !s) {
      setRecoverError("会话信息还没加载完，稍等一下再点");
      return;
    }
    setRecoverBusy(true);
    setRecoverError(null);
    try {
      const r = await invoke<{ cwd: string; detail: string }>("recover_session_cwd", {
        mode,
        sessionId: t.sessionId ?? "",
        originalCwd: t.cwd,
        targetCwd: mode === "relink" ? recoverPath.trim() : "",
        storageFolder: s?.storage_folder ?? "",
      });
      setRecoverTarget(null);
      setToast(r.detail);
      setTimeout(() => setToast(null), 4000);
      // 两件事都要做，缺一样这个 bug 就只修了一半：
      // ① 把持久化 tab 里那个死 cwd 换掉，否则下次启动照样按老路径 spawn
      // ② 就地把 PTY 起起来，用户不用再关了重开
      ws.updateTabCwd(t.paneId, r.cwd);
      const ok = await terminalManager.retrySpawn(t.paneId, r.cwd);
      if (!ok) {
        setToast(`${r.detail}（这个面板需要关掉重开）`);
        setTimeout(() => setToast(null), 5000);
      }
    } catch (e) {
      setRecoverError(String(e));
    } finally {
      setRecoverBusy(false);
    }
  }

  // 启动目录不存在、且这个 pane 不许降级（= resume tab）时，PTY 会拒绝启动并回调到这里。
  //
  // 挂在 TerminalManager 上而不是各个"打开会话"入口，因为**只有 `_spawnPty` 是真收口点**：
  // 侧栏点击 / 侧栏拖拽 / SessionTree 拖拽 / ⌘K+修饰键 / 重启后从持久化 workspace 恢复
  // —— 五条路最后都落到那儿。最后一条最要命：目录通常就是在 app 关着的时候被删/改名的，
  // 那是这个 bug 最常见的发生方式，而它绕过所有前端入口。
  useEffect(() => {
    // 对话框可以被关掉，所以那块空白面板必须自己能说明白怎么了 —— 否则就是一块没线索的死屏
    const openRecoverDialog = (paneId: string, cwd: string, sessionId: string | null) => {
      terminalManager.notice(
        paneId,
        `\r\n\x1b[33m原启动目录已不存在: ${cwd}\x1b[0m\r\n\x1b[2m会话记录没丢，丢的只是「在哪个目录启动」这把钥匙。若关掉了恢复窗口：关掉本面板，再从侧栏重新打开这条会话即可重试。\x1b[0m`,
      );
      // 一次只弹一个：改掉一个父目录会一口气带走 N 条会话，后面的面板不能抢掉
      // 用户正在输入路径的那个框。每个失败的面板屏幕上都留了上面那行提示，可逐个处理。
      if (recoverTargetRef.current) return;
      setRecoverPath(cwd);
      setRecoverError(null);
      setRecoverTarget({ paneId, cwd, sessionId });
    };
    terminalManager.onCwdMissing = (paneId, cwd) => {
      const sessionId =
        collectContainers(workspaceRef.current.root)
          .flatMap((c) => c.tabs)
          .find((t) => t.id === paneId)?.sessionId ?? null;
      // 会话最近待过的地方还在 → 悄悄用那儿，别拿这个去烦用户。
      // 「指到新位置」恢复过一次之后走的就是这条路：claude 在新目录里续写，last_cwd 成了新目录。
      const s = sessionId ? sessionsRef.current.find((x) => x.session_id === sessionId) : undefined;
      const last = s?.last_cwd;
      if (last && last !== cwd) {
        invoke<boolean>("dir_exists", { path: last })
          .then(async (alive) => {
            if (!alive) throw new Error("last_cwd 也没了");
            await invoke("recover_session_cwd", {
              mode: "relink",
              sessionId: s!.session_id,
              originalCwd: cwd,
              targetCwd: last,
              storageFolder: s!.storage_folder,
            });
            ws.updateTabCwd(paneId, last);
            await terminalManager.retrySpawn(paneId, last);
          })
          .catch(() => openRecoverDialog(paneId, cwd, sessionId));
        return;
      }
      openRecoverDialog(paneId, cwd, sessionId);
    };
    terminalManager.onCwdCorrected = (paneId, cwd) => ws.updateTabCwd(paneId, cwd);
    return () => {
      terminalManager.onCwdMissing = null;
      terminalManager.onCwdCorrected = null;
    };
  }, []);






  async function openDetail(s: SessionMeta) {
    setDetailSession(s);
    setDetailMessages([]);
    setDetailError(null);
    setDetailLoading(true);
    try {
      const msgs = await invoke<ConversationMessage[]>("read_session_messages", {
        sessionId: s.session_id,
      });
      setDetailMessages(msgs);
    } catch (e) {
      setDetailError(String(e));
    } finally {
      setDetailLoading(false);
    }
  }

  function closeDetail() {
    setDetailSession(null);
    setDetailMessages([]);
    setDetailError(null);
  }

  async function handleToggleArchive(s: SessionMeta) {
    try {
      if (s.archived) {
        await invoke("unarchive_session", { sessionId: s.session_id });
      } else {
        // 运行中的 session 归档需二次确认。
        // 必须用插件的 confirm 而不是 window.confirm —— 后者在这个 webview 里恒返回 false
        // （wry 的 WKUIDelegate 没实现 runJavaScriptConfirmPanelWithMessage:，WebKit 就
        // 不弹框直接给 false），归档会静默失败，见 src-tauri/Cargo.toml 的说明。
        if (s.running) {
          const ok = await confirmDialog(
            `该 session 正在运行中（PID ${s.pid}），归档将关闭终端并杀死子进程。`,
            { title: "确定归档？", kind: "warning", okLabel: "归档", cancelLabel: "取消" }
          );
          if (!ok) return;
        }
        // ref 立即标记：阻止 running-changed 在 await 窗口内覆盖归档状态
        archivingRef.current.add(s.session_id);
        try {
          await invoke("archive_session", { sessionId: s.session_id });
        } finally {
          archivingRef.current.delete(s.session_id);
        }
        // 收集所有要关闭的 tab（先收集再关闭，避免循环中修改 tree）
        // 子进程的清理不写在这里：closeTabWithChildren 每关一个 tab 都会做，
        // 归档不该是唯一记得做这件事的地方（原来这里手写了一遍，和 × 按钮里那份重复）
        const toClose: { containerId: string; tabId: string }[] = [];
        for (const c of collectContainers(ws.workspace.root)) {
          for (const t of c.tabs) {
            if (t.sessionId === s.session_id) {
              toClose.push({ containerId: c.id, tabId: t.id });
            }
          }
        }
        for (const { containerId, tabId } of toClose) {
          closeTabWithChildren(containerId, tabId);
        }
      }
      setSessions((prev) =>
        prev.map((x) => {
          if (x.session_id !== s.session_id) return x;
          if (!x.archived) return { ...x, archived: true, running: false, status: "idle" };
          return { ...x, archived: false };
        })
      );
    } catch (e) {
      archivingRef.current.delete(s.session_id);
      setToast(`归档失败: ${String(e)}`);
      setTimeout(() => setToast(null), 2000);
    }
  }

  async function handleCopy(s: SessionMeta) {
    const cmd = `cd ${s.cwd} && ${sessionResumeCmd(s)}`;
    try {
      await navigator.clipboard.writeText(cmd);
      setCopiedSessionId(s.session_id);
      setTimeout(() => {
        setCopiedSessionId((prev) => (prev === s.session_id ? null : prev));
      }, 1800);
    } catch {
      setToast("复制失败");
      setTimeout(() => setToast(null), 2000);
    }
  }

  function displayCwd(cwd: string, contextRoot?: string): string {
    // 优先显示相对项目 root 的短路径（./sub/dir 比 ~/IdeaProjects/foo/sub/dir 信息密度高）
    if (contextRoot && cwd.startsWith(contextRoot + "/")) {
      return "./" + cwd.slice(contextRoot.length + 1);
    }
    if (contextRoot && cwd === contextRoot) {
      return "./";
    }
    return shortenHome(cwd, home);
  }

  function renderSessionCard(s: SessionMeta, keyPrefix = "", projectRoot = "") {
    const tabOpen = findSessionLocation(s.session_id) !== null;
    // 当前 active container 的 active tab 对应的 session 高亮
    const activeContainer = ws.getActiveContainer();
    const activeTab = activeContainer?.tabs.find((t) => t.id === activeContainer.activeTabId);
    const focused = activeTab?.sessionId === s.session_id;
    const root = projectRoot || s.cwd;
    return (
      <li
        key={keyPrefix + s.session_id}
        data-session-id={s.session_id}
        className={
          "session session-clickable" +
          (s.archived ? " archived" : "") +
          (tabOpen ? " tab-open" : "") +
          (focused ? " session-focused" : "")
        }
        draggable
        onDragStart={(e) => {
          const spec: NewPaneSpec = {
            kind: "resume",
            cwd: s.cwd,
            initCommand: sessionResumeInit(s),
            sessionId: s.session_id,
            sessionShortId: s.short_id,
          };
          e.dataTransfer.setData(PANE_SPEC_MIME, encodePaneSpec(spec));
          e.dataTransfer.setData("text/plain", sessionResumeCmd(s));
          e.dataTransfer.effectAllowed = "copy";
          setTabDragImage(e, deriveSessionTabLabel(s));
        }}
        onClick={() => openResumeTab(s, root)}
        title={tabOpen ? "已打开，点击聚焦" : `点击恢复会话：${sessionResumeCmd(s)}`}
      >
        <div className="session-head">
          <span className="short-id">[{s.short_id}]</span>
          {s.display_name && (
            <span
              className={
                "session-name" +
                (s.name_source === "rename" ? " rename" : " auto")
              }
              title={NAME_SOURCE_HINT[s.name_source] || "自动生成的标题"}
            >
              {s.display_name}
            </span>
          )}
          {s.running && (
            <span
              className={"running status-" + (s.status || "idle")}
              title={s.waiting_for ? `等待: ${s.waiting_for}` : `状态: ${s.status || "运行中"}`}
            >
              {s.status === "busy"
                ? "工作中"
                : s.status === "waiting"
                ? "等待审批"
                : s.status === "idle"
                ? "空闲"
                : "运行中"}
            </span>
          )}
          <span className="time">{s.mtime_display}</span>
          <span className="human">({s.humanize})</span>
          <span className="count">{s.user_msg_count}条</span>
          {s.git_branch && <span className="branch">@{s.git_branch}</span>}
          {s.is_worktree && <span className="wt" title="worktree">wt</span>}
          {s.archived && <span className="archived-badge">已归档</span>}
        </div>
        {s.first_user_msg && (
          <div className="msg first">首: {truncate(s.first_user_msg)}</div>
        )}
        {s.last_user_msg && s.last_user_msg !== s.first_user_msg && (
          <div className="msg last">末: {truncate(s.last_user_msg)}</div>
        )}
        {s.last_cwd && s.last_cwd !== s.cwd && (
          <div className="msg recent-cwd" title={s.last_cwd}>
            最近活跃: {displayCwd(s.last_cwd, root)}
          </div>
        )}
        {s.running && s.child_processes.length > 0 && (
          <div
            className="msg child-procs"
            title={s.child_processes
              .map((p) => `${p.pid}\t${p.command}`)
              .join("\n")}
          >
            子进程({s.child_processes.length}):{" "}
            {s.child_processes
              .filter(shouldShowProcess)
              .slice(0, 6)
              .map(summarizeProcess)
              .join(", ") || "（仅 shell）"}
          </div>
        )}
        <div className="actions">
          <button
            className={"btn pin-btn" + (pinnedSessions.has(s.session_id) ? " pinned" : "")}
            onClick={(e) => {
              e.stopPropagation();
              togglePin(s.session_id);
            }}
            title={pinnedSessions.has(s.session_id) ? "取消置顶" : "置顶"}
          >
            {pinnedSessions.has(s.session_id) ? "★" : "☆"}
          </button>
          <button
            className={
              "btn " +
              (copiedSessionId === s.session_id ? "copied" : "primary")
            }
            onClick={(e) => {
              e.stopPropagation();
              handleCopy(s);
            }}
          >
            {copiedSessionId === s.session_id ? "✓" : "命令"}
          </button>
          <button
            className="btn"
            onClick={(e) => {
              e.stopPropagation();
              openDetail(s);
            }}
          >
            查看
          </button>
          <button
            className="btn"
            onClick={(e) => {
              e.stopPropagation();
              handleToggleArchive(s);
            }}
          >
            {s.archived ? "恢复" : "归档"}
          </button>
        </div>
      </li>
    );
  }

  // 稳定回调：deps=[] 永不重建，通过 ref 读最新 sessionsMap（sessions 变化不触发 WorkspaceView 重渲染）
  const stableGetTabTitle = useCallback((tab: { sessionId: string | null; label: string; kind: string; cwd: string }) => {
    if (tab.sessionId) {
      const meta = sessionsMapRef.current.get(tab.sessionId);
      if (meta) return deriveSessionTabLabel(meta);
    }
    if (tab.label) return tab.label;
    if (tab.kind === "shell") return basename(tab.cwd) + " · shell";
    return tab.kind;
  }, []);

  const stableGetTabStatus = useCallback((tab: { sessionId: string | null }) => {
    if (!tab.sessionId) return null;
    const meta = sessionsMapRef.current.get(tab.sessionId);
    if (!meta || !meta.running) return null;
    if (meta.status === "waiting") return "waiting" as const;
    if (meta.status === "busy") return "busy" as const;
    return "idle" as const;
  }, []);

  // Cmd+K items 缓存（只在 sessions/pinnedSessions 变化时重算）
  const paletteItems = useMemo(() => {
    const items: PaletteItem[] = [];
    const allActive = sessions.filter((s) => !s.archived);
    const waiting = allActive.filter((s) => s.running && s.status === "waiting");
    const running = allActive.filter((s) => s.running && s.status !== "waiting");
    const recent = sortSessions(
      allActive.filter((s) => !s.running),
      "recent",
      pinnedSessions
    ).slice(0, 30);

    function pushSession(s: SessionMeta, group: string) {
      const root = (s.cwd || "").split("/").pop() || "";
      const stat: "waiting" | "busy" | "idle" | "stopped" | "archived" = s.archived
        ? "archived"
        : s.status === "waiting" ? "waiting"
        : s.status === "busy" ? "busy"
        : s.running ? "idle"
        : "stopped";
      // 和侧栏同一套词（#187），只在 sessionStatus.ts 定义一处
      const statusLabel = stat === "waiting" ? waitingLabel(s.waiting_for) : STATUS_LABEL[stat];
      items.push({
        id: `s:${s.session_id}:${group}`,
        title: deriveSessionTabLabel(s),
        // short_id 去掉方括号：整串已经在用 `·` 分隔，再套一层 [] 是双重分隔，
        // 而且方括号在 11px 下只贡献噪点。
        subtitle: `${root} · ${s.short_id}${s.git_branch ? " · @" + s.git_branch : ""}`,
        hint: s.humanize,
        group,
        type: "Session",
        projectRoot: s.git_root || s.cwd,
        status: stat as any,
        statusLabel,
        waitingFor: s.waiting_for || undefined,
        running: s.running,
        archived: s.archived,
        pinned: pinnedSessions.has(s.session_id),
        mtime: s.mtime,
        msgCount: s.user_msg_count,
        action: (modifier) => {
          if (modifier === "split" || modifier === "newContainer") {
            const label = deriveSessionTabLabel(s);
            ws.handleSplitWithSession(
              ws.workspace.activeContainerId,
              modifier === "split" ? "v" : "h",
              "after",
              { kind: "resume", cwd: s.cwd, initCommand: sessionResumeInit(s), sessionId: s.session_id, sessionShortId: s.short_id }
            );
          } else {
            openResumeTab(s, s.cwd);
          }
        },
      });
    }
    const sortKey = (localStorage.getItem("makit-palette-sort") as SortKey) || "recent";
    const waitingUser = waiting.filter((s) => s.waiting_for === "user");
    const waitingApproval = waiting.filter((s) => s.waiting_for !== "user");
    // 分组名去掉前缀字形。原来是 "? 等待回答" / "⚠ 等待审批" / "▶ 工作中" / "⏱ 最近活跃" /
    // "🗄 已归档" —— ASCII 符号（? ⚠ ▶）和彩色 emoji（⏱ 🗄）混在一列里，字宽、基线、
    // 有没有颜色三样都不一致，五个组头的文字左缘是错开的。组头本来就有 chevron 和计数
    // 徽章当视觉锚点，行内每条又都有状态圆点，这个前缀是纯噪音。
    //
    // 注意：group 字符串同时是折叠状态的持久化 key（COLLAPSED_KEY），改名会让已保存的
    // 折叠状态失效一次 —— 只影响"哪几组是收起的"，可接受。
    // 组名和侧栏、状态筛选同一套词（#187）：原来的「工作中」把进行中和空闲混在一组，拆开
    const busy = running.filter((s) => s.status === "busy");
    const idle = running.filter((s) => s.status !== "busy");
    for (const s of sortSessions(waitingUser, sortKey, pinnedSessions)) pushSession(s, STATUS_LABEL.waiting_user);
    for (const s of sortSessions(waitingApproval, sortKey, pinnedSessions)) pushSession(s, STATUS_LABEL.waiting_approval);
    for (const s of sortSessions(busy, sortKey, pinnedSessions)) pushSession(s, STATUS_LABEL.busy);
    for (const s of sortSessions(idle, sortKey, pinnedSessions)) pushSession(s, STATUS_LABEL.idle);
    for (const s of sortSessions(allActive.filter((s) => !s.running), sortKey, pinnedSessions).slice(0, 30)) pushSession(s, STATUS_LABEL.stopped);
    const archived = sortSessions(sessions.filter((s) => s.archived), sortKey, pinnedSessions).slice(0, 30);
    for (const s of archived) pushSession(s, STATUS_LABEL.archived);

    return items;
  }, [sessions, pinnedSessions]);

  return (
    <main
      className="app"
      ref={appRef}
      onAnimationEnd={(e) => { if (e.animationName === "window-notify-flash") appRef.current?.classList.remove("app-notify-flash"); }}
    >
      {error && <div className="error">加载失败: {error}</div>}

      {/* macOS Overlay 全局标题栏：左 78px 给红绿黄占位
          - mousedown 显式调 startDragging：data-tauri-drag-region 在 Tauri 2 + WKWebView 偶尔拖不动，显式调最稳
          - dblclick 显式调 toggleMaximize：startDragging 会吃掉双击事件，需自己派发
          - 右键直接 block：Overlay 模式下系统 titlebar 菜单本来就拿不到，剩下的 WebView dev 菜单是噪音 */}
      <div
        className="app-titlebar"
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          const t = e.target as HTMLElement;
          if (t.closest("button, input, a, [role='button']")) return;getCurrentWindow().startDragging().catch((err) => console.warn("startDragging failed:", err));
        }}
        onDoubleClick={(e) => {
          const t = e.target as HTMLElement;
          if (t.closest("button, input, a, [role='button']")) return;
          getCurrentWindow().toggleMaximize().catch((err) => console.warn("toggleMaximize failed:", err));
        }}
        onContextMenu={(e) => e.preventDefault()}
      >
        {/* 左段：宽度 = sidebar 宽度（折叠时仅 78px 给红绿黄占位） */}
        <div
          className={"app-titlebar-sidebar" + (projectListCollapsed ? " collapsed" : "")}
          style={{ width: projectListCollapsed ? 78 : projectList.width }}
        />
        {/* 与 body 的 .resizer 同宽同位置的竖线分隔 */}
        {!projectListCollapsed && (
          <div className={"app-titlebar-resizer" + (resizerHovered || projectList.isDragging ? " hovered" : "")} />
        )}
        {/* 折叠 + 铃铛：绝对定位在 x=78（traffic lights 右边），sidebar 展开/折叠均可见 */}
        <div className="app-titlebar-controls">
          <button
            className="titlebar-icon-btn"
            onClick={() => setProjectListCollapsed((c) => !c)}
            title="折叠侧栏 (⌘B)"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor">
              <path d="M2 3a1 1 0 011-1h10a1 1 0 010 2H3a1 1 0 01-1-1zm0 5a1 1 0 011-1h6a1 1 0 010 2H3a1 1 0 01-1-1zm0 5a1 1 0 011-1h8a1 1 0 010 2H3a1 1 0 01-1-1z"/>
            </svg>
          </button>
          <button
            ref={bellBtnRef}
            className={"titlebar-icon-btn notif-bell" + (notifPanelOpen ? " active" : "") + (unreadCount > 0 ? " has-unread" : "")}
            onClick={() => {
              if (!notifPanelOpen && bellBtnRef.current) {
                setNotifPanelLeft(bellBtnRef.current.getBoundingClientRect().left);
              }
              setNotifPanelOpen((v) => !v);
            }}
            title={unreadCount > 0 ? `通知中心 (⌘I) · ${unreadCount} 条未读` : "通知中心 (⌘I)"}
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 16a2 2 0 0 0 2-2H6a2 2 0 0 0 2 2zm.995-14.901a1 1 0 1 0-1.99 0A5.002 5.002 0 0 0 3 6c0 1.098-.5 6-2 7h14c-1.5-1-2-5.902-2-7 0-2.42-1.72-4.44-4.005-4.901z"/>
            </svg>
            {/* 只用一个点，不显示条数：24×22 的按钮里放不下数字，7px 的字号根本读不出来，
                徽章反而盖掉铃铛一半。具体条数在展开的通知面板里看，hover 有 title 兜底。 */}
            {unreadCount > 0 && <span className="notif-bell-badge" />}
          </button>
        </div>
        <div className="app-titlebar-workspace">
          <div className="app-titlebar-title">{titlebarText}</div>
        </div>
      </div>

      <div className="body">
        <SessionTree
          sessions={sessions}
          pinnedSessions={pinnedSessions}
          activeSessionId={activeSessionId}
          loading={loading}
          query={query}
          onQueryChange={setQuery}
          onSessionClick={(s) => openResumeTab(s, s.cwd)}
          onSessionDragStart={(e, s) => {
            const spec: NewPaneSpec = {
              kind: "resume",
              cwd: s.cwd,
              initCommand: sessionResumeInit(s),
              sessionId: s.session_id,
              sessionShortId: s.short_id,
            };
            e.dataTransfer.setData(PANE_SPEC_MIME, encodePaneSpec(spec));
            e.dataTransfer.setData("text/plain", sessionResumeCmd(s));
            e.dataTransfer.effectAllowed = "copy";
            setTabDragImage(e, deriveSessionTabLabel(s));
          }}
          onTogglePin={togglePin}
          onArchive={(id) => {
            const s = sessionsMap.get(id);
            if (s) handleToggleArchive(s);
          }}
          onRefresh={load}
          revealTrigger={revealTrigger}
          clearFilterTrigger={clearFilterTrigger}
          searchRef={searchInputRef}
          onSettings={() => setSettingsOpen(true)}
          refreshing={loading}
          collapsed={projectListCollapsed}
          onCollapse={() => setProjectListCollapsed((c) => !c)}
          width={projectList.width}
          onResizeStart={(x) => projectList.startDrag(x)}
          onResizerHover={setResizerHovered}
          onNewSessionInDir={(cwd, tool) => ws.openNewSession(cwd, tool)}
          onNewShellInDir={(cwd) => ws.openShell(cwd)}
          /* 「打开中」段的数据源。侧栏原来只拿得到 activeSessionId（一条），
             答不了「我手上开着哪几个」—— 而那正是用户一天几十次要做的切换。 */
          openedSessionIds={openedSessionIds}
          onReturnFocus={focusActiveTerminal}
          onOpenDetail={openDetail}
          onOpenSessionInSplit={(s, dir) => {
            // 和命令面板里 ⌘/⇧ 回车走的是同一条路（上面 action(modifier)），
            // 免得"分屏打开一个会话"这件事出现第二种拼法。
            ws.handleSplitWithSession(ws.workspace.activeContainerId, dir, "after", {
              kind: "resume",
              cwd: s.cwd,
              initCommand: sessionResumeInit(s),
              sessionId: s.session_id,
              sessionShortId: s.short_id,
            });
          }}
        />

        {/* 右侧 workspace（终端区） */}
        <section className="workspace">
          <WorkspaceView
            workspace={ws.workspace}
            onSetActive={ws.setActive}
            onTabClick={ws.handleTabClick}
            onTabReveal={(cid, tid) => {
              const c = findContainer(ws.workspace.root, cid);
              const tab = c?.tabs.find((t) => t.id === tid);
              revealSidebarSession(tab);
            }}
            onTabClose={closeTabWithChildren}
            onSplit={ws.handleSplit}
            onNewShell={ws.handleNewShell}
            onMoveTab={ws.handleMoveTab}
            onTabReorder={ws.handleTabReorder}
            onSplitWithTab={ws.handleSplitWithTab}
            onSplitWithSession={ws.handleSplitWithSession}
            onDropSession={ws.handleDropSession}
            onUpdateRoot={ws.handleUpdateRoot}
            onToggleMaximize={ws.toggleMaximize}
            onContextMenu={(containerId, x, y) => {
              setPaneMenu({ kind: "container", containerId, x, y });
            }}
            onTabContextMenu={(containerId, tabId, x, y) => {
              setPaneMenu({ kind: "tab", containerId, tabId, x, y });
            }}
            getTabTitle={stableGetTabTitle}
            getTabStatus={stableGetTabStatus}
          />
        </section>
      </div>

      {paneMenu && (
        <ContextMenu
          x={paneMenu.x}
          y={paneMenu.y}
          items={paneMenuItems(paneMenu)}
          onClose={() => setPaneMenu(null)}
        />
      )}

      {toast && <div className="toast">{toast}</div>}

      {detailSession && (
        <div className="detail-overlay" onClick={closeDetail}>
          <div className="detail-panel" onClick={(e) => e.stopPropagation()}>
            <div className="detail-header">
              <div>
                <div className="detail-title">
                  [{detailSession.short_id}]
                  {detailSession.display_name && (
                    <span className="detail-name">{detailSession.display_name}</span>
                  )}
                </div>
                <div className="detail-sub">
                  {detailSession.mtime_display} ({detailSession.humanize}) ·{" "}
                  {detailSession.user_msg_count} 条用户消息 ·{" "}
                  {detailMessages.length} 条对话
                  {detailSession.git_branch && ` · @${detailSession.git_branch}`}
                  {detailSession.is_worktree && " · worktree"}
                </div>
                <div className="detail-cwd" title={detailSession.cwd}>
                  {detailSession.cwd}
                </div>
              </div>
              <div className="detail-actions">
                <button
                  className="btn"
                  onClick={() => setDetailReversed(!detailReversed)}
                  title={detailReversed ? "切换为正序（旧→新）" : "切换为逆序（新→旧）"}
                >
                  {detailReversed ? "↓ 新→旧" : "↑ 旧→新"}
                </button>
                <button className="btn" onClick={closeDetail}>
                  关闭
                </button>
              </div>
            </div>
            <div className="detail-body">
              {detailLoading && <div className="empty">加载中…</div>}
              {detailError && <div className="error">{detailError}</div>}
              {!detailLoading && !detailError && detailMessages.length === 0 && (
                <div className="empty">无对话内容</div>
              )}
              {(detailReversed ? [...detailMessages].reverse() : detailMessages).map((m, i) => (
                <div key={i} className={"msg-block " + m.role}>
                  <div className="msg-meta">
                    <span className="msg-role">{m.role === "user" ? "👤 用户" : "🤖 助手"}</span>
                    {m.timestamp && (
                      <span className="msg-ts">{m.timestamp.slice(0, 19).replace("T", " ")}</span>
                    )}
                    {m.tool_uses.length > 0 && (
                      <span className="msg-tools">tool: {m.tool_uses.join(", ")}</span>
                    )}
                  </div>
                  {m.text && <div className="msg-text">{m.text}</div>}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {(filterPopoverOpen || sortPopoverOpen || themePopoverOpen) && (
        <div className="popover-backdrop" onClick={() => {
          setFilterPopoverOpen(false);
          setSortPopoverOpen(false);
          setThemePopoverOpen(false);
        }} />
      )}

      {/* 关闭后归还焦点由上面的 useRestoreFocusOnClose(paletteOpen) 统一处理 */}
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        projects={projects.map((p) => p.root)}
        items={paletteItems}
        placeholder="搜索 session...  (⌘K)"
      />

      {terminalSearchOpen && (() => {
        const c = ws.getActiveContainer();
        const tabId = c?.activeTabId;
        const close = () => {
          if (tabId) terminalManager.searchClear(tabId);
          setTerminalSearchOpen(false);
          setTerminalSearchTerm("");
        };
        // 定位到 active container 右上角
        const el = c ? document.querySelector(`[data-container-id="${c.id}"]`) as HTMLElement | null : null;
        const rect = el?.getBoundingClientRect();
        const posStyle = rect
          ? { top: rect.top + 4, right: window.innerWidth - rect.right + 8 }
          : { top: 12, right: 24 };
        return (
          <div className="terminal-search-bar" style={posStyle}>
            <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M11.74 10.34a6 6 0 10-1.4 1.4l3.5 3.5a1 1 0 001.42-1.42l-3.52-3.48zM7 12a5 5 0 110-10 5 5 0 010 10z"/></svg>
            <input
              ref={terminalSearchInputRef}
              className="terminal-search-input"
              placeholder="在当前终端搜索..."
              value={terminalSearchTerm}
              onChange={(e) => {
                const v = e.target.value;
                setTerminalSearchTerm(v);
                if (tabId && v) terminalManager.searchNext(tabId, v);
                else if (tabId) terminalManager.searchClear(tabId);
              }}
              onKeyDown={(e) => {
                if (e.key === "Escape") { e.preventDefault(); close(); return; }
                if (e.key === "Enter") {
                  e.preventDefault();
                  if (!tabId) return;
                  if (e.shiftKey) terminalManager.searchPrevious(tabId, terminalSearchTerm);
                  else terminalManager.searchNext(tabId, terminalSearchTerm);
                  return;
                }
              }}
            />
            {/* 计数。空串时整个 span 不渲染而不是渲染个空的 —— 否则 flex gap 会
                在图标和按钮之间留一道莫名的缝。宽度靠 CSS 的 min-width 稳住，
                数字变化时 ↑↓ 按钮不该左右跳。 */}
            {(() => {
              const text = formatSearchCount(terminalSearchTerm, searchProgress);
              return text ? <span className="terminal-search-count">{text}</span> : null;
            })()}
            <button className="terminal-search-btn" onClick={() => tabId && terminalManager.searchPrevious(tabId, terminalSearchTerm)} title="上一个 (Shift+Enter)">↑</button>
            <button className="terminal-search-btn" onClick={() => tabId && terminalManager.searchNext(tabId, terminalSearchTerm)} title="下一个 (Enter)">↓</button>
            <button className="terminal-search-btn" onClick={close} title="关闭 (Esc)">×</button>
          </div>
        );
      })()}

      {notifPanelOpen && (
        <NotificationPanel
          notifications={notifications}
          unreadCount={unreadCount}
          onMarkRead={markRead}
          onMarkAllRead={markAllRead}
          onClearOne={clearOne}
          onClearAll={clearAll}
          onNavigate={(sessionId) => {
            const meta = sessionsMap.get(sessionId);
            if (!meta) return;
            const existing = findSessionLocation(meta.session_id);
            if (existing) {
              // session 已在窗口内 → 切 tab + 闪烁，不打开侧边栏
              ws.handleTabClick(existing.containerId, existing.tabId);
              flashContainer(
                existing.containerId,
                collectContainers(ws.workspace.root).findIndex((c) => c.id === existing.containerId),
              );
            } else {
              // session 不在窗口 → 完整导航 + 侧边栏定位
              revealSidebarSession({ sessionId, cwd: meta.cwd });
            }
          }}
          anchorLeft={notifPanelLeft}
          onClose={() => setNotifPanelOpen(false)}
        />
      )}

      {recoverTarget && (() => {
        const rs = recoverTarget.sessionId
          ? sessions.find((x) => x.session_id === recoverTarget.sessionId)
          : undefined;
        // codex 的会话按日期存在 `~/.codex/sessions/年/月/日/`，**不按 cwd 索引**
        // （storage_folder 恒为空串）。它根本没有钥匙可丢 —— 缺的只是一个能干活的目录。
        // 措辞必须跟着分岔，不然对 codex 用户是在讲一个不存在的问题。
        const keyedByCwd = rs ? !!rs.storage_folder : true;
        return (
        <div className="settings-backdrop" onClick={() => !recoverBusy && setRecoverTarget(null)}>
          <div className="settings-modal recover-modal" onClick={(e) => e.stopPropagation()}>
            <div className="settings-header">
              <span>启动目录已不存在</span>
              <button className="settings-close" onClick={() => setRecoverTarget(null)} disabled={recoverBusy}>×</button>
            </div>
            <div className="settings-body recover-body">
              {keyedByCwd ? (
                <p className="recover-note">
                  会话记录一个字节都没丢，丢的只是「在哪个目录启动」这把钥匙 ——
                  {" "}<code>claude -r</code>{" "}
                  只认原目录算出来的存储键。选一条路把钥匙对上：
                </p>
              ) : (
                <p className="recover-note">
                  这条会话不按目录索引，<code>{rs?.tool === "codex" ? "codex resume" : "resume"}</code>{" "}
                  在哪儿都能找到它 —— 缺的只是一个能干活的目录。给它一个即可：
                </p>
              )}
              <div className="recover-gone">{recoverTarget.cwd}</div>

              <div className="recover-option">
                <div className="recover-option-title">重建原目录</div>
                <div className="recover-option-desc">
                  {keyedByCwd
                    ? <>在原路径建一个空目录，钥匙自然对上，不动 <code>~/.claude</code>。
                       代码没了，但这条会话能接着聊 —— 会话里提到的文件路径都是空的。</>
                    : <>在原路径建一个空目录，会话在那儿接着跑。代码没了，会话里提到的文件路径都是空的。</>}
                </div>
                <button className="btn" onClick={() => runRecover("recreate")} disabled={recoverBusy}>
                  重建并打开
                </button>
              </div>

              <div className="recover-option">
                <div className="recover-option-title">指到新位置</div>
                <div className="recover-option-desc">
                  {keyedByCwd
                    ? <>代码搬家了就填新目录。会在新目录的存储键下建一个指向原会话记录的软链，
                       <strong>不拷贝</strong>（拷贝会变成同一个会话的两份分叉）。</>
                    : <>代码搬家了就填新目录。这个工具不按目录索引，所以只是换个工作目录，不动任何存储。</>}
                </div>
                <input
                  className="recover-input"
                  value={recoverPath}
                  spellCheck={false}
                  placeholder="/path/to/new/dir"
                  onChange={(e) => { setRecoverPath(e.target.value); setRecoverError(null); }}
                  onKeyDown={(e) => { if (e.key === "Enter" && !recoverBusy) runRecover("relink"); }}
                />
                <button className="btn" onClick={() => runRecover("relink")} disabled={recoverBusy || !recoverPath.trim()}>
                  指过去并打开
                </button>
              </div>

              {recoverError && <div className="recover-error">{recoverError}</div>}
            </div>
          </div>
        </div>
        );
      })()}

      {settingsOpen && (
        <div className="settings-backdrop" onClick={() => setSettingsOpen(false)}>
          <div className="settings-modal" onClick={(e) => e.stopPropagation()}>
            <div className="settings-header">
              <span>设置</span>
              <button className="settings-close" onClick={() => setSettingsOpen(false)}>×</button>
            </div>
            <div className="settings-body">
              <section className="settings-section">
                <div className="settings-label">外观主题</div>
                <select className="settings-select" value={theme} onChange={(e) => setTheme(e.target.value)}>
                  <optgroup label="深色">
                    {BUILTIN_THEMES.filter((t) => !isLightTheme(t)).map((t) => (
                      <option key={t.id} value={t.id}>{t.name}</option>
                    ))}
                  </optgroup>
                  <optgroup label="浅色">
                    {BUILTIN_THEMES.filter(isLightTheme).map((t) => (
                      <option key={t.id} value={t.id}>{t.name}</option>
                    ))}
                  </optgroup>
                  {importedThemes.length > 0 && (
                    <optgroup label="导入">
                      {importedThemes.map((t) => (
                        <option key={t.id} value={t.id}>{t.name}</option>
                      ))}
                    </optgroup>
                  )}
                </select>
                <div className="theme-preview">
                  <span className="theme-swatch" style={{ background: "var(--ansi-black)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-red)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-green)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-yellow)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-blue)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-magenta)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-cyan)" }} />
                  <span className="theme-swatch" style={{ background: "var(--ansi-white)" }} />
                </div>
                <div className="settings-actions">
                  <label className="settings-file-btn">
                    导入 .itermcolors
                    <input
                      type="file"
                      accept=".itermcolors,application/xml,text/xml"
                      onChange={(e) => {
                        const f = e.target.files?.[0];
                        e.target.value = "";
                        if (f) handleImportItermcolors(f);
                      }}
                    />
                  </label>
                  {theme.startsWith("imported:") && (
                    <button
                      className="settings-text-btn"
                      onClick={() => {
                        removeImportedTheme(theme);
                        setImportedThemes(listImportedThemes());
                        setTheme(BUILTIN_THEMES[0].id);
                      }}
                    >
                      删除当前
                    </button>
                  )}
                </div>
                <div className="settings-hint">
                  UI 配色由终端 16 色 + 前景/背景推导，因此任何 iTerm2 色板都能直接用。
                </div>
              </section>

              <section className="settings-section">
                <div className="settings-label">pane 落点图标</div>
                <select
                  className="settings-select"
                  value={paneIconSet}
                  onChange={(e) => {
                    setPaneIconSet(e.target.value);
                    savePaneIconSetId(e.target.value);
                  }}
                >
                  {PANE_ICON_SETS.map((s) => (
                    <option key={s.id} value={s.id}>{s.name}</option>
                  ))}
                </select>
                {/* 预览把序号标出来：图标按 pane 序号固定，而那个序号就是 ⌥⌘N 的 N。
                    不标数字的话这排图标看着只是"有哪些图案"，恰好漏掉了唯一要传达的事 */}
                <div className="pane-icon-preview">
                  {paneIconsFor(paneIconSet).length === 0 ? (
                    <span className="settings-hint">切 pane 时只浮出名字，不带图标。</span>
                  ) : (
                    paneIconsFor(paneIconSet).map((ic, i) => (
                      <span className="pane-icon-preview-item" key={i}>
                        <span className="pane-icon-preview-glyph">{ic}</span>
                        <span className="pane-icon-preview-num">{i + 1}</span>
                      </span>
                    ))
                  )}
                </div>
                <div className="settings-hint">
                  ⌥⌘方向键 / ⌥⌘数字 切 pane 时，目标 pane 中央浮出「图标 + 名字」。
                  图标按 pane 序号固定 —— 第 N 个 pane 永远是第 N 个图标，也就是 ⌥⌘N 里的 N。
                </div>
              </section>

              <section className="settings-section">
                <div className="settings-label">列表显示</div>
                <label className="settings-toggle-row">
                  <input type="checkbox" checked={localStorage.getItem("makit-show-attention") !== "false"} onChange={(e) => { localStorage.setItem("makit-show-attention", String(e.target.checked)); forceSettingsRender((n) => n + 1); }} />
                  <span>顶部「需要操作」区域</span>
                </label>
                <div className="settings-hint">将 waiting 状态的 session 置顶固定显示，方便快速发现需要审批的任务。</div>
              </section>

              <section className="settings-section">
                <div className="settings-label">系统通知</div>
                <label className="settings-toggle-row">
                  <input
                    type="checkbox"
                    checked={systemEnabled}
                    onChange={(e) => setSystemEnabled(e.target.checked)}
                  />
                  <span>启用 macOS 系统通知</span>
                </label>
                <label className="settings-toggle-row">
                  <input
                    type="checkbox"
                    checked={notifyApproval}
                    onChange={(e) => setNotifyApproval(e.target.checked)}
                  />
                  <span>等待审批时提醒（工具调用需确认）</span>
                </label>
                <label className="settings-toggle-row">
                  <input
                    type="checkbox"
                    checked={notifyUser}
                    onChange={(e) => setNotifyUser(e.target.checked)}
                  />
                  <span>等待回答时提醒（Claude 等待你输入）</span>
                </label>
                <div className="settings-hint">每次 session 进入 waiting 状态触发一次，再次 waiting 才重新提醒。</div>
                {/* 系统授权状态。上面那个开关只表达「我想收」，能不能收由系统说了算 ——
                    两者都为真才有横幅。不显示这一行的话，开关是开的却一条不来时无从判断。 */}
                <div className="settings-hint">
                  系统授权：
                  {permission === "granted" ? (
                    <b>已允许</b>
                  ) : permission === "denied" ? (
                    <>
                      <b>未允许</b> —— 横幅会被系统丢弃。到「系统设置 › 通知 › makit」里打开，或
                      <button
                        className="settings-inline-btn"
                        onClick={() => { void requestSystemPermission(); }}
                      >
                        请求授权
                      </button>
                    </>
                  ) : (
                    "读取中…"
                  )}
                </div>
                <button
                  className="settings-action-btn"
                  onClick={() => {
                    void sendTestNotification().then((ok) => {
                      if (!ok)
                        void messageDialog("系统未授权。到「系统设置 › 通知 › makit」里允许通知。", {
                          title: "发送失败",
                          kind: "warning",
                        });
                    });
                  }}
                >
                  发送测试通知
                </button>
                <div className="settings-hint">立刻发一条横幅验证链路，不必等真有 session 进入等待状态。</div>
                <button
                  className="settings-action-btn"
                  onClick={() => {
                    invoke<string>("install_claude_hook")
                      .then((r) =>
                        messageDialog(
                          r === "already_installed" ? "Hook 已安装，无需重复操作。" : "Hook 安装成功！Claude Code 重启后生效。",
                          { title: "安装 Hook" }
                        )
                      )
                      .catch((e) => messageDialog(String(e), { title: "安装失败", kind: "error" }));
                  }}
                >
                  安装 Claude Code Hook（推送模式）
                </button>
                <div className="settings-hint">将 makit-hook.sh 注册到 ~/.claude/settings.json，Claude Code 发出通知时实时推送（无需轮询）。</div>
              </section>


              <section className="settings-section">
                <div className="settings-label">Hover 详情卡片</div>
                <select className="settings-select" value={localStorage.getItem("makit-hover-mode") || "always"} onChange={(e) => { localStorage.setItem("makit-hover-mode", e.target.value); forceSettingsRender((n) => n + 1); }}>
                  <option value="always">始终显示（400ms 延迟）</option>
                  <option value="cmd">仅按住 ⌘ 时显示</option>
                  <option value="off">关闭</option>
                </select>
                <div className="settings-hint">Hover 时在 session 旁弹出详细信息卡片（ID、路径、分支、话题等），点击可复制。</div>
              </section>

              <section className="settings-section"><div className="settings-label">快捷键</div>
                {/* 和欢迎页那张表是同一份内容、同一种分组（⌘ = tab 轴，⌥⌘ = pane 轴）。
                    改键位时两处必须一起改 —— 这是目前唯一的两份来源。 */}
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">搜索 / 命令</div>
                  <div className="settings-shortcuts">
                    <div><span className="keys"><kbd>⌘K</kbd></span><span>全局搜索（命令面板）</span></div>
                    <div><span className="keys"><kbd>⌘F</kbd></span><span>当前终端内搜索</span></div>
                    <div><span className="keys"><kbd>⌘⇧F</kbd></span><span>聚焦侧栏 session 搜索</span></div>
                    <div><span className="keys"><kbd>⌘I</kbd></span><span>通知中心</span></div>
                    <div><span className="keys"><kbd>⌘R</kbd></span><span>刷新 session 列表</span></div>
                    <div><span className="keys"><kbd>⌘,</kbd></span><span>设置</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">Tab（⌘ 轴）</div>
                  <div className="settings-shortcuts">
                    <div><span className="keys"><kbd>⌘T</kbd></span><span>新建 shell tab</span></div>
                    <div><span className="keys"><kbd>⌘W</kbd></span><span>关闭当前 tab</span></div>
                    <div><span className="keys"><kbd>⌘</kbd><kbd>← → ↑ ↓</kbd></span><span>上 / 下一个 tab（iTerm2 惯例）</span></div>
                    <div><span className="keys"><kbd>⌘[</kbd><kbd>⌘]</kbd></span><span>同上（Chrome 惯例）</span></div>
                    <div><span className="keys"><kbd>⌘⇧[</kbd><kbd>⌘⇧]</kbd></span><span>同上（Safari 惯例）</span></div>
                    <div><span className="keys"><kbd>⌃⇧Tab</kbd><kbd>⌃Tab</kbd></span><span>同上（VS Code 惯例）</span></div>
                    <div><span className="keys"><kbd>⌘1</kbd><i>–</i><kbd>⌘9</kbd></span><span>切到当前 pane 的第 N 个 tab</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">Pane（⌥⌘ 轴）</div>
                  <div className="settings-shortcuts">
                    <div><span className="keys"><kbd>⌘D</kbd></span><span>左右分屏</span></div>
                    <div><span className="keys"><kbd>⌘⇧D</kbd></span><span>上下分屏</span></div>
                    <div><span className="keys"><kbd>⌥⌘←</kbd><kbd>⌥⌘→</kbd></span><span>切到左 / 右侧相邻 pane</span></div>
                    <div><span className="keys"><kbd>⌥⌘↑</kbd><kbd>⌥⌘↓</kbd></span><span>切到上 / 下方相邻 pane</span></div>
                    <div><span className="keys"><kbd>⌥⌘1</kbd><i>–</i><kbd>⌥⌘9</kbd></span><span>切到第 N 个 pane</span></div>
                    <div><span className="keys"><kbd>⌥⌘↩</kbd></span><span>最大化 / 还原当前 pane</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">侧栏 / 视图</div>
                  <div className="settings-shortcuts">
                    <div><span className="keys"><kbd>⌘B</kbd></span><span>折叠 / 展开侧栏</span></div>
                    <div><span className="keys"><kbd>⌘L</kbd></span><span>在侧栏定位当前 session</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">面板内</div>
                  <div className="settings-shortcuts">
                    <div><span className="keys"><kbd>↑</kbd><kbd>↓</kbd></span><span>命令面板 / 通知列表上下选择</span></div>
                    <div><span className="keys"><kbd>↩</kbd></span><span>打开 / 确认</span></div>
                    <div><span className="keys"><kbd>⌘↩</kbd></span><span>命令面板：在 split 中打开</span></div>
                    <div><span className="keys"><kbd>Esc</kbd></span><span>关闭面板 / 取消</span></div>
                  </div>
                </div>
              </section>

            </div>
          </div>
        </div>
      )}
      {(loading || refreshDone) && <div className="refresh-indicator">{loading ? "刷新中" : "✓ 已刷新"}</div>}
    </main>
  );
}

export default App;
