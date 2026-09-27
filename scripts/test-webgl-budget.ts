/**
 * WebGL 上下文预算（#230）。
 *
 * 为什么单独测：WebKit 每页最多 16 个 WebGL 上下文，第 17 个终端开始会把「最近最少用」的终端
 * 挤掉，而我们的回退是永久 DOM —— 多开终端时悄悄变慢（压测 #227 场景 1 证实）。改成自己管预算：
 * 可见的终端一定有 WebGL，超出预算时释放最久没看过的隐藏终端，重新看见时再加载。
 * 错了在 UI 上：可见的终端用着 DOM 渲染（刷屏卡）、或者来回切标签时反复建 / 拆上下文（切换卡）。
 */
import { planWebgl, type GlEntry } from "../src/webglBudget.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}
const e = (id: string, visible: boolean, hasGl: boolean, lastVisibleAt: number): GlEntry => ({ id, visible, hasGl, lastVisibleAt });

eq("预算内：什么都不动",
  planWebgl([e("a", true, true, 10), e("b", false, true, 5)], 3),
  { acquire: [], release: [] });

eq("新终端可见、没 WebGL：给它",
  planWebgl([e("a", true, false, 10)], 3),
  { acquire: ["a"], release: [] });

eq("超预算：释放最久没看过的隐藏终端，给可见的",
  planWebgl([e("old", false, true, 1), e("mid", false, true, 5), e("new", true, false, 9)], 2),
  { acquire: ["new"], release: ["old"] });

eq("隐藏终端不主动加载（懒）",
  planWebgl([e("a", false, false, 3)], 3),
  { acquire: [], release: [] });

eq("被降级过的终端重新看见：加回来",
  planWebgl([e("back", true, false, 20), e("x", false, true, 4)], 2),
  { acquire: ["back"], release: [] });

eq("可见的永远不释放，即使可见的比预算还多（多出来的可见终端不给）",
  planWebgl([e("v1", true, true, 3), e("v2", true, true, 2), e("v3", true, false, 1)], 2),
  { acquire: [], release: [] });

eq("隐藏里保留最近看过的，释放更老的",
  planWebgl([e("h1", false, true, 1), e("h2", false, true, 2), e("h3", false, true, 3), e("v", true, true, 9)], 3),
  { acquire: [], release: ["h1"] });

console.log(`✓ WebGL 上下文预算全部通过（${n} 项）`);
