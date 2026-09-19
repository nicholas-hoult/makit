/**
 * 终端滚轮换算，照 对标终端 的做法（#189）。
 *
 * 为什么单独测：
 *
 * 1. `isDiscreteWheel` / `WheelClassifier` 决定这一下归谁管：离散滚轮（鼠标一格一格）交回 xterm（已经是每格 3 行，和 对标终端 一致），
 *    触控板精确滚动我们自己换算。判错了，鼠标滚轮会突然快一倍，或者触控板仍然慢 —— 手感上一眼就能感觉到，
 *    但在 UI 上没法断言。
 *
 * 2. `precisePixelsToRows` 照 对标终端 `Surface.zig` 的 scrollCallback：像素 × 倍率先累积，
 *    满一行才滚、截断取整、余数留给下一次；方向一反就丢掉旧余数。慢慢推时一格格走、不丢也不多，
 *    全靠这几条边界。
 */
import { WheelClassifier, isDiscreteWheel, precisePixelsToRows } from "../src/terminalWheel.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    console.error(`✗ ${name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
    process.exit(1);
  }
}

// ── isDiscreteWheel：WKWebView 实测 —— 离散 deltaY=40×格、wheelDeltaY=-120×格；触控板 deltaY=像素 ──
eq("鼠标 1 格", isDiscreteWheel(40, -120), true);
eq("鼠标加速后 5 格", isDiscreteWheel(200, -600), true);
eq("鼠标向上 1 格", isDiscreteWheel(-40, 120), true);
eq("触控板 7px", isDiscreteWheel(7, -21), false);
eq("触控板 63px", isDiscreteWheel(63, -189), false);
eq("触控板惯性 12px", isDiscreteWheel(12, -36), false);
eq("没有纵向位移（横滑）", isDiscreteWheel(0, 0), false);

// ── WheelClassifier：单个事件分不清「触控板正好 40px」和「鼠标 1 格」（WKWebView 里读数完全相同），
//    靠上下文：触控板是一串连续事件、几乎总从小位移开始；250ms 内见过明确的精确事件，就把这一串都算精确 ──
{
  const c = new WheelClassifier();
  eq("独立的一格滚轮 → 离散", c.isDiscrete(40, -120, 1000), true);
  eq("隔一会再一格 → 仍离散", c.isDiscrete(40, -120, 1500), true);
  eq("触控板起手小位移 → 精确", c.isDiscrete(6, -18, 3000), false);
  eq("同一串里正好 40px → 仍精确", c.isDiscrete(40, -120, 3016), false);
  eq("同一串里 80px → 仍精确", c.isDiscrete(80, -240, 3032), false);
  eq("串内一直续期（每 16ms）→ 仍精确", c.isDiscrete(40, -120, 3250), false);
  eq("停手超过 250ms 后的一格 → 离散", c.isDiscrete(40, -120, 3600), true);
  eq("横滑 / 0 位移 → 不算离散", c.isDiscrete(0, 0, 3700), false);
}

// ── precisePixelsToRows：行高 17，精确滚动放大 2 倍，speed 1 ──
const H = 17;
eq("不满一行：不滚，全留作余数", precisePixelsToRows(0, 5, H), { rows: 0, pending: 10 });
eq("余数累积到满一行才滚", precisePixelsToRows(10, 5, H), { rows: 1, pending: 3 });
eq("一次多行：截断取整，余数保留", precisePixelsToRows(0, 40, H), { rows: 4, pending: 12 });
eq("向上（负数）对称", precisePixelsToRows(0, -40, H), { rows: -4, pending: -12 });
eq("方向反转：丢掉旧余数", precisePixelsToRows(12, -5, H), { rows: 0, pending: -10 });
eq("840px 快划 = 98 行", precisePixelsToRows(0, 840, H).rows, 98);
eq("speed 0.5 抵消 boost = 1:1", precisePixelsToRows(0, 34, H, 0.5), { rows: 2, pending: 0 });
eq("行高未知（0）：不滚", precisePixelsToRows(0, 40, 0), { rows: 0, pending: 0 });

console.log(`✓ 终端滚轮换算全部通过（${n} 项）`);
