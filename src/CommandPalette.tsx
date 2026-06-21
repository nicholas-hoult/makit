import { useEffect, useMemo, useRef, useState } from "react";

export type PaletteItem = {
  id: string;
  title: string;
  subtitle?: string;
  hint?: string;
  group?: string;
  type?: string;
  projectRoot?: string;
  status?: "waiting" | "busy" | "idle" | null;  // 用于状态点显示
  statusLabel?: string;   // 状态文字徽章
  waitingFor?: string;    // waiting 细分："user" = 等待回答，其他 = 等待审批
  running?: boolean;      // 用于「运行中/已停止」筛选
  pinned?: boolean;
  archived?: boolean;
  mtime?: number;
  msgCount?: number;
  action: (modifier?: "default" | "split" | "newContainer") => void;
};

type Props = {
  open: boolean;
  onClose: () => void;
  items: PaletteItem[];
  placeholder?: string;
  projects?: string[];      // 全部项目列表（用于侧栏筛选）
};

const HISTORY_KEY = "ccs-palette-history";
const COLLAPSED_KEY = "ccs-palette-collapsed";
const FILTER_KEY = "ccs-palette-filter";
const MAX_HISTORY = 8;

function loadCollapsed(): Set<string> {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw);
    return new Set(Array.isArray(arr) ? arr : []);
  } catch { return new Set(); }
}

function saveCollapsed(s: Set<string>) {
  try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify(Array.from(s))); } catch {}
}

type SavedFilter = {
  type?: string;
  project?: string;
  time?: TimeFilter;
  status?: string[];  // ["waiting","busy","idle","stopped"] 任意子集，空=全部
  pinnedOnly?: boolean;
};

function loadFilter(): SavedFilter {
  try {
    const raw = localStorage.getItem(FILTER_KEY);
    return raw ? JSON.parse(raw) : {};
  } catch { return {}; }
}

function saveFilter(f: SavedFilter) {
  try { localStorage.setItem(FILTER_KEY, JSON.stringify(f)); } catch {}
}

function loadHistory(): string[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    return Array.isArray(arr) ? arr.slice(0, MAX_HISTORY) : [];
  } catch { return []; }
}

function saveHistory(q: string) {
  if (!q.trim()) return;
  try {
    const cur = loadHistory();
    const next = [q, ...cur.filter((x) => x !== q)].slice(0, MAX_HISTORY);
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next));
  } catch {}
}

type TimeFilter = "all" | "today" | "week" | "month";
type StatusKey = "waiting_approval" | "waiting_user" | "busy" | "idle" | "stopped" | "archived";

