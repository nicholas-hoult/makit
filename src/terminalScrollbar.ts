/**
 * 终端滚动条：只在**人在翻**的时候显形，停手 idleMs 后淡出 —— 与侧栏等处的
 * `scrollActivity.ts` 同一套时序（#188）。
 *
 * 为什么不能复用 scrollActivity：xterm 6 的滚动是它自己的 SmoothScrollableElement
 * （从 VS Code 移植），视口是虚拟的，**不产生 DOM scroll 事件**，document 捕获监听收不到。
 * 滚动条也不是 `::-webkit-scrollbar`，而是 `.xterm-scrollable-element > .scrollbar` 这层 DOM。
 *
 * 为什么要压掉 xterm 自己的显隐：它用的是 VS Code 的 Auto 可见性 ——「指针在终端上就一直显示」。
 * 终端是一直被指着的地方，于是条子等于常驻；而拖分屏 / 拖文件时 `.xterm-inner` 会被置
 * `pointer-events: none`，mouseleave 可能收不到，条子还会卡在显示态。CSS 那边
 * （App.css `.xterm-inner:not(.is-scrolling)`）在没滚的时候强制藏起来，滚的时候再交回
 * xterm 自己判断（没有回滚内容时它本来就不画）。
 *
 * 「人在翻」的判据：滚轮落在终端上，或视口离开了底部（键盘翻页 / 拖滑块）。输出跟随
 * 底部的那种 onScroll **不算** —— 否则 Claude 一刷屏条子就一直闪。
 *
 * 滑块长度封顶 1/4 轨道：xterm 的长度 = 可见行 / 总行数，新开的 pane 回滚少，滑块能占大半条
 * 轨道。xterm 没有这个选项，只能在它写完 inline style 后改写（`watchSlider`）。代价：按住
 * 滑块拖时，xterm 按它自己的长度换算鼠标位移，截短后滑块会比指针走得略快；滚轮 / 触控板
 * 不受影响。
 */
import type { IDisposable, Terminal } from "@xterm/xterm";

/** 视口不在底部 = 有人在看回滚；贴底的 onScroll 是新输出把视口往下带 */
export function isUserScroll(viewportY: number, baseY: number): boolean {
  return viewportY < baseY;
}

/**
 * 滚动条那一条命中区的宽度（#206）：指针落在终端右边缘这么宽的一条里就让滚动条显形，
 * 这样不用先滚一下也能直接抓住滑块拖。取 14px —— xterm 的 overlay 条宽 ~10px，留一点余量好按。
 */
export const GUTTER_PX = 14;

/** 指针在不在滚动条那一条里。`x` 是指针的 clientX，`right` 是终端的右边缘 */
export function isInScrollbarGutter(x: number, right: number): boolean {
  return x > right - GUTTER_PX && x <= right;
}

/** xterm（VS Code ScrollbarState）自己的滑块最小长度，封顶时不能比它还短 */
const MIN_SLIDER = 20;

/**
 * 把 xterm 算出的滑块（长度 size、顶端 top）封顶到轨道的 maxRatio，并按比例重映射位置：
 * xterm 的 top ∈ [0, track - size]，截短后映射到 [0, track - capped]，顶贴顶、底贴底。
 */
export function capSlider(track: number, size: number, top: number, maxRatio = 0.25): { size: number; top: number } {
  if (track <= 0) return { size, top };
  const cap = Math.max(MIN_SLIDER, Math.floor(track * maxRatio));
  if (size <= cap) return { size, top };
  const range = track - size;
  const ratio = range > 0 ? top / range : 0;
  return { size: cap, top: Math.round(ratio * (track - cap)) };
}

/**
 * 盯住 xterm 的垂直滑块：xterm 每次写 height / top，按 capSlider 改写。
 * xterm 的 FastDomNode 只在它自己的值变了才写 DOM，所以改写后的值会一直留着，直到它下一次
 * 真的变化 —— 这时 MutationObserver 收到的是 xterm 的新值，重新算一遍即可。区分「xterm 写的」
 * 和「我们刚写的」靠记住上一次写入的值。
 */
