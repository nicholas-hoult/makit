/**
 * #137（中英输入法快切重复输入）的**取证工具，不是修复**。
 *
 * 为什么必须先取证：一个字符从按键到 PTY 要穿过四层 —— 原生 IME → WKWebView 事件 →
 * xterm 的 capture 监听 → 我们的 bubble 监听（`terminalInputFix.ts`）→ `onData` 的
 * composition gate。读源码能列出好几条都能解释「重复」的路径，但分不出哪条真的发生：
 *
 *   A. `CompositionHelper.ts:110-115` —— IME 激活时 keydown 的 keyCode 是 229，
 *      xterm 提前 return **且不 preventDefault**，同时挂一个 setTimeout diff textarea
 *      再发一次（`:198`）。会不会重复，取决于我们的 preventDefault 有没有挡住 textarea 变化。
 *   B. `CompositionHelper.ts:154-176` —— compositionend 的最终字符在 setTimeout(0) 里发，
 *      靠 `_dataAlreadySent`（`:161`）扣掉 keydown 已发的部分。而我们的 `term.input()`
 *      绕过了 xterm，`_dataAlreadySent` 不会被更新。
 *   C. `terminalInputFix.ts:80` 的 flag 去重按**值**比较，IME 吞掉 keydown 时 flag 会变旧。
 *
 * 判别它们只需要三个字段，而三个都只能在真机敲键时读到：
 *   1. `beforeinput` 的 `cancelable` —— 若 `insertCompositionText` 不可取消，
 *      `terminalInputFix.ts:85` 的 preventDefault 就是空操作（A 成立）。
 *   2. `input` 事件到底有没有发生 —— 它证明 preventDefault 有没有生效。
 *   3. 每一次 `onData` 的实际载荷 —— 重复发生在 DOM 层还是 onData 层。
 *
 * 用法：
 *   开启  localStorage.setItem("MAKIT_IME_TRACE", "1")  然后重载（⌘R）
 *   复现  切到中文输入法，快速切几次，敲出重复的那个字符
 *   读取  控制台执行 __imeTrace()，得到一段可直接粘贴的文本
 *   清空  __imeTraceClear()
 *
 * 关掉时全部是空函数，不进任何热路径。
 */

const MAX_LINES = 400;
const lines: string[] = [];
let firstAt = 0;

function enabled(): boolean {
  // localStorage 在某些沙箱下取值会抛，取不到就当没开
  try {
    return localStorage.getItem("MAKIT_IME_TRACE") === "1";
  } catch {
    return false;
  }
}

function push(line: string) {
  if (lines.length === 0) firstAt = performance.now();
  // 相对毫秒比绝对时间好读：A/B 两条路径的区别就是「同一次按键内」还是「隔了一个 tick」
  const ms = Math.round(performance.now() - firstAt);
  lines.push(`+${String(ms).padStart(5)}ms  ${line}`);
  if (lines.length > MAX_LINES) lines.shift();
}

function installGlobals() {
  const g = globalThis as unknown as Record<string, unknown>;
  if (g.__imeTrace) return;
  g.__imeTrace = () => (lines.length ? lines.join("\n") : "(空 —— 还没有事件被记录)");
  g.__imeTraceClear = () => {
    lines.length = 0;
  };
}

/**
 * 挂到 xterm 的 textarea 上记录整条 DOM 事件链。
 *
 * **必须在 `attachMacShiftSymbolFix` 之后挂**：三方都用 bubble 阶段，同阶段按注册顺序跑，
 * 只有最后注册才能看到前面两个（xterm 的 capture 监听 + 我们自己的 bubble 监听）
 * 是否已经 `preventDefault()`。顺序错了 `prevented` 一栏就恒为 0，整个 trace 失去意义。
 */
export function attachIMETrace(
  term: { textarea: HTMLTextAreaElement | undefined | null },
  paneId: string,
): () => void {
  if (!enabled()) return () => {};
  const ta = term.textarea;
  if (!ta) return () => {};
  installGlobals();

  const tag = paneId.slice(0, 6);
  const q = (s: string | null) => JSON.stringify(s);

  const onKeyDown = (e: KeyboardEvent) =>
    push(
      `${tag} keydown        key=${q(e.key)} keyCode=${e.keyCode} shift=${+e.shiftKey} isComposing=${+e.isComposing} prevented=${+e.defaultPrevented}`,
    );
  const onCompStart = (e: CompositionEvent) => push(`${tag} compositionstart  data=${q(e.data)}`);
  const onCompUpdate = (e: CompositionEvent) => push(`${tag} compositionupdate data=${q(e.data)}`);
  const onCompEnd = (e: CompositionEvent) => push(`${tag} compositionend    data=${q(e.data)}`);
  // cancelable 是本次取证的头号目标：insertCompositionText 若不可取消，
  // terminalInputFix.ts:85 的 preventDefault 是空操作，那里就是多余的一次发送。
  const onBeforeInput = (e: InputEvent) =>
    push(
      `${tag} beforeinput    type=${e.inputType} data=${q(e.data)} cancelable=${+e.cancelable} prevented=${+e.defaultPrevented}`,
    );
  // 这条出现 = preventDefault 没生效（或没人调），xterm 的 _inputEvent 会跟着跑一遍。
  // DOM lib 把 "input" 标成 Event（只有 "beforeinput" 才是 InputEvent），所以这里手动窄化。
  const onInput = (ev: Event) => {
    const e = ev as InputEvent;
    push(`${tag} input          type=${e.inputType} data=${q(e.data)} prevented=${+e.defaultPrevented}`);
  };

  ta.addEventListener("keydown", onKeyDown);
  ta.addEventListener("compositionstart", onCompStart);
  ta.addEventListener("compositionupdate", onCompUpdate);
  ta.addEventListener("compositionend", onCompEnd);
  ta.addEventListener("beforeinput", onBeforeInput);
  ta.addEventListener("input", onInput);

  return () => {
    ta.removeEventListener("keydown", onKeyDown);
    ta.removeEventListener("compositionstart", onCompStart);
    ta.removeEventListener("compositionupdate", onCompUpdate);
    ta.removeEventListener("compositionend", onCompEnd);
    ta.removeEventListener("beforeinput", onBeforeInput);
    ta.removeEventListener("input", onInput);
  };
}

/**
 * 记录一次 `onData`，以及它有没有被 composition gate 丢掉。
 *
 * DOM 事件链只能说明「浏览器给了几次」，这条才说明「PTY 收到了几次」。重复到底出在
 * 哪一层，靠对比这两组数字。
 */
export function traceData(paneId: string, data: string, discarded: boolean): void {
  if (!enabled()) return;
  installGlobals();
  push(
    `${paneId.slice(0, 6)} onData         ${JSON.stringify(data)} ${discarded ? "✗ 被 gate 丢弃(composing)" : "→ 写入 PTY"}`,
  );
}
