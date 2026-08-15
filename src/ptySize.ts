/**
 * PTY 尺寸记账本 —— **一个 pane 只有一份**。
 *
 * #156（终端花屏 / 重复行）的根因就在「有几份」上。
 *
 * 花屏的机制是确定的：xterm 比 PTY 窄的时候，程序按 PTY 那个更大的宽度排版，xterm 只能
 * 自动折行 —— 一行变两行。而程序的差分重绘靠「上移我以为的 N 行再重写」定位，它算的 N
 * 比实际少，于是上移不到位，旧行不被清掉、新帧写在下面，屏幕上就积出好几份可读文字。
 * 这个残留**永远不会自愈**，因为它已经是缓冲区里的内容了。
 *
 * 而 makit 有三条路会改尺寸，且日常高频交叉：
 *   · `doFit`（ResizeObserver）  拖分割线 / 缩窗口 / 全屏动画
 *   · `fitAll()`                 分屏 / 关 pane / 放大还原（`WorkspaceView.tsx`）
 *   · `fit(id)`                  切 tab / 切 pane 焦点（`Terminal.tsx`）
 *
 * 旧实现里只有 `doFit` 带去重，用的是它自己闭包里的 `lastCols/lastRows`；另外两条路
 * 改了 PTY 尺寸却不通知它。于是这个序列（全是日常操作）就能造出永久不一致：
 *
 *   1. 拖分割线      doFit：xterm=80、PTY=80，lastCols=80
 *   2. 切个 tab 回来  fit(id)：布局变了，xterm=100、PTY=100 —— **lastCols 还是 80**
 *   3. 再拖回原位     doFit：fit 得到 80，`80 === lastCols` → 判定「没变」，**跳过 pty_resize**
 *   4. PTY 停在 100，xterm 只有 80 → 花屏
 *
 * 所以这里的不变量只有一条，也是全部：
 *
 *   **`knownSize(pane)` 必须恒等于 xterm 当前的 cols×rows。**
 *
 * 一份账才能维持它。另外，发送失败必须回滚 —— 旧实现的 `.catch(() => {})` 把失败吞掉，
 * 记账本却已经记成「发过了」，那是同一个不变量的第二种破法。
 */

export type Size = { cols: number; rows: number };

/** 这次要不要真的发，以及发之前 PTY 知道的是什么（失败时用它回滚）。 */
export type Claim = { send: boolean; prev: Size | null };

const known = new Map<string, Size>();

/**
 * xterm 现在是 `size`，问「需要告诉 PTY 吗」。
 *
 * 需要就顺手记账 —— 记账和判定必须是同一个动作，拆成两步就会有人只做一半。
 */
export function claimResize(paneId: string, size: Size): Claim {
  const prev = known.get(paneId) ?? null;
  if (prev && prev.cols === size.cols && prev.rows === size.rows) {
    return { send: false, prev };
  }
  known.set(paneId, { cols: size.cols, rows: size.rows });
  return { send: true, prev };
}

/**
 * `pty_resize` 没送到，把账退回去。
 *
 * 不退的话这个尺寸就被永久当成「PTY 已经知道」，之后再也不会重发 —— 和跳过一次
 * resize 是完全一样的后果。
 *
 * `sent` 是当初 claim 的那个尺寸：**只有账上还是我记的那一笔时才退**。`pty_resize` 是异步的，
 * 失败回执可能晚于另一条路的成功发送；无条件退账会把新的、正确的那笔盖成旧值，
 * 于是下一次 fit 又被判成「没变」—— 退账本身变成了 #156。
 */
export function revertResize(paneId: string, sent: Size, prev: Size | null): void {
  const cur = known.get(paneId);
  if (!cur || cur.cols !== sent.cols || cur.rows !== sent.rows) return;
  if (prev) known.set(paneId, prev);
  else known.delete(paneId);
}

/** PTY 现在以为自己多大。没告诉过它就是 null。 */
export function knownSize(paneId: string): Size | null {
  return known.get(paneId) ?? null;
}

/** pane 销毁时清掉，否则 paneId 复用时会继承上一条命的账。 */
export function forgetPane(paneId: string): void {
  known.delete(paneId);
}

/** 极窄 pane 的下限。低于这个尺寸的 TUI 会彻底错乱，不如给它一个能用的最小值。 */
export const MIN_COLS = 20;
export const MIN_ROWS = 5;

/**
 * 把 fit 出来的尺寸夹到下限。
 *
 * 关键在**调用方必须把 xterm 一起夹**（`terminal.resize(...)`）。只抬 PTY 那一边就是 #156
 * 本身：50px 宽的 pane 里 xterm 只有 7 列，却告诉 PTY 20 列，程序按 20 排版，xterm 折行，
 * 花屏且不自愈。下限本身没错，错的是让两边拿到不同的数。
 */
export function clampSize(size: Size): Size {
  return { cols: Math.max(MIN_COLS, size.cols), rows: Math.max(MIN_ROWS, size.rows) };
}
