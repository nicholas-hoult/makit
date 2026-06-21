import { useCallback, useEffect, useMemo, useRef, useState, startTransition } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { TerminalView } from "./Terminal";
import { terminalManager } from "./TerminalManager";
import { WorkspaceView } from "./WorkspaceView";
import { useWorkspace } from "./useWorkspace";
import { findContainer, collectContainers, layoutTree, findNearestContainer } from "./workspace-types";
import type { Direction } from "./workspace-types";
import { CommandPalette, PaletteItem } from "./CommandPalette";
import { SessionTree } from "./SessionTree";
import { useNotifications } from "./useNotifications";
import { NotificationPanel } from "./NotificationPanel";
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

type RunningMeta = { session_id: string; status: string; waiting_for: string; pid: number };
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


function truncate(text: string, n = 100): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length <= n ? flat : flat.slice(0, n) + "…";
}


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


const DRAG_MIME = "application/x-ccs-pane-spec";

function sessionResumeCmd(s: SessionMeta): string {
  return s.tool === "codex" ? `codex resume ${s.session_id}` : `claude -r ${s.session_id}`;
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
  const terminalSearchInputRef = useRef<HTMLInputElement>(null);
  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  const [selectedRoot, setSelectedRoot] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [refreshDone, setRefreshDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
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

  // 当前 active tab 对应的 session id（需在 useNotifications 之前计算）
  const activeSessionId = useMemo(() => {
    const c = ws.getActiveContainer();
    const tab = c?.tabs.find((t) => t.id === c.activeTabId);
    return tab?.sessionId ?? null;
  }, [ws.workspace]);

  const { notifications, unreadCount, markRead, markReadBySession, markAllRead, clearOne, clearAll,
    systemEnabled, setSystemEnabled,
    notifyApproval, setNotifyApproval, notifyUser, setNotifyUser } =
    useNotifications(sessions, activeSessionId, flashWindow);

  // 切 tab 时自动标记该 session 的通知为已读（同 对标产品: dismissFocusedPanelNotificationIfActive）
  useEffect(() => {
    if (activeSessionId) markReadBySession(activeSessionId);
  }, [activeSessionId]);

  // 置顶收藏：localStorage 持久化
  const [pinnedSessions, setPinnedSessions] = useState<Set<string>>(() => {
    try {
      const raw = localStorage.getItem("ccs-pinned-sessions");
      return raw ? new Set(JSON.parse(raw)) : new Set();
    } catch { return new Set(); }
  });
  useEffect(() => {
    try {
      localStorage.setItem("ccs-pinned-sessions", JSON.stringify([...pinnedSessions]));
    } catch {}
  }, [pinnedSessions]);

  const [containerContextMenu, setContainerContextMenu] = useState<{ containerId: string; x: number; y: number } | null>(null);

  function togglePin(sessionId: string) {
    setPinnedSessions((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      return next;
    });
  }

  // 主题切换：默认 vscode-dark，localStorage 持久化
  const [theme, setTheme] = useState<string>(
    () => localStorage.getItem("ccs-theme") || "vscode-dark"
  );
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("ccs-theme", theme);
  }, [theme]);

  // 布局状态：左栏（项目列）/ 中栏（sessions）独立可折叠，持久化到 localStorage
  const [projectListCollapsed, setProjectListCollapsed] = useState<boolean>(
    () => localStorage.getItem("ccs-project-list-collapsed") === "1"
  );
  useEffect(() => {
    localStorage.setItem("ccs-project-list-collapsed", projectListCollapsed ? "1" : "0");
  }, [projectListCollapsed]);

  // 左/中栏宽度可拖拽，min/max 防止拖崩布局；宽度持久化到 localStorage
  const projectList = useResizable("ccs-project-list-width", 280, 180, 480);
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
              const r = map.get(s.session_id);
              if (r) return { ...s, running: true, status: r.status, waiting_for: r.waiting_for, pid: r.pid };
              if (s.running) return { ...s, running: false, status: "idle", waiting_for: "", pid: 0 };
              return s;
            });
          });
        });
      }).catch(() => {});
    }).then((fn) => { unlisten1 = fn; });

    // projects/ 变化只订阅不处理：新 session 由 focus 时的 load() 捕获，避免每条消息触发全量扫描冻结 UI
    listen<void>("sessions-changed", () => {}).then((fn) => { unlisten2 = fn; });

    return () => { unlisten1?.(); unlisten2?.(); };
  }, []);


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

  // sessions 加载后：对所有已打开的 resume tab 做 symlink 预检查（目录改名兼容）
  useEffect(() => {
    if (sessions.length === 0) return;
    try {
      const containers = collectContainers(ws.workspace.root);
      for (const c of containers) {
        for (const t of c.tabs) {
          if (t.kind === "resume" && t.sessionId) {
            const s = sessions.find((x) => x.session_id === t.sessionId);
            if (s && s.cwd && s.last_cwd && s.cwd !== s.last_cwd) {
              invoke("ensure_session_symlink", { originalCwd: s.cwd, targetCwd: s.last_cwd }).catch(() => {});
            }
          }
        }
      }
    } catch {}
    // Shell tab 自动绑定 session：如果有 running session 的 pty_id 匹配某个 shell tab
    const runningSessions = sessions.filter((s) => s.running && s.pty_id);
    if (runningSessions.length > 0) {
      const containers = collectContainers(ws.workspace.root);
      for (const c of containers) {
        for (const t of c.tabs) {
          if ((t.kind === "shell" || t.kind === "new") && !t.sessionId) {
            const match = runningSessions.find((s) => s.pty_id === t.id);
            if (match) {
              ws.bindSessionToTab(c.id, t.id, match.session_id, match.short_id);
            }
          }
        }
      }
    }
  }, [sessions]);


  // 全局 Cmd/Ctrl 按下状态：用于终端链接 underline 显示（仅按住 Cmd 时识别为链接）
  useEffect(() => {
    function onDown(e: KeyboardEvent) {
      if (e.key === "Meta" || e.key === "Control") document.body.classList.add("cmd-pressed");
    }
    function onUp(e: KeyboardEvent) {
      if (e.key === "Meta" || e.key === "Control") document.body.classList.remove("cmd-pressed");
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

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<boolean>("window-focus-changed", ({ payload: focused }) => {
      document.body.classList.toggle("window-focused", focused);
    }).then(fn => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  // 屏蔽 macOS WKWebView 默认右键菜单（Look Up / Translate / Search with Baidu...）
  // 终端选中文本时这个菜单很碍事；xterm 自己用 mousedown 处理鼠标，不依赖 contextmenu
  useEffect(() => {
    function handler(e: MouseEvent) {
      e.preventDefault();
    }
    window.addEventListener("contextmenu", handler);
    return () => window.removeEventListener("contextmenu", handler);
  }, []);


  // Cmd+B / Cmd+\ / Cmd+Shift+B → 切换侧栏（SessionTree）
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey)) return;
      const k = e.key.toLowerCase();
      if (k === "b") {
        e.preventDefault();
        setProjectListCollapsed((c) => !c);
        return;
      }
    }
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
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
        requestAnimationFrame(() => searchInputRef.current?.focus());
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
          ws.handleTabClose(c.id, c.activeTabId);
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
      // Cmd+1~9 切到第 N 个 container（对标产品/iTerm 风格）
      if (k >= "1" && k <= "9") {
        e.preventDefault();
        e.stopPropagation();
        const idx = parseInt(k, 10) - 1;
        const containers = collectContainers(ws.workspace.root);
        if (containers[idx]) ws.setActive(containers[idx].id);
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
      if (!(e.metaKey || e.ctrlKey) || !e.altKey) return;
      const k = e.key.toLowerCase();
      // ⌘⌥↩ 最大化/还原当前 container（muxy 同款）
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
        requestAnimationFrame(() => {
          const wrapper = document.querySelector(`[data-container-id="${target}"]`);
          const cv = wrapper?.querySelector(".container-view") as HTMLElement | null;
          if (cv) {
            cv.classList.remove("container-flash");
            void cv.offsetWidth;
            cv.classList.add("container-flash");
            setTimeout(() => cv.classList.remove("container-flash"), 500);
          }
        });
      }
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [ws.workspace]);

  // Cmd+[ / ] 或 Cmd+Shift+[ / ] 切同 container 内 tab
  // macOS: Shift+[ = {, Shift+] = }，所以也检测 { }
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      const isPrev = e.key === "[" || e.key === "{" || e.code === "BracketLeft";
      const isNext = e.key === "]" || e.key === "}" || e.code === "BracketRight";
      if (!isPrev && !isNext) return;
      e.preventDefault();
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

  // Cmd+I = 通知中心（对标产品 同款）
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey) return;
      if (e.key !== "i" && e.key !== "I") return;
      e.preventDefault();
      setNotifPanelOpen((v) => !v);
    }
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Cmd+R = WebView 页面刷新（原生行为，不拦截）→ 自动触发 load()

  // Cmd+L 聚焦到左树当前 session（展开侧栏 + 滚动到高亮 session）
  const [revealTrigger, setRevealTrigger] = useState(0);
  const [clearFilterTrigger, setClearFilterTrigger] = useState(0);
  useEffect(() => {
    function handler(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "l") {
        e.preventDefault();
        setProjectListCollapsed(false);
        setRevealTrigger((n) => n + 1);
      }
    }
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

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

  // 给 session tab 生成显示名：rename/slug → first_user_msg → short_id
  function deriveSessionTabLabel(s: SessionMeta): string {
    if (s.display_name && s.display_name.trim()) {
      return s.display_name;
    }
    if (s.first_user_msg && s.first_user_msg.trim()) {
      return truncate(s.first_user_msg, 24);
    }
    return `[${s.short_id}]`;
  }

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

  function openResumeTab(s: SessionMeta, _projectRoot: string) {
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
    const label = deriveSessionTabLabel(s);
    const resumeCwd = s.cwd;
    ensureSessionPath(s);
    ws.openSession(s.session_id, s.short_id, resumeCwd, label, s.tool);
  }






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
        // 运行中的 session 归档需二次确认
        if (s.running) {
          const ok = window.confirm(
            `该 session 正在运行中（PID ${s.pid}），归档将关闭终端并杀死子进程。确定归档？`
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
        // 归档后：先杀子进程，再关闭所有相关 tab
        if (s.child_processes?.length) {
          invoke("kill_pids", { pids: s.child_processes.map((p) => p.pid) }).catch(() => {});
        }
        // 收集所有要关闭的 tab（先收集再关闭，避免循环中修改 tree）
        const toClose: { containerId: string; tabId: string }[] = [];
        for (const c of collectContainers(ws.workspace.root)) {
          for (const t of c.tabs) {
            if (t.sessionId === s.session_id) {
              toClose.push({ containerId: c.id, tabId: t.id });
            }
          }
        }
        for (const { containerId, tabId } of toClose) {
          ws.handleTabClose(containerId, tabId);
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
    const home = "/Users/me";
    return cwd.startsWith(home) ? "~" + cwd.slice(home.length) : cwd;
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
            initCommand: `clear && ${sessionResumeCmd(s)}`,
            sessionId: s.session_id,
            sessionShortId: s.short_id,
          };
          e.dataTransfer.setData(DRAG_MIME, encodePaneSpec(spec));
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
                (s.name_source === "rename" ? " rename" : " slug")
              }
              title={s.name_source === "rename" ? "用户 rename" : "Claude 生成 slug"}
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
      const statusLabel = stat === "waiting"
        ? (s.waiting_for === "user" ? "等待回答" : "等待审批")
        : stat === "busy" ? "工作中"
        : stat === "idle" ? "空闲"
        : stat === "archived" ? "已归档"
        : stat === "stopped" ? "已停止"
        : "";
      items.push({
        id: `s:${s.session_id}:${group}`,
        title: deriveSessionTabLabel(s),
        subtitle: `${root} · [${s.short_id}]${s.git_branch ? " · @" + s.git_branch : ""}`,
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
              { kind: "resume", cwd: s.cwd, initCommand: `clear && ${sessionResumeCmd(s)}`, sessionId: s.session_id, sessionShortId: s.short_id }
            );
          } else {
            openResumeTab(s, s.cwd);
          }
        },
      });
    }
    const sortKey = (localStorage.getItem("ccs-palette-sort") as SortKey) || "recent";
    const waitingUser = waiting.filter((s) => s.waiting_for === "user");
    const waitingApproval = waiting.filter((s) => s.waiting_for !== "user");
    for (const s of sortSessions(waitingUser, sortKey, pinnedSessions)) pushSession(s, "? 等待回答");
    for (const s of sortSessions(waitingApproval, sortKey, pinnedSessions)) pushSession(s, "⚠ 等待审批");
    for (const s of sortSessions(running, sortKey, pinnedSessions)) pushSession(s, "▶ 工作中");
    for (const s of sortSessions(allActive.filter((s) => !s.running), sortKey, pinnedSessions).slice(0, 30)) pushSession(s, "⏱ 最近活跃");
    const archived = sortSessions(sessions.filter((s) => s.archived), sortKey, pinnedSessions).slice(0, 30);
    for (const s of archived) pushSession(s, "🗄 已归档");

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
            title="通知中心 (⌘I)"
          >
            <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 16a2 2 0 0 0 2-2H6a2 2 0 0 0 2 2zm.995-14.901a1 1 0 1 0-1.99 0A5.002 5.002 0 0 0 3 6c0 1.098-.5 6-2 7h14c-1.5-1-2-5.902-2-7 0-2.42-1.72-4.44-4.005-4.901z"/>
            </svg>
            {unreadCount > 0 && (
              <span className="notif-bell-badge">{unreadCount > 99 ? "99+" : unreadCount}</span>
            )}
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
              initCommand: `clear && ${sessionResumeCmd(s)}`,
              sessionId: s.session_id,
              sessionShortId: s.short_id,
            };
            e.dataTransfer.setData(DRAG_MIME, encodePaneSpec(spec));
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
          onSettings={() => setSettingsOpen(true)}
          refreshing={loading}
          collapsed={projectListCollapsed}
          onCollapse={() => setProjectListCollapsed((c) => !c)}
          width={projectList.width}
          onResizeStart={(x) => projectList.startDrag(x)}
          onResizerHover={setResizerHovered}
          onNewSessionInDir={(cwd, tool) => ws.openNewSession(cwd, tool)}
          onNewShellInDir={(cwd) => ws.openShell(cwd)}
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
            onTabClose={(containerId, tabId) => {
              // 关闭 tab 前：杀 session 的子进程（包括已被 launchd 收养的孤儿）
              const c = findContainer(ws.workspace.root, containerId);
              const tab = c?.tabs.find((t) => t.id === tabId);
              if (tab?.sessionId) {
                const meta = sessionsMap.get(tab.sessionId);
                if (meta?.child_processes?.length) {
                  const pids = meta.child_processes.map((p) => p.pid);
                  invoke("kill_pids", { pids }).catch(() => {});
                }
              }
              ws.handleTabClose(containerId, tabId);
            }}
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
              setContainerContextMenu({ containerId, x, y });
            }}
            getTabTitle={stableGetTabTitle}
            getTabStatus={stableGetTabStatus}
          />
        </section>
      </div>

      {containerContextMenu && (
        <div className="context-menu-backdrop" onClick={() => setContainerContextMenu(null)}>
          <div className="context-menu" style={{ left: containerContextMenu.x, top: containerContextMenu.y }} onClick={(e) => e.stopPropagation()}>
            <button onClick={() => { ws.handleSplit(containerContextMenu.containerId, "v"); setContainerContextMenu(null); }}>Split 左右</button>
            <button onClick={() => { ws.handleSplit(containerContextMenu.containerId, "h"); setContainerContextMenu(null); }}>Split 上下</button>
            <button onClick={() => { ws.handleNewShell(containerContextMenu.containerId); setContainerContextMenu(null); }}>新终端</button>
            <button onClick={() => {
              const c = findContainer(ws.workspace.root, containerContextMenu.containerId);
              if (c) ws.handleTabClose(containerContextMenu.containerId, c.activeTabId);
              setContainerContextMenu(null);
            }}>关闭</button>
          </div>
        </div>
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

      <CommandPalette
        open={paletteOpen}
        onClose={() => {
          setPaletteOpen(false);
          // 关闭后重新 focus 终端
          const c = ws.getActiveContainer();
          if (c?.activeTabId) {
            requestAnimationFrame(() => terminalManager.focus(c.activeTabId));
          }
        }}
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
              requestAnimationFrame(() => {
                const wrapper = document.querySelector(`[data-container-id="${existing.containerId}"]`);
                const cv = wrapper?.querySelector(".container-view") as HTMLElement | null;
                if (cv) {
                  cv.classList.remove("container-flash");
                  void cv.offsetWidth;
                  cv.classList.add("container-flash");
                  setTimeout(() => cv.classList.remove("container-flash"), 500);
                }
              });
            } else {
              // session 不在窗口 → 完整导航 + 侧边栏定位
              revealSidebarSession({ sessionId, cwd: meta.cwd });
            }
          }}
          anchorLeft={notifPanelLeft}
          onClose={() => setNotifPanelOpen(false)}
        />
      )}

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
                    <option value="vscode-dark">VS Code Dark</option>
                    <option value="github-dark">GitHub Dark</option>
                    <option value="tokyo-night">Tokyo Night</option>
                    <option value="dracula">Dracula</option>
                    <option value="catppuccin-mocha">Catppuccin Mocha</option>
                    <option value="nord">Nord</option>
                    <option value="one-dark">One Dark</option>
                    <option value="monokai-pro">Monokai Pro</option>
                    <option value="gruvbox-dark">Gruvbox Dark</option>
                    <option value="rose-pine">Rosé Pine</option>
                    <option value="solarized-dark">Solarized Dark</option>
                  </optgroup>
                  <optgroup label="浅色">
                    <option value="vscode-light">VS Code Light</option>
                    <option value="github-light">GitHub Light</option>
                    <option value="catppuccin-latte">Catppuccin Latte</option>
                    <option value="solarized-light">Solarized Light</option>
                  </optgroup>
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
              </section>

              <section className="settings-section">
                <div className="settings-label">列表显示</div>
                <label className="settings-toggle-row">
                  <input type="checkbox" checked={localStorage.getItem("ccs-show-attention") !== "false"} onChange={(e) => { localStorage.setItem("ccs-show-attention", String(e.target.checked)); forceSettingsRender((n) => n + 1); }} />
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
                <button
                  className="settings-action-btn"
                  onClick={() => {
                    invoke<string>("install_claude_hook")
                      .then((r) => alert(r === "already_installed" ? "Hook 已安装，无需重复操作。" : "Hook 安装成功！Claude Code 重启后生效。"))
                      .catch((e) => alert("安装失败：" + e));
                  }}
                >
                  安装 Claude Code Hook（推送模式）
                </button>
                <div className="settings-hint">将 ccs-hook.sh 注册到 ~/.claude/settings.json，Claude Code 发出通知时实时推送（无需轮询）。</div>
              </section>


              <section className="settings-section">
                <div className="settings-label">Hover 详情卡片</div>
                <select className="settings-select" value={localStorage.getItem("ccs-hover-mode") || "always"} onChange={(e) => { localStorage.setItem("ccs-hover-mode", e.target.value); forceSettingsRender((n) => n + 1); }}>
                  <option value="always">始终显示（400ms 延迟）</option>
                  <option value="cmd">仅按住 ⌘ 时显示</option>
                  <option value="off">关闭</option>
                </select>
                <div className="settings-hint">Hover 时在 session 旁弹出详细信息卡片（ID、路径、分支、话题等），点击可复制。</div>
              </section>

              <section className="settings-section"><div className="settings-label">快捷键</div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">搜索 / 命令</div>
                  <div className="settings-shortcuts">
                    <div><kbd>⌘K</kbd><span>全局搜索（命令面板）</span></div>
                    <div><kbd>⌘F</kbd><span>当前终端内搜索</span></div>
                    <div><kbd>⌘⇧F</kbd><span>聚焦侧栏 session 搜索</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">布局 / 视图</div>
                  <div className="settings-shortcuts">
                    <div><kbd>⌘B</kbd><span>折叠项目列表</span></div>
                    <div><kbd>⌘\</kbd><span>折叠会话列表</span></div>
                    <div><kbd>⌘D</kbd><span>左右分屏</span></div>
                    <div><kbd>⌘⇧D</kbd><span>上下分屏</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">终端操作</div>
                  <div className="settings-shortcuts">
                    <div><kbd>⌘T</kbd><span>新建 shell tab</span></div>
                    <div><kbd>⌘W</kbd><span>关闭当前 tab</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">导航</div>
                  <div className="settings-shortcuts">
                    <div><kbd>⌘1</kbd>~<kbd>⌘9</kbd><span>切到第 N 个 container</span></div>
                    <div><kbd>⌘⌥←</kbd><kbd>⌘⌥→</kbd><span>上/下一个 container</span></div>
                    <div><kbd>⌘⌥↑</kbd><kbd>⌘⌥↓</kbd><span>上/下一个 container</span></div>
                  </div>
                </div>
                <div className="settings-shortcut-group">
                  <div className="settings-shortcut-group-title">面板内</div>
                  <div className="settings-shortcuts">
                    <div><kbd>↑↓</kbd><span>命令面板列表选择</span></div>
                    <div><kbd>↵</kbd><span>打开 / 确认</span></div>
                    <div><kbd>⌘↵</kbd><span>命令面板：在 split 中打开</span></div>
                    <div><kbd>Esc</kbd><span>关闭面板 / 取消</span></div>
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
