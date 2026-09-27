// WebGL 上下文预算（#230）。测试见 scripts/test-webgl-budget.ts。
//
// WebKit 每页最多 16 个 WebGL 上下文；超出后浏览器按「最近最少用」挤掉别的终端，而 xterm 的回退是
// 永久 DOM。这里自己管：可见的终端一定有 WebGL；预算不够时释放最久没看过的隐藏终端；
// 隐藏终端不主动加载（懒），重新看见时再给。预算留余量（见 TerminalManager 的 WEBGL_BUDGET）。

export type GlEntry = { id: string; visible: boolean; hasGl: boolean; lastVisibleAt: number };

export function planWebgl(entries: GlEntry[], limit: number): { acquire: string[]; release: string[] } {
  const byRecent = (a: GlEntry, b: GlEntry) => b.lastVisibleAt - a.lastVisibleAt;
  // 优先级：可见的（最近看的在前）→ 已经有 WebGL 的隐藏终端（最近看的在前）
  const order = [
    ...entries.filter((x) => x.visible).sort(byRecent),
    ...entries.filter((x) => !x.visible && x.hasGl).sort(byRecent),
  ];
  const keep = new Set(order.slice(0, limit).map((x) => x.id));
  return {
    acquire: entries.filter((x) => x.visible && !x.hasGl && keep.has(x.id)).map((x) => x.id),
    // 可见的永远不释放（即使可见的比预算还多：多出来的只是不给新的）
    release: entries.filter((x) => x.hasGl && !x.visible && !keep.has(x.id)).map((x) => x.id),
  };
}
