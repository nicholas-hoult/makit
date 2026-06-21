import { memo, useEffect, useState, useRef, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";

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
  collapsed: boolean;
  onCollapse: () => void;
  width: number;
  onResizeStart: (x: number) => void;
  onResizerHover?: (hovered: boolean) => void;
  showAttention?: boolean;
  onNewSessionInDir?: (cwd: string, tool?: string) => void;
  onNewShellInDir?: (cwd: string) => void;
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
    const hasActive = sessions.some((s) => s.status === "busy" || s.status === "waiting" || s.running);
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

function statusIcon(s: SessionMeta): string {
  if (s.status === "waiting") return "●";
  if (s.running) return "▶";
  return "○";
}

function statusClass(s: SessionMeta): string {
  if (s.status === "waiting") return "waiting";
  if (s.status === "busy") return "busy";
  if (s.running) return "idle";
  return "stopped";
}

export const SessionTree = memo(function SessionTree({
  sessions, pinnedSessions, activeSessionId, loading, query, onQueryChange,
  onSessionClick, onSessionDragStart,
  onTogglePin, onArchive, onRefresh, onSettings, refreshing, revealTrigger, clearFilterTrigger,
  collapsed, onCollapse, width, onResizeStart, onResizerHover, onNewSessionInDir, onNewShellInDir,
}: Props) {
  const [sortKey, setSortKey] = useState<SortKey>(() => (localStorage.getItem("ccs-tree-sort") as SortKey) || "recent");
  const [showPinnedOnly, setShowPinnedOnly] = useState(() => localStorage.getItem("ccs-tree-pinned-only") === "true");
  const [showArchived, setShowArchived] = useState(() => localStorage.getItem("ccs-tree-show-archived") === "true");
  const [showRunningOnly, setShowRunningOnly] = useState(() => localStorage.getItem("ccs-tree-running-only") === "true");
  const [viewMode, setViewMode] = useState<"status" | "project">(
    () => (localStorage.getItem("ccs-sidebar-view") as "status" | "project") || "status"
  );
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem("ccs-proj-collapsed");
      return new Set(saved ? JSON.parse(saved) : []);
    } catch {
      return new Set();
    }
  });
  const [collapsedSections, setCollapsedSections] = useState<Set<string>>(() => {
    try {
      const saved = localStorage.getItem("ccs-section-collapsed");
      return new Set(saved ? JSON.parse(saved) : []);
    } catch { return new Set(); }
  });
  function toggleSection(title: string) {
    setCollapsedSections((prev) => {
      const next = new Set(prev);
      next.has(title) ? next.delete(title) : next.add(title);
      localStorage.setItem("ccs-section-collapsed", JSON.stringify([...next]));
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
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; session: SessionMeta } | null>(null);
  const [hoverCard, setHoverCard] = useState<{ x: number; y: number; session: SessionMeta } | null>(null);
  const [visibleRecent, setVisibleRecent] = useState(RECENT_PAGE);
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const treeBodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => { localStorage.setItem("ccs-tree-sort", sortKey); }, [sortKey]);
  useEffect(() => { localStorage.setItem("ccs-tree-pinned-only", String(showPinnedOnly)); }, [showPinnedOnly]);
  useEffect(() => { localStorage.setItem("ccs-tree-show-archived", String(showArchived)); }, [showArchived]);
  useEffect(() => { localStorage.setItem("ccs-tree-running-only", String(showRunningOnly)); }, [showRunningOnly]);
  useEffect(() => { localStorage.setItem("ccs-sidebar-view", viewMode); }, [viewMode]);
  useEffect(() => {
    localStorage.setItem("ccs-proj-collapsed", JSON.stringify([...collapsedProjects]));
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

  // 段 1：监控（waiting / running）
  const monitor = useMemo(() => {
    return filtered
      .filter((s) => s.status === "waiting" || s.running)
      .sort((a, b) => {
        const aw = a.status === "waiting" ? 1 : 0;
        const bw = b.status === "waiting" ? 1 : 0;
        if (aw !== bw) return bw - aw;
        return b.mtime - a.mtime;
      });
  }, [filtered]);
  const monitorIds = useMemo(() => new Set(monitor.map((s) => s.session_id)), [monitor]);

  // 段 2：工作集（pin 但不在监控）
  const pinned = useMemo(() => {
    return filtered
      .filter((s) => pinnedSessions.has(s.session_id) && !monitorIds.has(s.session_id))
      .sort(sortFn);
  }, [filtered, pinnedSessions, monitorIds, sortKey]);

  // 段 3：最近（其余）
  const recent = useMemo(() => {
    return filtered
      .filter((s) => !pinnedSessions.has(s.session_id) && !monitorIds.has(s.session_id))
      .sort(sortFn);
  }, [filtered, pinnedSessions, monitorIds, sortKey]);
  const recentVisible = useMemo(() => recent.slice(0, visibleRecent), [recent, visibleRecent]);

  const projectGroups = useMemo(() => {
    if (viewMode !== "project") return [];
    return groupByGitRoot(filtered);
  }, [filtered, viewMode]);

  // reveal：滚动到 active session（必要时翻页）
  useEffect(() => {
    if (!revealTrigger || !activeSessionId) return;
    const idx = recent.findIndex((s) => s.session_id === activeSessionId);
    if (idx >= visibleRecent) {
      setVisibleRecent(Math.ceil((idx + 1) / RECENT_PAGE) * RECENT_PAGE);
    }
    requestAnimationFrame(() => {
      const el = treeBodyRef.current?.querySelector(".tree-session.focused") as HTMLElement | null;
      el?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
  }, [revealTrigger]);

  // 滚动加载：滚到底部 200px 内自动加载下一页
  useEffect(() => {
    const body = treeBodyRef.current;
    if (!body) return;
    function onScroll() {
      if (!body) return;
      if (body.scrollTop + body.clientHeight + 200 >= body.scrollHeight) {
        if (visibleRecent < recent.length) {
          setVisibleRecent((v) => Math.min(v + RECENT_PAGE, recent.length));
        }
      }
    }
    body.addEventListener("scroll", onScroll);
    return () => body.removeEventListener("scroll", onScroll);
  }, [recent.length, visibleRecent]);

  // 折叠时整条侧栏隐藏（拖窗/红绿黄占位由全局 .app-titlebar 负责）
  if (collapsed) return null;

  function renderSession(s: SessionMeta) {
    const isPinned = pinnedSessions.has(s.session_id);
    const title = s.display_name || s.first_user_msg || `[${s.short_id}]`;
    const projectName = basename(s.git_root || s.cwd);
    const statusLabel = s.status === "waiting" ? "等待审批" : s.status === "busy" ? "工作中" : s.running ? "空闲" : "";
    return (
      <li
        key={s.session_id}
        data-session-id={s.session_id}
        className={"tree-session " + statusClass(s) + (s.session_id === activeSessionId ? " focused" : "")}
        onClick={() => onSessionClick(s)}
        onContextMenu={(e) => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, session: s }); }}
        onMouseEnter={(e) => {const mode = localStorage.getItem("ccs-hover-mode") || "always";
          if (mode === "off") return;
          if (mode === "cmd" && !e.metaKey && !e.ctrlKey) return;
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
        <span className={"tree-status-icon " + statusClass(s)}>{statusIcon(s)}</span>
        {isPinned && <span className="tree-pin">★</span>}
        <div className="tree-session-content">
          <div className="tree-session-row1">
            <span className="tree-session-title">{title}</span>
            {statusLabel && <span className={"tree-status-badge " + statusClass(s)}>{statusLabel}</span>}
            <span className="tree-session-time">{s.humanize}</span>
          </div>
          <div className="tree-session-row2">
            {logoUrls[s.tool]
              ? <img src={logoUrls[s.tool]} className="tree-session-tool-logo" alt={s.tool} title={s.tool} />
              : s.tool === "codex" && <span className="tree-tool-badge codex">CX</span>
            }
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

  function renderProjectView() {
    return (
      <>
        {renderSection("活跃", monitor, { iconClass: "tree-attention", icon: "●" })}
        {projectGroups.map((group) => {
          const colKey = group.gitRoot || group.name;
          const defaultCollapsed = !group.hasActive;
          const isCollapsed = collapsedProjects.has(colKey)
            ? true
            : (!collapsedProjects.has("__expanded__" + colKey) && defaultCollapsed);
          return (
            <section key={colKey} className="tree-section tree-project-group">
              <div
                className={"tree-project-header" + (isCollapsed ? " collapsed" : "")}
                onClick={() => {
                  setCollapsedProjects((prev) => {
                    const next = new Set(prev);
                    const expandedKey = "__expanded__" + colKey;
                    if (isCollapsed) {
                      next.delete(colKey);
                      next.add(expandedKey);
                    } else {
                      next.delete(expandedKey);
                      next.add(colKey);
                    }
                    return next;
                  });
                }}
              >
                <span className={"tree-project-arrow" + (isCollapsed ? "" : " open")}>›</span>
<span className="tree-project-name">{group.name}</span>
                <span className="tree-section-count">{group.sessions.length}</span>
                {(onNewSessionInDir || onNewShellInDir) && (
                  <button
                    className="tree-project-add"
                    title={`在 ${group.name} 新建 session（⌘点击新建 shell）`}
                    onClick={(e) => {
                      e.stopPropagation();
                      const cwd = group.gitRoot || group.sessions[0]?.cwd || "~";
                      if (e.metaKey || e.ctrlKey) { onNewShellInDir?.(cwd); return; }
                      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
                      setToolPicker({ cwd, tools: ["claude", "codex"], x: rect.right + 4, y: rect.top });
                    }}
                  >+</button>
                )}
              </div>
              {!isCollapsed && (
                <ul className="tree-session-list">
                  {group.sessions.map(renderSession)}
                </ul>
              )}
            </section>
          );
        })}
        {projectGroups.length === 0 && monitor.length === 0 && (
          <div className="tree-empty">{loading ? "加载中…" : (query ? "无匹配 session" : "无 session")}</div>
        )}
      </>
    );
  }

  function renderSection(title: string, list: SessionMeta[], opts: { iconClass?: string; icon?: string } = {}) {
    if (list.length === 0) return null;
    const collapsed = collapsedSections.has(title);
    return (
      <section className={"tree-section " + (opts.iconClass || "")}>
        <div className="tree-section-header tree-section-header-clickable" onClick={() => toggleSection(title)}>
          {opts.icon && <span className="tree-section-icon">{opts.icon}</span>}
          <span>{title}</span>
          <span className="tree-section-count">{list.length}</span>
        </div>
        {!collapsed && <ul className="tree-session-list">{list.map(renderSession)}</ul>}
      </section>
    );
  }

  return (
    <>
    <aside className="session-tree" style={{ width }}>
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

        <select className="tree-view-select" value={viewMode} onChange={(e) => setViewMode(e.target.value as "status" | "project")}>
          <option value="status">按状态</option>
          <option value="project">按项目</option>
        </select>

        <button
          className="tree-filter-btn"
          onClick={() => setFilterOpen(!filterOpen)}
          title="筛选"
        >
          ⚙
        </button>
      </div>

      <div className="tree-body" ref={treeBodyRef}>
        {viewMode === "status" ? (
          <>
            {renderSection("活跃", monitor, { iconClass: "tree-attention", icon: "●" })}
            {renderSection("收藏", pinned, { icon: "★" })}
            {renderSection("最近", recentVisible, { icon: "○" })}
            {recentVisible.length < recent.length && (
              <div className="tree-load-more" onClick={() => setVisibleRecent((v) => Math.min(v + RECENT_PAGE, recent.length))}>
                加载更多（{recent.length - recentVisible.length} 个）
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
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09a1.65 1.65 0 00-1-1.51 1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09a1.65 1.65 0 001.51-1 1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06a1.65 1.65 0 001.82.33H9a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z"/></svg>
        </button>
        {filterOpen && (
          <>
            <div className="tree-filter-backdrop" onClick={() => setFilterOpen(false)} />
            <div className="tree-filter-panel tree-filter-panel-up">
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
        <div className="tree-context-backdrop" onClick={() => setToolPicker(null)} />
        <div className="tree-context-menu" style={{ left: toolPicker.x, top: toolPicker.y }}>
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
      <div className="tree-context-backdrop" onClick={() => setContextMenu(null)}>
        <div
          className="tree-context-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          ref={(el) => {
            if (!el) return;
            const rect = el.getBoundingClientRect();
            const vw = window.innerWidth;
            const vh = window.innerHeight;
            let x = contextMenu.x;
            let y = contextMenu.y;
            if (rect.bottom > vh) y = vh - rect.height - 8;
            if (rect.right > vw) x = vw - rect.width - 8;
            if (y < 4) y = 4;
            if (x < 4) x = 4;
            if (x !== contextMenu.x || y !== contextMenu.y) {
              el.style.left = x + "px";
              el.style.top = y + "px";
            }
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <button onClick={() => { onTogglePin(contextMenu.session.session_id); setContextMenu(null); }}>
            {pinnedSessions.has(contextMenu.session.session_id) ? "取消收藏" : "收藏"}
          </button>
          <button onClick={() => { onArchive(contextMenu.session.session_id); setContextMenu(null); }}>
            {contextMenu.session.archived ? "取消归档" : "归档"}
          </button>
          <button onClick={() => { navigator.clipboard.writeText(contextMenu.session.cwd).catch(() => {}); setContextMenu(null); }}>
            复制路径
          </button>
          <button onClick={() => { navigator.clipboard.writeText(contextMenu.session.session_id).catch(() => {}); setContextMenu(null); }}>
            复制 ID
          </button>
        </div>
      </div>
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