function watchSlider(root: HTMLElement): () => void {
  const slider = root.querySelector<HTMLElement>(".xterm-scrollable-element > .xterm-scrollbar.xterm-vertical > .xterm-slider");
  const track = slider?.parentElement;
  if (!slider || !track) return () => {};
  let xtermSize = 0, xtermTop = 0;   // xterm 想要的
  let ourSize = -1, ourTop = -1;      // 我们上次写的（数值，-1 = 还没写过）
  const px = (v: string) => parseFloat(v) || 0;
  // 比数值而不是比字符串：浏览器会把 transform 重新序列化（空格、单位写法都可能变），
  // 拿我们写进去的字符串去比对读回来的，永远不相等 —— MutationObserver 会被自己的写入喂成死循环。
  const same = (a: number, b: number) => Math.abs(a - b) < 0.5;
  const apply = () => {
    const h = px(slider.style.height);
    const t = px(slider.style.top);
    if (!same(h, ourSize)) xtermSize = h;   // 不是我们写的 → 就是 xterm 写的
    if (!same(t, ourTop)) xtermTop = t;
    const c = capSlider(track.clientHeight, xtermSize, xtermTop);
    ourSize = c.size;
    ourTop = c.top;
    if (!same(h, c.size)) slider.style.height = `${c.size}px`;
    if (!same(t, c.top)) slider.style.top = `${c.top}px`;
  };
  const mo = new MutationObserver(apply);
  mo.observe(slider, { attributes: true, attributeFilter: ["style"] });
  apply();
  return () => mo.disconnect();
}

interface Clock {
  setTimeout(fn: () => void, ms: number): number;
  clearTimeout(id: number): void;
}

/**
 * 边沿触发的活动计时器：第一次 ping → show；之后的 ping 只续期；停手 idleMs → hide。
 * 时钟可注入，便于单测。
 */
export function createActivity(
  show: () => void,
  hide: () => void,
  idleMs: number,
  clock: Clock = { setTimeout: (f, ms) => window.setTimeout(f, ms), clearTimeout: (id) => window.clearTimeout(id) },
) {
  let timer: number | undefined;
  let disposed = false;
  return {
    ping() {
      if (disposed) return;
      if (timer === undefined) show();
      else clock.clearTimeout(timer);
      timer = clock.setTimeout(() => {
        timer = undefined;
        hide();
      }, idleMs);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      if (timer !== undefined) {
        clock.clearTimeout(timer);
        timer = undefined;
        hide();
      }
    },
  };
}

/** 给一个终端挂上显隐时序：`el` 是包住 xterm 的 `.xterm-inner` */
export function installTerminalScrollbar(term: Terminal, el: HTMLElement, idleMs = 900): IDisposable {
  const activity = createActivity(
    () => el.classList.add("is-scrolling"),
    () => el.classList.remove("is-scrolling"),
    idleMs,
  );
  const onWheel = () => activity.ping();
  el.addEventListener("wheel", onWheel, { capture: true, passive: true });

  // 指针移到右边缘那一条 → 显形并可抓（#206）。用 JS 判定而不是 CSS :hover：
  // :hover 只由真实指针驱动，自动化测试里派发的事件不会触发它，等于没法验证。
  const onPointerMove = (e: PointerEvent) => {
    const inGutter = isInScrollbarGutter(e.clientX, el.getBoundingClientRect().right);
    el.classList.toggle("gutter-hover", inGutter);
  };
  const onPointerLeave = () => el.classList.remove("gutter-hover");
  el.addEventListener("pointermove", onPointerMove, { passive: true });
  el.addEventListener("pointerleave", onPointerLeave, { passive: true });
  const sub = term.onScroll(() => {
    const b = term.buffer.active;
    if (isUserScroll(b.viewportY, b.baseY)) activity.ping();
  });
  const unwatch = watchSlider(el);
  return {
    dispose() {
      el.removeEventListener("wheel", onWheel, { capture: true });
      el.removeEventListener("pointermove", onPointerMove);
      el.removeEventListener("pointerleave", onPointerLeave);
      el.classList.remove("gutter-hover");
      sub.dispose();
      unwatch();
      activity.dispose();
    },
  };
}
