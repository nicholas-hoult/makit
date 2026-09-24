/**
 * 侧栏「这一帧真的渲染出来了哪些行」。
 *
 * 键盘导航（↑↓、⌘L 定位、←/→ 折叠）全部建立在一个显式的行顺序上，而折叠功能让
 * 「有哪些 session」不再等于「渲染了哪些行」：折叠的组里那些 session 连 <li> 都没有。
 * 渲染侧和导航侧各写一遍组的顺序必然漂 —— 折叠了一组却忘了从顺序里摘掉，表现是 ↑↓
 * 走到一个不存在的行上、选中框凭空消失，而这种 bug 在界面上看不出来源。
 *
 * 所以顺序只在这里算一次，纯函数，可以直接测。组的**内容**由调用方（SessionTree）
 * 传进来，它渲染时用的是同一份数组。
 */

/** 只要 session_id —— 顺序不关心 session 上的其它任何字段。 */
type Row = { session_id: string };

export type OrderGroup = { id: string; list: Row[] };
/**
 * 项目组直接传「折没折」的结论，不传集合：项目组的折叠是双侧编码（还要区分「显式
 * 展开」，因为默认值取决于组里有没有活跃进程），那套算法归 projectCollapse.ts。
 * 在这里再解一遍就是把同一个编码实现两份。
 */
export type OrderProject = { collapsed: boolean; sessions: Row[] };

export type OrderInput = {
  /**
   * 两个视图都常驻在最上面的组，按渲染顺序：置顶、已打开（#187）。
   * 它们不属于任何一种分组方式，视图切换只影响它们下面那部分。
   */
  top: OrderGroup[];
  viewMode: "status" | "project";
  /** 状态视图下的带标签分组，按渲染顺序。 */
  labeled: OrderGroup[];
  /**
   * 状态视图下的历史，**已经切到当前那一页**（visibleRecent）再按日期分好的段（#187）。
   * 传整个 history 是错的：没渲染出来的行不能进导航顺序，否则 ↓ 会走进虚空。
   * 按「最近活动」排序时是「今天 / 昨天 / …」几段，各自可折叠；按别的排序时不分段，
   * 只有一个 id 为 "history"、没有标题的组 —— 它不可折叠（见下）。
   */
  history: OrderGroup[];
  /** 项目视图下的项目组，按渲染顺序。 */
  projects: OrderProject[];
  /** 状态组的折叠集合。单侧编码：在集合里 = 折叠（默认恒为展开，所以不需要第二侧）。 */
  collapsedGroups: Set<string>;
};

export function visibleSessionOrder(o: OrderInput): string[] {
  const ids: string[] = [];
  const push = (g: OrderGroup) => {
    if (o.collapsedGroups.has(g.id)) return;
    for (const s of g.list) ids.push(s.session_id);
  };

  for (const g of o.top) push(g);
  if (o.viewMode === "status") {
    for (const g of o.labeled) push(g);
    for (const g of o.history) {
      // 不分段时那个历史组不可折叠：它没有标签、没有可点的表头，也不该有 —— 它是默认档，
      // 折叠它等于把整列清空。所以它不查 collapsedGroups。按日期分出来的段有表头，照常可折。
      if (g.id === "history") for (const s of g.list) ids.push(s.session_id);
      else push(g);
    }
  } else {
    for (const g of o.projects) {
      if (g.collapsed) continue;
      for (const s of g.sessions) ids.push(s.session_id);
    }
  }
  return ids;
}
