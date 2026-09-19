/**
 * 终端触控板滚动的换算，对标 对标产品（#189）。
 *
 * 对标产品（对标终端 内核）的做法，照抄三件事：
 * 1. 「越用力越快」由 macOS 自己给 —— 系统送来的位移已经按手速加速过，终端不再做非线性加速；
 * 2. 精确滚动（触控板，含惯性阶段）位移 × 2 —— 对标产品 `对标终端TerminalScrollBoost.swift` 的
 *    "historical 2x precise-delta boost"；
 * 3. 像素 → 行：对标终端 `Surface.zig` scrollCallback —— 累积到满一行才滚，截断取整，余数留给下一次，
 *    方向一反丢掉旧余数。
 *
 * 实测 xterm 自己的换算约为 1.25 × 像素 / 行高，只有 对标产品 的六成多，所以精确滚动由我们接管。
 * 离散滚轮（鼠标一格一格）xterm 已经是每格 3 行、和 对标终端 `discrete:3` 一致，不碰。
 *
 * WKWebView 里分辨两种设备（实测）：离散滚轮 `deltaY = 40 × 格`、`wheelDeltaY` 是 120 的倍数；
 * 触控板 `deltaY` 就是像素。恰好 40px 的触控板位移会被当成一格离散 —— 少见，差别也只是这一下按 3 行走。
 */
import type { IDisposable, Terminal } from "@xterm/xterm";

/** 对标产品 对精确滚动的固定放大 */
const PRECISE_BOOST = 2;

export function isDiscreteWheel(deltaY: number, wheelDeltaY: number): boolean {
  return wheelDeltaY !== 0 && wheelDeltaY % 120 === 0;
}

/** 同一串触控板事件的最大间隔：超过就算这一串结束了（触控板 / 惯性事件约 16ms 一个） */
const PRECISE_STREAM_GAP_MS = 250;

/**
 * 单个事件分不清「触控板正好 40px」和「鼠标 1 格」—— WKWebView 里两者读数完全相同
 * （实测注入 40px 的连续位移，被 isDiscreteWheel 判成滚轮，那一串按 xterm 的速度走了）。
 * 靠上下文补：触控板是一串连续事件、几乎总从小位移起手；只要这一串里出现过明确的精确事件，
 * 后面的都算精确，直到停手超过 PRECISE_STREAM_GAP_MS。
 */
export class WheelClassifier {
  private lastPreciseAt = -Infinity;

  isDiscrete(deltaY: number, wheelDeltaY: number, now: number): boolean {
    if (deltaY === 0) return false;
    const inPreciseStream = now - this.lastPreciseAt <= PRECISE_STREAM_GAP_MS;
    if (inPreciseStream || !isDiscreteWheel(deltaY, wheelDeltaY)) {
      this.lastPreciseAt = now;
      return false;
    }
    return true;
  }
}

/**
 * 对标终端 的精确滚动换算。`speed` 对应 对标产品 的「Scroll Speed」设置（默认 1，范围 0.25–3）。
 * 返回这次要滚的行数（正 = 向下看新内容），和留给下一次的余数（像素）。
 */
export function precisePixelsToRows(
  pending: number,
  deltaPx: number,
  cellHeight: number,
  speed = 1,
): { rows: number; pending: number } {
  if (cellHeight <= 0) return { rows: 0, pending: 0 };
  const px = deltaPx * PRECISE_BOOST * speed;
  const carried = pending !== 0 && Math.sign(pending) !== Math.sign(px) ? 0 : pending;
  const total = carried + px;
  const rows = Math.trunc(total / cellHeight);
  return { rows, pending: total - rows * cellHeight };
}

/**
 * 接管精确滚动：只在「普通缓冲区 + 程序没开鼠标上报」时。备用屏幕（vim / less）和鼠标模式下，
 * xterm 要把滚轮转成方向键或鼠标事件发给程序，那是程序的事，交回给它。
 *
 * 为什么不用 `attachCustomWheelEventHandler`：xterm 6 里它只管「滚轮 → 鼠标上报 / 方向键」那条路，
 * 回滚历史的滚动是内部 SmoothScrollableElement **另挂的** wheel 监听，返回 false 拦不住（实测照样按
 * xterm 的速度滚）。所以在外层 `.xterm-inner` 的捕获阶段截住、`stopPropagation` 不让它下去。
 * 同元素上的其他捕获监听（terminalScrollbar 的显隐）不受 stopPropagation 影响。
 */
export function install对标产品Wheel(term: Terminal, el: HTMLElement): IDisposable {
  let pending = 0;
  const classifier = new WheelClassifier();
  const onWheel = (e: WheelEvent) => {
    const wheelDeltaY = (e as WheelEvent & { wheelDeltaY: number }).wheelDeltaY;
    // 用处理时刻而不是 e.timeStamp：后者取自原生事件，合成 / 转发的事件可能全是同一个值
    if (e.deltaY === 0 || classifier.isDiscrete(e.deltaY, wheelDeltaY, performance.now())) {
      pending = 0;
      return;
    }
    if (term.buffer.active.type !== "normal" || term.modes.mouseTrackingMode !== "none") return;
    const screen = el.querySelector<HTMLElement>(".xterm-screen");
    const cellHeight = screen && term.rows > 0 ? screen.clientHeight / term.rows : 0;
    if (cellHeight <= 0) return;
    e.stopPropagation();
    e.preventDefault();
    const r = precisePixelsToRows(pending, e.deltaY, cellHeight);
    pending = r.pending;
    if (r.rows !== 0) term.scrollLines(r.rows);
  };
  el.addEventListener("wheel", onWheel, { capture: true, passive: false });
  return { dispose: () => el.removeEventListener("wheel", onWheel, { capture: true }) };
}
