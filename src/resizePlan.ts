/**
 * 改尺寸的行列分离（#203）。照 VS Code 的 terminalResizeDebouncer（同为 xterm.js）：
 * 改行数便宜（实测 ~1ms），改列数要把整段回滚按新宽度重排（5000 行 ~21ms，4 个 pane 叠加每帧近 50ms）。
 * 所以拖动中行数立即跟、列数延后到停手（或松手）再重排一次 —— 拖动不再一顿一顿，也不再每 200ms 跳一次。
 * 代价：拖宽度时内容暂不折行（变窄右侧暂时被裁、变宽右侧暂时留空），停手后一次到位。
 */

/** 缓冲区小于这个行数时重排很便宜，行列都立即调（VS Code 同值） */
export const SMALL_BUFFER_LINES = 200;
/** 列数的尾随防抖：停手这么久后重排（VS Code 同值） */
export const COLS_DEBOUNCE_MS = 100;

interface Size {
  cols: number;
  rows: number;
}

/**
 * 这一次改尺寸做什么。行数变了总是立即生效（便宜）；列数变了，小缓冲区或 `immediate`
 * （一次性的改动：松手、缩放字号、最大化）立即，否则延后。
 */
export function planResize(
  current: Size,
  next: Size,
  bufferLength: number,
  immediate: boolean,
): { rowsNow: boolean; colsNow: boolean; colsLater: boolean } {
  const rowsChanged = next.rows !== current.rows;
  const colsChanged = next.cols !== current.cols;
  const colsNow = colsChanged && (immediate || bufferLength < SMALL_BUFFER_LINES);
  return {
    rowsNow: rowsChanged,
    colsNow,
    colsLater: colsChanged && !colsNow,
  };
}

interface Clock {
  setTimeout(fn: () => void, ms: number): number;
  clearTimeout(id: number): void;
}

/** 尾随防抖：schedule 反复调用只在最后一次之后 ms 执行一次；flush 立即执行待办 */
export function createTrailingDebounce(
  fn: () => void,
  ms: number,
  clock: Clock = { setTimeout: (f, t) => window.setTimeout(f, t), clearTimeout: (id) => window.clearTimeout(id) },
) {
  let timer: number | undefined;
  let disposed = false;
  const cancel = () => {
    if (timer !== undefined) clock.clearTimeout(timer);
    timer = undefined;
  };
  return {
    schedule() {
      if (disposed) return;
      cancel();
      timer = clock.setTimeout(() => {
        timer = undefined;
        fn();
      }, ms);
    },
    flush() {
      if (timer === undefined) return;
      cancel();
      fn();
    },
    dispose() {
      disposed = true;
      cancel();
    },
  };
}
