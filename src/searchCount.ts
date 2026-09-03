/**
 * ⌘F 终端内搜索的计数文本（「3/17」）。
 *
 * 数据来自 xterm search addon 的 `onDidChangeResults`，两个字段都有坑：
 *
 * - **`resultIndex` 是 0-based**，直接显示就是「0/17」这种差一错误。
 * - **匹配数超过高亮上限时 `resultIndex` 是 -1**（`highlightLimit` 默认 1000）。
 *   这时 addon 自己也不知道当前停在第几个，所以只能报总数。
 *
 * 抽成独立模块是为了能测这几个边界 —— 它们都不是「显示得好不好看」的问题，
 * 而是会不会给出一个错误的数字。
 */

/** `onDidChangeResults` 的负载，改名以免和 xterm 的类型耦合 */
export type SearchProgress = { index: number; count: number };

/**
 * @param term 当前搜索词。为空时返回 null —— 刚按 ⌘F 打开搜索条、还没输入时
 *   不该先闪一个「无结果」出来。
 * @param p 最近一次结果事件；还没收到过时传 null。
 */
export function formatSearchCount(term: string, p: SearchProgress | null): string | null {
  if (!term.trim()) return null;
  if (!p) return null;
  // count===0 必须排在 index<0 前面：没有匹配时 addon 给的是
  // { resultIndex: -1, resultCount: 0 }，两个条件同时成立。
  if (p.count === 0) return "无结果";
  // index 拿不到（超出高亮上限）或越界时，只报总数，不编一个位置出来。
  if (p.index < 0 || p.index >= p.count) return `${p.count} 个结果`;
  return `${p.index + 1}/${p.count}`;
}
