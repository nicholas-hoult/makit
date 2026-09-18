/**
 * 全局：任何元素被滚动时给它挂上 `is-scrolling`，停手 `idleMs` 后摘掉 —— macOS
 * overlay 滚动条的时序。哪些容器真的画细条由 CSS 那份选择器列表决定（见 App.css
 * 里 `.tree-body::-webkit-scrollbar` 那段），这里只负责"有没有在滚"这一个事实。
 *
 * 为什么非 JS 不可：CSS 里唯一能表达"有人在用这个面板"的是 `:hover`，而 hover 回答
 * 的是"指针在不在上面"，不是"有没有在滚"。之前那版就是 hover 驱动的，于是在面板里
 * 操作时鼠标一直在面板内、条子一直亮着，等于没隐藏。
 *
 * 为什么一个监听能覆盖全部容器：scroll 事件**不冒泡**，但**捕获阶段**照样会经过
 * document。于是这一个监听就能收到页面上任何滚动容器的 scroll，包括之后才挂载的
 * （命令面板、通知抽屉、右键菜单都是用完就卸载的）—— 逐个组件挂 hook 的话，每加
 * 一个可滚动面板都要记得补一次，漏掉就是"这里的条子又不会自己藏"。
 *
 * 直接改 classList 而不走 state：纯装饰，不值得每次 scroll 触发一轮 React 渲染。
 * 重复 add 同一个 token 是 no-op（不写属性、不引起 invalidation），所以终端输出
 * 刷屏时这里也不会一帧一帧地重算样式。
 */
export function installScrollActivity(idleMs = 900): () => void {
  // 用 WeakMap 存定时器：元素卸载后自己就被回收，不需要在任何地方注销
  const timers = new WeakMap<Element, number>();

  function onScroll(e: Event) {
    const el = e.target;
    if (!(el instanceof HTMLElement)) return; // document / window 的 scroll 不管
    el.classList.add("is-scrolling");
    const prev = timers.get(el);
    if (prev !== undefined) clearTimeout(prev);
    timers.set(
      el,
      window.setTimeout(() => {
        el.classList.remove("is-scrolling");
        timers.delete(el);
      }, idleMs),
    );
  }

  document.addEventListener("scroll", onScroll, { capture: true, passive: true });
  return () => document.removeEventListener("scroll", onScroll, { capture: true });
}
