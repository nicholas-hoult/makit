import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { isCmd } from "./keys";
import { STATUS_LABEL } from "./sessionStatus";

export type PaletteItem = {
  id: string;
  title: string;
  subtitle?: string;
  hint?: string;
  group?: string;
  type?: string;
  projectRoot?: string;
  // 实际流进来的还有 "stopped" / "archived"（见 App.tsx pushSession），CSS 里
  // .palette-status-dot.stopped/.archived 也一直在按这两个值写，只有这个类型漏了，
  // 于是调用处得靠 `stat as any` 绕过去。补齐它。
  status?: "waiting" | "busy" | "idle" | "stopped" | "archived" | null;  // 用于状态点显示
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

const HISTORY_KEY = "makit-palette-history";
const COLLAPSED_KEY = "makit-palette-collapsed";
const FILTER_KEY = "makit-palette-filter";
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

/**
 * 把命中的子串标出来。
 *
 * 搜索本身是大小写不敏感的 includes（见下面 filtered），这里必须用**同一条规则**切分：
 * 高亮和"这条为什么会出现"是同一个问题的两面，规则一旦不一致，就会出现一条完全没有
 * 高亮的结果，比不高亮更让人困惑。
 *
 * 命中可能落在 subtitle / group / type 上（hay 是四者拼起来的），所以标题没高亮是正常的，
 * 不是漏标 —— subtitle 也走这个函数，两处都没有就说明是 group/type 命中的。
 */
function highlight(text: string, needle: string): ReactNode {
  if (!needle) return text;
  const lower = text.toLowerCase();
  const q = needle.toLowerCase();
  const out: ReactNode[] = [];
  let cursor = 0;
  let key = 0;
  for (;;) {
    const at = lower.indexOf(q, cursor);
    if (at < 0) break;
    if (at > cursor) out.push(text.slice(cursor, at));
    out.push(<mark key={key++} className="palette-mark">{text.slice(at, at + q.length)}</mark>);
    cursor = at + q.length;
  }
  if (cursor === 0) return text;   // 一次都没命中，原样返回，不产生多余节点
  if (cursor < text.length) out.push(text.slice(cursor));
  return out;
}

// 状态药丸只留"需要人动手"的那一档 + 归档。
//
// 这条规则侧栏已经定过了（SessionTree.tsx 里 .tree-status-badge 那段注释）：
// 「工作中」和脉动的圆点说的是同一件事，「空闲」「已停止」更是每一行都挂一个灰药丸、
// 信息量为零却每行都在和标题抢宽度。⌘K 这边一直没跟上，于是列表里几乎每行右侧
// 都有一颗药丸，标题的可用宽度被凭空削掉一截。状态交给圆点。
//
// archived 例外：它不是运行态而是生命周期分桶，出现频率低（不破坏行的节奏），
// 而且一颗暗紫圆点没人能学会它是"已归档"。
function showsStatusPill(status: PaletteItem["status"]): boolean {
  return status === "waiting" || status === "archived";
}

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
  const [filterSort, setFilterSort] = useState<string>(() => localStorage.getItem("makit-palette-sort") || "recent");
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
      if (e.key === "Escape" || (e.key === "k" && isCmd(e))) { e.preventDefault(); onClose(); }
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
        const mod = e.shiftKey && isCmd(e) ? "newContainer"
          : isCmd(e) ? "split" : "default";
        activate(item, mod);
      }
    }
  }

  const hasFilters = filterProject || filterTime !== "all" || filterStatus.size > 0 || filterPinnedOnly;
  const q = query.trim();   // 给 highlight() 用；大小写归一在函数里做

  // 抽出来是因为空状态那边也要用（原来只有筛选栏底部一个入口）。
  // 刻意不重置 activeType：那是顶部 chips 的状态，和侧栏这几项不是一组，
  // 一起清掉会让人以为 chips 坏了。
  function clearFilters() {
    setFilterProject("");
    setFilterTime("all");
    setFilterStatus(new Set());
    setFilterPinnedOnly(false);
  }

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
              <div className="palette-empty">
                <div className="palette-empty-title">
                  {q ? <>没有匹配 <span className="palette-empty-q">{q}</span> 的结果</> : "这里什么都没有"}
                </div>
                {/* 筛选条件是存在 localStorage 里的（FILTER_KEY），会跨次启动留着。
                    上次筛了「只看置顶」忘了清，下次打开 ⌘K 就是一片空白、且没有任何线索
                    说明为什么 —— 空状态是唯一该说这句话的地方。 */}
                {hasFilters && (
                  <button className="palette-empty-clear" onClick={clearFilters}>
                    当前有筛选生效，点这里清除
                  </button>
                )}
              </div>
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
                    {/* 一个字形转 90°，不是换字形。原来是 ▸/▾ 互换 —— 这两个字符的
                        视觉重量差挺多（▾ 明显更粗更宽），展开/收起时组头会"跳"一下；
                        而且 .palette-section-chevron 上那句 `transition: transform`
                        因为根本没有 transform 在变，一直是死的。 */}
                    <span className="palette-section-chevron">▸</span>
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
                          const mod = e.shiftKey && isCmd(e) ? "newContainer"
                            : isCmd(e) ? "split" : "default";
                          activate(it, mod);
                        }}
                        onMouseEnter={() => { if (mouseMovedRef.current) setActiveIdx(idx); }}
                      >
                        {/* 前导槽宽度固定：状态点和 ★ 都是"有就画"的，直接并排放会让标题的
                            左边缘在相邻两行之间来回抖，扫列表时眼睛锁不住一条左轨。
                            槽子空着也占位，标题的 x 就成了常量。 */}
                        <span className="palette-item-lead">
                          {it.status && <span className={"palette-status-dot " + it.status} title={it.statusLabel ?? it.status} />}
                          {it.pinned && <span className="palette-pin" title="置顶">★</span>}
                        </span>
                        <div className="palette-item-main">
                          <span className="palette-item-title">{highlight(it.title, q)}</span>
                          {it.subtitle && <span className="palette-item-sub">{highlight(it.subtitle, q)}</span>}
                        </div>
                        <span className="palette-item-meta">
                          {it.statusLabel && showsStatusPill(it.status) && (
                            <span className={"palette-status-label " + (it.status ?? "")}>{it.statusLabel}</span>
                          )}
                          {it.hint && <span className="palette-item-hint">{it.hint}</span>}
                        </span>
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
            <span><kbd>⌘↵</kbd> 拆分</span>
            {/* onKey / onClick 里一直支持 ⌘⇧↵ = newContainer，只有这行提示没写出来 */}
            <span><kbd>⌘⇧↵</kbd> 新容器</span>
            <span className="palette-footer-end"><kbd>Esc</kbd> 取消</span>
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
              {/* 字形单独放进定宽槽：原来是拼在文案里的（"⚠ 等待审批"、"🗄 已归档"），
                  而 ⚠/?/▶/○ 和 emoji 🗄 的字宽各不相同、"已停止"又干脆没有字形，
                  六行的文字左缘是锯齿状的。顺手把 🗄 换成等宽的 ▤ —— emoji 是彩色的，
                  夹在一列单色符号里最扎眼。 */}
              {([
                // 文案和侧栏同一套（#187），只在 sessionStatus.ts 定义一处
                ["waiting_approval", "⚠", STATUS_LABEL.waiting_approval],
                ["waiting_user", "?", STATUS_LABEL.waiting_user],
                ["busy", "▶", STATUS_LABEL.busy],
                ["idle", "○", STATUS_LABEL.idle],
                ["stopped", "·", STATUS_LABEL.stopped],
                ["archived", "▤", STATUS_LABEL.archived],
              ] as [StatusKey, string, string][]).map(([k, glyph, label]) => (
                <label key={k} className="palette-filter-row">
                  <input type="checkbox" checked={filterStatus.has(k)} onChange={() => toggleStatus(k)} />
                  <span className={"palette-filter-glyph " + k}>{glyph}</span>
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
                  <input type="radio" name="sort" checked={filterSort === k} onChange={() => { setFilterSort(k); localStorage.setItem("makit-palette-sort", k); }} />
                  {k === "recent" ? "最近活动" : k === "count" ? "消息数" : "首条消息"}
                </label>
              ))}
            </div>

            {hasFilters && (
              <button className="palette-filter-clear" onClick={clearFilters}>清除全部筛选</button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
