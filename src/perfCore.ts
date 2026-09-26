// 性能埋点的纯逻辑（#218），DOM / IPC 部分在 perf.ts。测试见 scripts/test-perf-core.ts。

export type InteractionKind = "tab" | "session-row" | "sidebar" | "other";

/** 点的是什么：从目标元素往上的 class 链里，最近的一层说了算。 */
export function interactionKind(classChain: string[]): InteractionKind {
  for (const cls of classChain) {
    const names = cls.split(/\s+/);
    if (names.includes("container-tab")) return "tab";
    if (names.includes("tree-session")) return "session-row";
    if (names.some((c) => c === "tree-group-header" || c === "tree-project-header")) return "sidebar";
  }
  return "other";
}

export type FrameStats = { toPaint: number | null; maxFrame: number | null; longFrames: number };

/**
 * 以触发时刻为起点的帧统计。第一帧 = 这次操作的结果被画出来的时刻；最长帧把「触发 → 第一帧」
 * 也算进去（触发时主线程正卡着，这段就是用户感到的卡顿）。一帧都没有（窗口不可见）时不报数。
 */
export function frameStats(start: number, frames: number[], longMs = 100): FrameStats {
  if (frames.length === 0) return { toPaint: null, maxFrame: null, longFrames: 0 };
  let prev = start, maxFrame = 0, longFrames = 0;
  for (const t of frames) {
    const gap = t - prev;
    if (gap > maxFrame) maxFrame = gap;
    if (gap > longMs) longFrames++;
    prev = t;
  }
  return { toPaint: Math.round(frames[0] - start), maxFrame: Math.round(maxFrame), longFrames };
}

export type Stage = { stage: string; at: number; delta: number };

/** 阶段时间线：换算到零点、按时间排序、取整，并给出每段相对上一段的增量。 */
export function stages(zero: number, marks: { stage: string; at: number }[]): Stage[] {
  const sorted = [...marks].sort((a, b) => a.at - b.at);
  let prev = 0;
  return sorted.map((m) => {
    const at = Math.round(m.at - zero);
    const s = { stage: m.stage, at, delta: at - prev };
    prev = at;
    return s;
  });
}
