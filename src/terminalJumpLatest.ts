/**
 * 终端「回到最新」浮层按钮（#205）：往回翻历史时右下角浮出，点一下回到底部，贴底自动隐藏。
 *
 * 判据和滚动条显隐（`terminalScrollbar.ts`）用的是同一件事实 —— 视口离没离开底部 —— 但两者的时机不同：
 * 滚动条只在「人正在滚」时显形，按钮要在「人翻上去之后」一直挂着，直到回到底部。
 *
 * 贴底时的新输出会让 `viewportY` 和 `baseY` 一起涨，所以判据必须是两者相比，不能只看「有没有滚动过」。
 */
import type { IDisposable, Terminal } from "@xterm/xterm";

/** 视口离开底部就该显示；贴底（含贴底时有新输出）和没有回滚历史都不显示 */
export function shouldShowJumpLatest(viewportY: number, baseY: number): boolean {
  return viewportY < baseY;
}

/** 给一个终端挂上按钮；`el` 是包住 xterm 的 `.xterm-inner` */
export function installJumpLatest(term: Terminal, el: HTMLElement): IDisposable {
  const btn = document.createElement("button");
  btn.className = "terminal-jump-latest";
  btn.type = "button";
  btn.textContent = "↓ 回到最新";
  btn.title = "回到最新输出";
  // 不抢终端焦点：按下时阻止默认的聚焦行为，点完键盘还在终端里
  btn.addEventListener("mousedown", (e) => e.preventDefault());
  btn.addEventListener("click", () => {
    term.scrollToBottom();
    sync();
  });
  el.appendChild(btn);

  function sync() {
    const b = term.buffer.active;
    btn.classList.toggle("is-visible", shouldShowJumpLatest(b.viewportY, b.baseY));
  }

  // onScroll：人在翻；onWriteParsed：贴底时有新输出会把 baseY 推走，翻上去的状态下也要跟着更新
  const subs = [term.onScroll(sync), term.onWriteParsed(sync)];
  sync();

  return {
    dispose() {
      for (const s of subs) s.dispose();
      btn.remove();
    },
  };
}
