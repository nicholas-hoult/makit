/**
 * #156（终端花屏 / 同一行出现两串可读文字）的**看门狗**。修复在 `ptySize.ts`，这里只负责
 * 证明修复真的守住了 —— 平时关着。
 *
 * 花屏归到同一个量上：「终端认为一行有多宽 / 一屏有多少行」和「跑在里面的程序认为的」
 * 是否一致。不一致，程序的差分重绘（上移 N 行再重写）就落在错的行上：旧行没被清掉、
 * 新帧盖上去，屏幕上就是重复行，而且**永远不会自愈**（它已经是缓冲区里的内容了）。
 *
 * 前三轮都在猜 renderer（WebGL 字形图集），但那条路解释不了「英文也重复」。真正的根因
 * 是 cols/rows 记账：三条 resize 路径（`doFit` / `fit` / `fitAll`）只有 `doFit` 带去重，
 * 而它用的是自己闭包里的 `lastCols/lastRows`，看不见另外两条路也改过 PTY —— 详见 `ptySize.ts`。
 *
 * 另一件已测得的事（`scripts/probe-156.sh`）：xterm 和 Claude Code 的字宽表在 1192 个码位上
 * 不一致（`⏺` `✳` `⚠` 这类）。它方向相反（xterm 比程序窄），只会让右边缘参差，不会造成
 * 重复行 —— 那是 #183，另一个 bug。
 *
 * 所以这里只记数字，一个字符的内容都不碰：
 *   开启  localStorage.setItem("MAKIT_SIZE_TRACE", "1")  然后重载（⌘R）
 *   复现  正常用，尤其是拖分割线 / 缩放窗口 / 进出全屏 / 切 tab
 *   读取  控制台执行 __sizeTrace()          → 全部事件（纯数字）
 *         控制台执行 __size156()            → 一行结论：有没有出现过记账不一致
 *   清空  __sizeTraceClear()
 *
 * 关掉时全部是空函数，不进任何热路径。修复之后 `__size156()` 应当一直是「记账一致」；
 * 哪天它变成「不一致」，就是这条路上又出现了新的第四条 resize 路径。
 */

const MAX_LINES = 600;
const lines: string[] = [];
let firstAt = 0;

/** 每个 pane 最后一次**告诉 PTY**的尺寸。判据的另一半，xterm 那半直接现读。 */
const sentToPty = new Map<string, { cols: number; rows: number }>();
/** 出现过几次「PTY 已确认，但此刻 xterm 的尺寸和它不一样」。这就是花屏的直接证据。 */
let mismatches = 0;

function enabled(): boolean {
  try {
    return localStorage.getItem("MAKIT_SIZE_TRACE") === "1";
  } catch {
    return false;
  }
}

function push(line: string) {
  if (lines.length === 0) firstAt = performance.now();
  const ms = Math.round(performance.now() - firstAt);
  lines.push(`+${String(ms).padStart(6)}ms  ${line}`);
  if (lines.length > MAX_LINES) lines.shift();
}

function installGlobals() {
  const g = globalThis as unknown as Record<string, unknown>;
  if (g.__sizeTrace) return;
  g.__sizeTrace = () => (lines.length ? lines.join("\n") : "(空 —— 还没有事件被记录)");
  g.__sizeTraceClear = () => {
    lines.length = 0;
    mismatches = 0;
  };
  g.__size156 = () =>
    mismatches === 0
      ? `记账一致：${lines.length} 条事件里没有出现过 xterm 与 PTY 尺寸不一致 → 花屏不在 cols/rows 这条路上`
      : `记账不一致 ${mismatches} 次 → 花屏的直接原因就在这里（跑 __sizeTrace() 看是哪一次、差多少）`;
}

/** xterm 自己改了尺寸。同步发生，是「先动的那一边」。 */
export function traceXtermResize(paneId: string, cols: number, rows: number): void {
  if (!enabled()) return;
  installGlobals();
  const s = sentToPty.get(paneId);
  const gap = s ? `PTY 还以为 ${s.cols}x${s.rows}` : "PTY 尚未被告知过";
  push(`${paneId.slice(0, 6)} xterm.resize   → ${cols}x${rows}   （${gap}）`);
}

/**
 * 发出一次 `pty_resize`。`site` 用来区分三条 resize 路径（doFit / fit / fitAll），
 * 因为「哪条路漏了」比「差了几列」更能定位。
 */
export function traceResizeSent(paneId: string, site: string, cols: number, rows: number): void {
  if (!enabled()) return;
  installGlobals();
  sentToPty.set(paneId, { cols, rows });
  push(`${paneId.slice(0, 6)} pty_resize→    ${cols}x${rows}   [${site}]`);
}

/**
 * `pty_resize` 的回执。**判据在这一刻**：PTY 已经知道了新尺寸，此时 xterm 若和它不一样，
 * 就是一次真实的、会留疤的记账不一致（异步窗口期里又 fit 过一次，或者这次 invoke 失败了）。
 */
export function traceResizeAck(
  paneId: string,
  site: string,
  sent: { cols: number; rows: number },
  now: { cols: number; rows: number },
  err?: unknown,
): void {
  if (!enabled()) return;
  installGlobals();
  if (err !== undefined) {
    // 还没 spawn 的 pane 被拒是预期的，不算证据；其余的拒绝都意味着 PTY 从没学到这个尺寸
    if (!site.includes("未spawn")) mismatches++;
    push(`${paneId.slice(0, 6)} pty_resize ✗   ${sent.cols}x${sent.rows} 被拒 [${site}] err=${String(err).slice(0, 60)}`);
    return;
  }
  const bad = now.cols !== sent.cols || now.rows !== sent.rows;
  if (bad) mismatches++;
  push(
    `${paneId.slice(0, 6)} pty_resize ✓   PTY=${sent.cols}x${sent.rows}  xterm=${now.cols}x${now.rows}  [${site}]` +
      (bad ? `  ✗ 不一致 Δcols=${now.cols - sent.cols} Δrows=${now.rows - sent.rows}` : ""),
  );
}
