/**
 * 侧栏的分组、历史的日期分段、项目组内的优先级排序（#187 第二期）。
 *
 * 顺序参考了 Claude / Codex 两个桌面客户端（调研记录在 #187）：置顶放最上面；按状态分组时
 * 「等你处理」排第一，然后是在干活的、闲着的；历史按日期分段照 Claude 的做法（今天 / 昨天 /
 * 近几天各一天 / 更早）；项目组内照 Codex 的「优先级」排序。
 *
 * 抽成纯函数是为了能测：渲染和键盘导航都用这里算出来的同一份结果（见 treeOrder.ts）。
 */
import { runState, type RunState } from "./sessionStatus.ts";

export type GroupRow = { session_id: string; running: boolean; status: string; mtime: number };

export type StatusGroups<T> = {
  pinned: T[];
  opened: T[];
  attention: T[];
  busy: T[];
  idle: T[];
  history: T[];
};

/**
 * 按状态分组。链条先到先得，一条会话只进它符合条件的最靠前那一组：
 * 置顶 → 已打开 → 需要回应 → 进行中 → 空闲 → 历史（已停止）。
 *
 * - 置顶排在「已打开」前面（两个客户端都这么放），所以置顶的会话哪怕开着、在跑，也只在置顶里
 * - 已打开按屏幕上的标签顺序（openedIndex），不按时间：它是导航面板，位置比新旧重要
 * - 需要回应 / 进行中 / 空闲固定按最近活动：它们是拿来盯的，用户选的排序对它们没意义
 * - 置顶和历史尊重用户选的排序
 */
export function statusGroups<T extends GroupRow>(
  list: T[],
  opts: { openedIndex: Map<string, number>; pinned: Set<string>; sort: (a: T, b: T) => number },
): StatusGroups<T> {
  const taken = new Set<string>();
  const take = (pred: (s: T) => boolean, sorter: (a: T, b: T) => number) => {
    const out = list.filter((s) => !taken.has(s.session_id) && pred(s));
    for (const s of out) taken.add(s.session_id);
    return out.sort(sorter);
  };
  const byRecent = (a: T, b: T) => b.mtime - a.mtime;
  return {
    pinned: take((s) => opts.pinned.has(s.session_id), opts.sort),
    opened: take((s) => opts.openedIndex.has(s.session_id),
      (a, b) => opts.openedIndex.get(a.session_id)! - opts.openedIndex.get(b.session_id)!),
    attention: take((s) => runState(s) === "waiting", byRecent),
    busy: take((s) => runState(s) === "busy", byRecent),
    idle: take((s) => runState(s) === "idle", byRecent),
    history: take(() => true, opts.sort),
  };
}

export type DayBucket<T> = { id: string; label: string; list: T[] };

/** 今天、昨天之外，再单独列出几天（第 2~6 天）；第 7 天起并入「更早」。照 Claude 桌面版 */
const SINGLE_DAYS = 7;

/**
 * 把历史按**本地日历日**分段：今天 / 昨天 / 近几天各一天（显示「9月22日」）/ 更早。
 * 按日历日而不是按 24 小时切：昨晚 23 点的会话，今早 10 点看也是「昨天」。
 * 段内保持传入的顺序（调用方已经排好）；没有会话的段不出现。
 *
 * 组 id 用于存折叠状态，所以「今天 / 昨天 / 更早」用固定 id，不随日期变；中间那几天用具体日期。
 */
export function dayBuckets<T extends { mtime: number }>(list: T[], now: Date): DayBucket<T>[] {
  const today0 = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const buckets = new Map<string, DayBucket<T>>();
  const order: string[] = [];
  for (const s of list) {
    const d = new Date(s.mtime * 1000);
    const day0 = new Date(d.getFullYear(), d.getMonth(), d.getDate());
    // 用四舍五入消掉夏令时那一小时的差
    const diff = Math.round((today0.getTime() - day0.getTime()) / 86_400_000);
    let id: string, label: string;
    if (diff <= 0) { id = "day-today"; label = "今天"; }
    else if (diff === 1) { id = "day-yesterday"; label = "昨天"; }
    else if (diff < SINGLE_DAYS) {
      id = `day-${day0.getFullYear()}-${day0.getMonth() + 1}-${day0.getDate()}`;
      label = `${day0.getMonth() + 1}月${day0.getDate()}日`;
    } else { id = "day-older"; label = "更早"; }
    let b = buckets.get(id);
    if (!b) { b = { id, label, list: [] }; buckets.set(id, b); order.push(id); }
    b.list.push(s);
  }
  // 按日期新旧排段：今天 → 昨天 → 具体日期（新到旧）→ 更早。不能用首次出现的顺序：
  // 用户按「消息数」排序时列表不按时间，但那种情况调用方就不该分段（见 SessionTree）
  const rank = (id: string) => id === "day-today" ? 0 : id === "day-yesterday" ? 1 : id === "day-older" ? 3 : 2;
  const dayOf = (b: DayBucket<T>) => Math.max(...b.list.map((s) => s.mtime));
  return order.map((id) => buckets.get(id)!).sort((a, b) => rank(a.id) - rank(b.id) || dayOf(b) - dayOf(a));
}

const PRIORITY: Record<RunState, number> = { waiting: 0, busy: 1, idle: 2, stopped: 3 };

/** 项目组内的顺序：需要回应 → 进行中 → 空闲 → 已停止，同级按最近活动。照 Codex 的「优先级」排序 */
export function priorityCompare(a: GroupRow, b: GroupRow): number {
  return PRIORITY[runState(a)] - PRIORITY[runState(b)] || b.mtime - a.mtime;
}

/** 项目组一页显示多少条（#219）。和历史组的 RECENT_PAGE 同一个量级 */
export const PROJECT_PAGE = 50;

/**
 * 项目组分页（#219）：一个项目 250 条会话时，展开就一次渲染全部，实测卡 1 秒以上。
 * `limit` 是这个项目记住的上限（点过「显示更多」），没有就是一页；`mustShow` 是要定位的
 * 会话（⌘L），排在后面时把上限放宽到按页取整后能包住它。渲染和 ↑↓ 导航共用这份结果。
 */
export function pageProject<T extends { session_id: string }>(
  sessions: T[],
  limit: number | undefined,
  mustShow: string | null,
): { shown: T[]; hidden: number } {
  let cap = limit ?? PROJECT_PAGE;
  if (mustShow) {
    const idx = sessions.findIndex((s) => s.session_id === mustShow);
    if (idx >= cap) cap = Math.ceil((idx + 1) / PROJECT_PAGE) * PROJECT_PAGE;
  }
  if (sessions.length <= cap) return { shown: sessions, hidden: 0 };
  return { shown: sessions.slice(0, cap), hidden: sessions.length - cap };
}
