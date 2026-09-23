/**
 * 容器尺寸 → 行列数（#111）。
 *
 * 为什么单独测：这段是从 `FitAddon.proposeDimensions()` 抄过来**只删一项**的 —— 上游无条件给
 * 滚动条扣 16px，而我们的滚动条是 overlay（不占布局宽度），那一扣就是每个终端白少 2 列。
 * 抄错边界（floor 还是 round、下限夹在哪、padding 扣几次）在 UI 上的表现是「终端右边多一条空白」
 * 或「最后一列被截掉半个字」，都要盯着看才发现；而算出 0 或负数会直接让 PTY 拿到荒唐的尺寸（#156 花屏）。
 */
import { proposeGeometry } from "../src/resizePlan.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}

// 实测基准：720px 容器、13px ui-monospace、cell 8.037×16 —— 上游会给出 87 列，我们要 89
eq("实测基准：720px / cell 8.037 → 89 列（上游是 87）",
   proposeGeometry(720, 400, 0, 0, 8.037, 16), { cols: 89, rows: 25 });

eq("整除时不多不少", proposeGeometry(800, 400, 0, 0, 8, 16), { cols: 100, rows: 25 });
eq("除不尽向下取整（宁可留白也不能截字）", proposeGeometry(799, 399, 0, 0, 8, 16), { cols: 99, rows: 24 });
eq("padding 从可用宽度里扣", proposeGeometry(800, 400, 16, 8, 8, 16), { cols: 98, rows: 24 });
eq("窄到放不下一列：夹到下限 2", proposeGeometry(5, 400, 0, 0, 8, 16), { cols: 2, rows: 25 });
eq("矮到放不下一行：夹到下限 1", proposeGeometry(800, 5, 0, 0, 8, 16), { cols: 100, rows: 1 });
eq("padding 比容器还大：不出负数", proposeGeometry(20, 20, 40, 40, 8, 16), { cols: 2, rows: 1 });
eq("容器为 0（还没布局）：不出 NaN", proposeGeometry(0, 0, 0, 0, 8, 16), { cols: 2, rows: 1 });

console.log(`✓ 容器尺寸换算行列全部通过（${n} 项）`);
