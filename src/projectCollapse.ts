/**
 * 侧栏「按项目」视图里，一个项目组折没折。
 *
 * 单独一个模块是为了能测：这套编码的两侧存在同一个 Set 里，而 ⌘L 的定位要
 * **反过来强制展开** —— 折叠的组连 <li> 都不渲染，querySelector 拿回 null，
 * scrollIntoView 静默什么也不做，用户看到的就是「⌘L 定位不到，必须自己点开目录」。
 *
 * 编码（一个 Set 存两侧）：
 *   colKey                  = 用户显式折叠过
 *   "__expanded__" + colKey = 用户显式展开过
 * 之所以要两侧，是因为默认值不是常量 —— 组里有活跃 session 时默认展开，否则默认
 * 折叠。只存一侧就没法表达「默认折叠但我手动开了」。
 */

const EXPANDED_PREFIX = "__expanded__";

/// 一个项目组在 collapsed 集合里的键。空 gitRoot 的组（session 没有 git 仓库）
/// 退到组名，和 groupByGitRoot 生成 name 的规则一致。
export function projectCollapseKey(gitRoot: string, name: string): string {
  return gitRoot || name;
}

/// hasActive = 组里有 running / busy / waiting 的进程。它是**默认值**的来源，
/// 和「当前选中哪个 session」无关 —— 所以定位到一个已停止的 session 时，
/// 它所在的组默认就是折叠的。
export function isProjectCollapsed(collapsed: Set<string>, colKey: string, hasActive: boolean): boolean {
  if (collapsed.has(colKey)) return true;
  return !collapsed.has(EXPANDED_PREFIX + colKey) && !hasActive;
}

/// 点表头：在两个显式态之间翻。两侧都要动（删一个加一个），只加不删的话
/// 显式折叠会一直压着显式展开。
export function toggleProjectCollapsed(collapsed: Set<string>, colKey: string, hasActive: boolean): Set<string> {
  const next = new Set(collapsed);
  if (isProjectCollapsed(collapsed, colKey, hasActive)) {
    next.delete(colKey);
    next.add(EXPANDED_PREFIX + colKey);
  } else {
    next.delete(EXPANDED_PREFIX + colKey);
    next.add(colKey);
  }
  return next;
}

/// 右键「只看这个项目」：其余全部折叠，自己展开。
///
/// 这里**该**写显式态（和 ⌘L 的 expandProjectForReveal 相反）：用户是主动说
/// 「我只想看这一个」，把它记成偏好正是他要的；而 ⌘L 只是顺路定位，改偏好属于
/// 越权。同一个数据结构，两条路的正确行为不同，所以是两个函数而不是一个带 flag。
export function collapseOtherProjects(
  collapsed: Set<string>,
  allKeys: string[],
  keepKey: string,
): Set<string> {
  const next = new Set(collapsed);
  for (const k of allKeys) {
    if (k === keepKey) continue;
    next.delete(EXPANDED_PREFIX + k);
    next.add(k);
  }
  next.delete(keepKey);
  next.add(EXPANDED_PREFIX + keepKey);
  return next;
}

/// ⌘L 定位：把目标组强制展开。
///
/// 本来就展开着就原样返回（同一个 Set 引用）：给一个默认展开的组补
/// `__expanded__` 等于把「默认」偷偷改成「用户显式展开」，之后组里进程全停了
/// 它也不会再自动收起来。
export function expandProjectForReveal(collapsed: Set<string>, colKey: string, hasActive: boolean): Set<string> {
  if (!isProjectCollapsed(collapsed, colKey, hasActive)) return collapsed;
  const next = new Set(collapsed);
  next.delete(colKey);
  next.add(EXPANDED_PREFIX + colKey);
  return next;
}
