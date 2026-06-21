import type { Terminal } from "@xterm/xterm";

type TerminalWithInput = Pick<Terminal, "input" | "textarea">;
type TerminalWithTextarea = Pick<Terminal, "textarea">;

// macOS WKWebView 下中文/日文 IME 输入会出现重复字符（敲一下出 2 个）。
// 根因：xterm.js 内部 CompositionHelper 在 WebKit 下偶尔把
// insertCompositionText 的中间状态也漏给 onData，叠加 compositionend 的最终字符 → 重复。
// 修法：自己监听 compositionstart/compositionend，期间立 isComposing 标志；
// 外部 onData 回调读这个标志，true 时丢弃，仅放行 compositionend 之后的最终字符。
export function attachIMECompositionGate(term: TerminalWithTextarea): {
  isComposing: () => boolean;
  detach: () => void;
} {
  const ta = term.textarea;
  if (!ta) return { isComposing: () => false, detach: () => {} };

  let composing = false;
  const onStart = () => { composing = true; };
  // compositionend 同步触发，xterm 内部在此之后才把最终字符 trigger 到 onData，
  // 所以 isComposing 必须 *在* xterm 处理前置 false → 用 setTimeout(0) 反而错。
  // 实测：compositionend handler 同步设 false 即可，xterm 的最终 onData 能正常通过。
  const onEnd = () => { composing = false; };

  ta.addEventListener("compositionstart", onStart);
  ta.addEventListener("compositionend", onEnd);
  return {
    isComposing: () => composing,
    detach: () => {
      ta.removeEventListener("compositionstart", onStart);
      ta.removeEventListener("compositionend", onEnd);
    },
  };
}

function getPrintableSymbolInput(data: string | null): string | null {
  if (data === null || data.length === 0) return null;
  if (data.length > 8) return null;
  if (!/^[\p{P}\p{S}]+$/u.test(data)) return null;
  return data;
}

function isSymbolInputType(inputType: string): boolean {
  return inputType === "insertText" || inputType === "insertCompositionText";
}

// macOS WebView IME workaround：
// 中英二合一输入法按 Shift 切 EN/CN 时，第一个符号只走 beforeinput
// 不走 keydown，xterm.onData 听不到 → 用户要敲 2 次符号才出
// 双层监听：keydown 标记自己已处理；beforeinput 兜底手动 term.input(data) 转发
// 参考实现：hanshuaikang/nezha src/components/terminalInputFix.ts
export function attachMacShiftSymbolFix(term: TerminalWithInput): () => void {
  if (typeof navigator === "undefined") return () => {};
  const isMacWebKit =
    /Mac/i.test(navigator.platform || "") &&
    /AppleWebKit/i.test(navigator.userAgent);
  if (!isMacWebKit || !term.textarea) return () => {};

  const textarea = term.textarea;
  let keydownHandledByXterm: string | null = null;

  const handleKeyDown = (event: KeyboardEvent) => {
    keydownHandledByXterm = null;
    if (
      event.keyCode !== 229 &&
      event.shiftKey &&
      !event.ctrlKey &&
      !event.altKey &&
      !event.metaKey &&
      getPrintableSymbolInput(event.key) !== null
    ) {
      keydownHandledByXterm = event.key;
    }
  };

  const handleBeforeInput = (event: InputEvent) => {
    const symbol = getPrintableSymbolInput(event.data);
    if (!isSymbolInputType(event.inputType) || symbol === null) return;
    // keydown 已经把符号交给 xterm，跳过避免双发
    if (keydownHandledByXterm === symbol) {
      keydownHandledByXterm = null;
      return;
    }
    term.input(symbol);
    event.preventDefault();
  };

  textarea.addEventListener("keydown", handleKeyDown);
  textarea.addEventListener("beforeinput", handleBeforeInput);

  return () => {
    textarea.removeEventListener("keydown", handleKeyDown);
    textarea.removeEventListener("beforeinput", handleBeforeInput);
  };
}
