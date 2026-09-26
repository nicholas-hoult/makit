/**
 * 性能埋点的纯逻辑（#218）：点的是什么、帧统计、阶段时间线。
 *
 * 为什么单独测：
 * - 埋点是给打包版查问题用的，数字错了不会有任何界面症状 —— 只会在某天查「为什么卡」时
 *   拿着一份错的日志得出错的结论。#216 里就踩过：拿文件缓存已热的 523ms 当成冷启动的数。
 * - 「点击后多久画出来」「期间最长卡了多久」的口径必须钉死：以点击时刻为起点，
 *   第一帧就是这次点击的结果被画出来的时刻；最长帧要把「点击 → 第一帧」这段也算进去
 *   （点击时主线程正卡着，这段就是用户感到的卡顿）。
 * - 点击目标按 class 往上找，标签页里点到的是图标 / 文字 span，也要认成「标签页」。
 */
import { interactionKind, frameStats, stages } from "../src/perfCore.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}

// ── 点的是什么：从目标元素往上的 class 链 ──
eq("标签页本身", interactionKind(["container-tab active", "container-tabs"]), "tab");
eq("标签页里的文字", interactionKind(["container-tab-label", "container-tab active", "container-tabs"]), "tab");
eq("标签栏空白处不算标签", interactionKind(["container-tabs", "container-tab-bar"]), "other");
eq("侧栏会话行里的状态点", interactionKind(["tree-status-icon busy", "tree-session busy focused", "tree-body"]), "session-row");
eq("侧栏分组标题", interactionKind(["tree-group-arrow open", "tree-group-header", "tree-body"]), "sidebar");
eq("侧栏项目标题", interactionKind(["tree-project-arrow", "tree-project-header", "tree-body"]), "sidebar");
eq("终端里点一下", interactionKind(["xterm-screen", "xterm", "container-terminal"]), "other");
eq("最近的那层说了算：会话行里的按钮仍是会话行", interactionKind(["tree-session-content", "tree-session idle", "tree-body"]), "session-row");

// ── 帧统计：起点 = 点击时刻 ──
eq("正常：16ms 后出第一帧，之后平稳",
  frameStats(1000, [1016, 1033, 1050]),
  { toPaint: 16, maxFrame: 17, longFrames: 0 });
eq("点击时主线程卡着：第一帧 250ms 后才来，这段算进最长帧",
  frameStats(1000, [1250, 1266, 1283]),
  { toPaint: 250, maxFrame: 250, longFrames: 1 });
eq("中间卡了一下",
  frameStats(0, [16, 33, 180, 196]),
  { toPaint: 16, maxFrame: 147, longFrames: 1 });
eq("阈值可调（默认 100ms，恰好 100 不算）",
  frameStats(0, [100, 216], 100),
  { toPaint: 100, maxFrame: 116, longFrames: 1 });
eq("一帧都没有（窗口不可见）：不瞎报",
  frameStats(0, []),
  { toPaint: null, maxFrame: null, longFrames: 0 });

// ── 阶段时间线：以某个零点换算，按时间排序，取整 ──
eq("换算 + 排序 + 每段增量",
  stages(1000, [
    { stage: "JS 开始", at: 2960.4 },
    { stage: "窗口创建", at: 2071.6 },
    { stage: "侧栏有数据", at: 3238 },
  ]),
  [
    { stage: "窗口创建", at: 1072, delta: 1072 },
    { stage: "JS 开始", at: 1960, delta: 888 },
    { stage: "侧栏有数据", at: 2238, delta: 278 },
  ]);
eq("空列表", stages(0, []), []);

console.log(`✓ 性能埋点纯逻辑全部通过（${n} 项）`);
