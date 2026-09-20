/**
 * 改尺寸的行列分离（#203）。改行数便宜（实测 ~1ms），改列数要把整段回滚按新宽度重排
 * （回滚 5000 行时 ~21ms/次，2000 行约 8ms）—— 所以两者不能一视同仁。
 *
 * 行数每帧跟；列数按固定节奏跟（`COLS_FOLLOW_MS`），两次之间不重排。
 * 先做过「只在停手后折一次」（VS Code terminalResizeDebouncer 的做法），实测用户反馈「必须慢慢拖，
 * 不然文字跟不上手」，与对标 对标产品（对标终端 每 25ms 跟一次）不符 —— 改成节奏跟随，并把 scrollback 降到 2000
 * 让单次重排降到跟得动的量级（实测 2 pane 快拖：每帧忙碌中位 15ms，文字最多落后 33ms）。
 */

/** 缓冲区小于这个行数时重排很便宜，行列都立即调（VS Code 同值） */
export const SMALL_BUFFER_LINES = 200;
/** 列数跟随的节奏：拖动中每这么久重排一次（对标终端 是 25ms，这里取一帧多一点，实测跟得动） */
export const COLS_FOLLOW_MS = 33;

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
  now(): number;
  setTimeout(fn: () => void, ms: number): number;
  clearTimeout(id: number): void;
}

/**
 * 列数跟随的节流：`request` 反复调用时，首次立即执行、之后每 ms 一次，停手后补最后一次；
 * `flush`（松手）立即执行并取消待办。首次立即执行是为了起手不延迟；补最后一次是为了「停在哪就折到哪」。
 */
export function createColsFollower(
  fn: () => void,
  ms: number,
  clock: Clock = {
    now: () => performance.now(),
    setTimeout: (f, t) => window.setTimeout(f, t),
    clearTimeout: (id) => window.clearTimeout(id),
  },
) {
  let timer: number | undefined;
  let lastRun = -Infinity;
  let disposed = false;
  const cancel = () => {
    if (timer !== undefined) clock.clearTimeout(timer);
    timer = undefined;
  };
  const run = () => {
    lastRun = clock.now();
    fn();
  };
  return {
    request() {
      if (disposed) return;
      if (clock.now() - lastRun >= ms) {
        cancel();
        run();
        return;
      }
      if (timer !== undefined) return; // 已经排着「补最后一次」
      timer = clock.setTimeout(() => {
        timer = undefined;
        run();
      }, ms - (clock.now() - lastRun));
    },
    flush() {
      if (timer === undefined) return;
      cancel();
      run();
    },
    dispose() {
      disposed = true;
      cancel();
    },
  };
}