export function CommandPalette({ open, onClose, items, placeholder, projects = [] }: Props) {
  const initFilter = loadFilter();
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const [activeType, setActiveType] = useState<string>(initFilter.type ?? "全部");
  const [filterPanelOpen, setFilterPanelOpen] = useState(false);
  const [filterProject, setFilterProject] = useState<string>(initFilter.project ?? "");
  const [filterTime, setFilterTime] = useState<TimeFilter>(initFilter.time ?? "all");
  const [filterStatus, setFilterStatus] = useState<Set<StatusKey>>(() => new Set((initFilter.status ?? []) as StatusKey[]));
  const [filterPinnedOnly, setFilterPinnedOnly] = useState<boolean>(initFilter.pinnedOnly ?? false);
  const [filterSort, setFilterSort] = useState<string>(() => localStorage.getItem("ccs-palette-sort") || "recent");
  const [history, setHistory] = useState<string[]>([]);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => loadCollapsed());

  // 持久化筛选状态
  useEffect(() => {
    saveFilter({
      type: activeType, project: filterProject, time: filterTime,
      status: Array.from(filterStatus), pinnedOnly: filterPinnedOnly,
    });
  }, [activeType, filterProject, filterTime, filterStatus, filterPinnedOnly]);

  function toggleStatus(s: StatusKey) {
    setFilterStatus((prev) => {
      const next = new Set(prev);
      if (next.has(s)) next.delete(s); else next.add(s);
      return next;
    });
  }
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const mouseMovedRef = useRef(false);

  useEffect(() => {
    if (open) {
      mouseMovedRef.current = false;
      setQuery("");
      setActiveIdx(0);
      setFilterPanelOpen(false);
      setHistory(loadHistory());
      requestAnimationFrame(() => {
        inputRef.current?.focus();
        const el = listRef.current?.querySelector("[data-idx=\"0\"]") as HTMLElement | null;
        el?.scrollIntoView({ block: "nearest" });
      });
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" || (e.key === "k" && (e.metaKey || e.ctrlKey))) { e.preventDefault(); onClose(); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, onClose]);

  // 收集全部 type
  const types = useMemo(() => {
    const ts = ["全部"];
    for (const it of items) {
      if (it.type && !ts.includes(it.type)) ts.push(it.type);
    }
    return ts;
  }, [items]);

  // 时间过滤帮助
  const timeFilter = useMemo(() => {
    const now = Date.now() / 1000;
    if (filterTime === "today") return (mt: number) => now - mt < 86400;
    if (filterTime === "week") return (mt: number) => now - mt < 86400 * 7;
    if (filterTime === "month") return (mt: number) => now - mt < 86400 * 30;
    return null;
  }, [filterTime]);

  // 应用所有过滤
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items.filter((it) => {
      if (activeType !== "全部" && it.type !== activeType) return false;
      if (filterProject && it.projectRoot !== filterProject) return false;
      if (filterStatus.size > 0) {
        let itStat: StatusKey;
        if (it.status === "waiting") {
          itStat = it.waitingFor === "user" ? "waiting_user" : "waiting_approval";
        } else {
          itStat = (it.status ?? (it.running ? "idle" : "stopped")) as StatusKey;
        }
        if (!filterStatus.has(itStat)) return false;
      }
      if (filterPinnedOnly && !it.pinned) return false;
      if (timeFilter && it.mtime && !timeFilter(it.mtime)) return false;
      if (q) {
        const hay = `${it.title} ${it.subtitle ?? ""} ${it.group ?? ""} ${it.type ?? ""}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [items, activeType, filterProject, filterStatus, filterPinnedOnly, timeFilter, query]);

  // 按 group 分组 + 组内排序
  const grouped = useMemo(() => {
    const map = new Map<string, PaletteItem[]>();
    const order: string[] = [];
    for (const it of filtered) {
      const g = it.group ?? "";
      if (!map.has(g)) { map.set(g, []); order.push(g); }
      map.get(g)!.push(it);
    }
    const sortFn = (a: PaletteItem, b: PaletteItem) => {
      if (a.pinned !== b.pinned) return (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0);
      if (filterSort === "count") return (b.msgCount ?? 0) - (a.msgCount ?? 0);
      if (filterSort === "firstMsg") return (a.title || "").localeCompare(b.title || "");
      return (b.mtime ?? 0) - (a.mtime ?? 0);
    };
    return order.map((g) => ({ group: g, items: map.get(g)!.sort(sortFn) }));
  }, [filtered, filterSort]);

  // 每组无搜索时限制 10 条（减少首次渲染 DOM 节点数），有搜索时全量
  const MAX_PER_GROUP = query.trim() ? 999 : 10;

  // 扁平索引用于上下选择（排除折叠组的项）
  const flat = useMemo(() => {
    return grouped.flatMap(({ group, items: g }) =>
      collapsedGroups.has(group) ? [] : g.slice(0, MAX_PER_GROUP)
    );
  }, [grouped, collapsedGroups, MAX_PER_GROUP]);

  // 预计算 idx map（避免 render 时 O(n²) indexOf）
  const idxMap = useMemo(() => {
    const m = new Map<string, number>();
    flat.forEach((it, i) => m.set(it.id, i));
    return m;
  }, [flat]);

  useEffect(() => { setActiveIdx(0); }, [query, activeType, filterProject, filterTime, filterStatus, filterPinnedOnly, collapsedGroups]);

  function toggleCollapsed(group: string) {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group)) next.delete(group); else next.add(group);
      saveCollapsed(next);
      return next;
    });
  }

  // 选中项滚动到可见
  useEffect(() => {
    const el = listRef.current?.querySelector(`[data-idx="${activeIdx}"]`) as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest" });
  }, [activeIdx]);

  if (!open) return null;

  function activate(item: PaletteItem, modifier: "default" | "split" | "newContainer" = "default") {
    saveHistory(query);
    item.action(modifier);
    onClose();
  }

  function onKey(e: React.KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
    if (e.key === "ArrowDown") { e.preventDefault(); setActiveIdx((i) => Math.min(flat.length - 1, i + 1)); return; }
    if (e.key === "ArrowUp") { e.preventDefault(); setActiveIdx((i) => Math.max(0, i - 1)); return; }
    if (e.key === "Enter") {
      e.preventDefault();
      const item = flat[activeIdx];
      if (item) {
        const mod = e.shiftKey && (e.metaKey || e.ctrlKey) ? "newContainer"
          : (e.metaKey || e.ctrlKey) ? "split" : "default";
        activate(item, mod);
      }
    }
  }

  const hasFilters = filterProject || filterTime !== "all" || filterStatus.size > 0 || filterPinnedOnly;

  return (
    <div className="palette-backdrop" onClick={onClose}>
      <div className={"palette" + (filterPanelOpen ? " with-filter" : "")} onClick={(e) => e.stopPropagation()}>
        <div className="palette-main">
          <div className="palette-header">
            <svg className="palette-search-icon" width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><path d="M11.74 10.34a6 6 0 10-1.4 1.4l3.5 3.5a1 1 0 001.42-1.42l-3.52-3.48zM7 12a5 5 0 110-10 5 5 0 010 10z"/></svg>
            <input
              ref={inputRef}
              className="palette-input"
              placeholder={placeholder ?? "你想搜的，在这里都能搜到"}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onKey}
            />
            <button className={"palette-filter-btn" + (filterPanelOpen ? " active" : "") + (hasFilters ? " has-filters" : "")}
              onClick={() => setFilterPanelOpen((v) => !v)}
              title="筛选">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M2 3h12l-4.5 6v4l-3 1V9L2 3z"/></svg>
              <span>筛选</span>
            </button>
            <button className="palette-close" onClick={onClose} title="关闭 (Esc)">×</button>
          </div>

          {types.length > 2 && (
            <div className="palette-chips">
              {types.map((t) => (
                <button
                  key={t}
                  className={"palette-chip" + (t === activeType ? " active" : "")}
                  onClick={() => setActiveType(t)}
                >{t}</button>
              ))}
            </div>
          )}

          <div ref={listRef} className="palette-body" onMouseMove={() => { mouseMovedRef.current = true; }}>
            {!query && history.length > 0 && (
              <div className="palette-section">
                <div className="palette-section-title">
                  <span>搜索历史</span>
                  <button className="palette-section-action" onClick={() => { localStorage.removeItem(HISTORY_KEY); setHistory([]); }} title="清除">×</button>
                </div>
                <div className="palette-history">
                  {history.map((h) => (
                    <button key={h} className="palette-history-chip" onClick={() => setQuery(h)}>{h}</button>
                  ))}
                </div>
              </div>
            )}

            {grouped.length === 0 && (
              <div className="palette-empty">没有匹配项</div>
            )}

            {grouped.map(({ group, items: groupItems }) => {
              const collapsed = collapsedGroups.has(group);
              return (
              <div key={group || "__empty__"} className="palette-section">
                {group && (
                  <button
                    className={"palette-section-title palette-section-toggle" + (collapsed ? " collapsed" : "")}
                    onClick={() => toggleCollapsed(group)}
                  >
                    <span className="palette-section-chevron">{collapsed ? "▸" : "▾"}</span>
                    <span>{group}</span>
                    <span className="palette-section-count">{groupItems.length}</span>
                  </button>
                )}
                {!collapsed && (
                <ul className="palette-list">
                  {groupItems.slice(0, MAX_PER_GROUP).map((it) => {
                    const idx = idxMap.get(it.id) ?? -1;
                    return (
                      <li
                        key={it.id}
                        data-idx={idx}
                        className={"palette-item" + (idx === activeIdx ? " active" : "")}
                        onClick={(e) => {
                          const mod = e.shiftKey && (e.metaKey || e.ctrlKey) ? "newContainer"
                            : (e.metaKey || e.ctrlKey) ? "split" : "default";
                          activate(it, mod);
                        }}
                        onMouseEnter={() => { if (mouseMovedRef.current) setActiveIdx(idx); }}
                      >
                        {it.status && <span className={"palette-status-dot " + it.status} title={it.statusLabel ?? it.status} />}
                        {it.pinned && <span className="palette-pin" title="置顶">★</span>}
                        <div className="palette-item-main">
                          <span className="palette-item-title">{it.title}</span>
                          {it.subtitle && <span className="palette-item-sub">{it.subtitle}</span>}
                        </div>
                        {it.statusLabel && <span className={"palette-status-label " + (it.status ?? "")}>{it.statusLabel}</span>}
                        {it.hint && <span className="palette-item-hint">{it.hint}</span>}
                      </li>
                    );
                  })}
                </ul>
                )}
              </div>
              );
            })}
          </div>

          <div className="palette-footer">
            <span><kbd>↑↓</kbd> 选择</span>
            <span><kbd>↵</kbd> 打开</span>
            <span><kbd>⌘↵</kbd> 在 split 打开</span>
            <span><kbd>Esc</kbd> 取消</span>
          </div>
        </div>

        {filterPanelOpen && (
          <div className="palette-filter-panel">
            <div className="palette-filter-header">
              <span>筛选</span>
              <button onClick={() => setFilterPanelOpen(false)}>×</button>
            </div>

            <div className="palette-filter-section">
              <div className="palette-filter-label">项目</div>
              <select value={filterProject} onChange={(e) => setFilterProject(e.target.value)}>
                <option value="">全部</option>
                {projects.map((p) => (
                  <option key={p} value={p}>{p.split("/").pop()}</option>
                ))}
              </select>
            </div>

            <div className="palette-filter-section">
              <div className="palette-filter-label">时间</div>
              {(["all", "today", "week", "month"] as TimeFilter[]).map((t) => (
                <label key={t} className="palette-filter-row">
                  <input type="radio" checked={filterTime === t} onChange={() => setFilterTime(t)} />
                  {t === "all" ? "全部" : t === "today" ? "今天" : t === "week" ? "最近一周" : "最近一个月"}
                </label>
              ))}
            </div>

            <div className="palette-filter-section">
              <div className="palette-filter-label">运行状态（多选）</div>
              {([
                ["waiting_approval", "⚠ 等待审批"],
                ["waiting_user", "? 等待回答"],
                ["busy", "▶ 工作中"],
                ["idle", "○ 空闲（运行中）"],
                ["stopped", "已停止（未归档）"],
                ["archived", "🗄 已归档"],
              ] as [StatusKey, string][]).map(([k, label]) => (
                <label key={k} className="palette-filter-row">
                  <input type="checkbox" checked={filterStatus.has(k)} onChange={() => toggleStatus(k)} />
                  {label}
                </label>
              ))}
            </div>

            <div className="palette-filter-section">
              <div className="palette-filter-label">标签</div>
              <label className="palette-filter-row">
                <input type="checkbox" checked={filterPinnedOnly} onChange={(e) => setFilterPinnedOnly(e.target.checked)} />
                只看置顶
              </label>

              <div className="palette-filter-label">排序</div>
              {(["recent", "count", "firstMsg"] as const).map((k) => (
                <label className="palette-filter-row" key={k}>
                  <input type="radio" name="sort" checked={filterSort === k} onChange={() => { setFilterSort(k); localStorage.setItem("ccs-palette-sort", k); }} />
                  {k === "recent" ? "最近活动" : k === "count" ? "消息数" : "首条消息"}
                </label>
              ))}
            </div>

            {hasFilters && (
              <button className="palette-filter-clear" onClick={() => {setFilterProject(""); setFilterTime("all"); setFilterStatus(new Set()); setFilterPinnedOnly(false);
              }}>清除全部筛选</button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
