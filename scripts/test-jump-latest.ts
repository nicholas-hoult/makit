/**
 * 终端「回到最新」按钮的显示判据（#205）。
 *
 * 为什么单独测：按钮该不该出现，只取决于「视口是不是离开了底部」这一件事，但它有两个容易搞错的边界：
 *   1. 贴底时有新输出（`viewportY` 和 `baseY` 一起涨）——**不能**冒出来，否则 Claude 一刷屏按钮就闪；
 *   2. 没有回滚历史（两者都是 0）——也不该出现。
 * 错了在 UI 上就是「按钮一直挂着」或「翻上去了却没有按钮」，而这两种都只有盯着终端才能发现。
 */
import { shouldShowJumpLatest } from "../src/terminalJumpLatest.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  if (got !== want) {
    console.error(`✗ ${name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
    process.exit(1);
  }
}

eq("贴底：不显示", shouldShowJumpLatest(120, 120), false);
eq("往回翻了一行：显示", shouldShowJumpLatest(119, 120), true);
eq("翻到最顶：显示", shouldShowJumpLatest(0, 500), true);
eq("没有回滚历史：不显示", shouldShowJumpLatest(0, 0), false);
eq("贴底时来了新输出（两者一起涨）：不显示", shouldShowJumpLatest(200, 200), false);

console.log(`✓ 回到最新按钮显示判据全部通过（${n} 项）`);
