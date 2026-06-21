import { Terminal as Xterm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { attachMacShiftSymbolFix, attachIMECompositionGate } from "./terminalInputFix";
import { activateUnicodeProvider } from "./terminal/unicode-provider";

export type TerminalInstance = {
  terminal: Xterm;
  element: HTMLDivElement;
  fitAddon: FitAddon;
  searchAddon: SearchAddon;
  paneId: string;
  cwd: string;
  initCommand: string | null;
  ptyReady: boolean;
  disposed: boolean;
  unlistenData: UnlistenFn | null;
  unlistenExit: UnlistenFn | null;
  resizeObserver: ResizeObserver | null;
  themeObserver: MutationObserver | null;
  detachShiftFix: (() => void) | null;
  detachImeGate: (() => void) | null;
  isComposing: () => boolean;
  linkProviderDisposable: { dispose: () => void } | null;
};

// 匹配本地路径（支持中文文件名 + 目录）：
// 1. 含扩展名的文件：foo.md、Lens智能选币器_TRD.md、src/bar.ts:12:34
// 2. 绝对路径或 ~ 路径（含目录）：/Users/foo、~/Pictures
// 3. 显式相对路径：./foo、../bar
// 4. ls -F 风格目录（后缀 /）：Pictures/
// 注：裸标识符（无前缀/后缀/扩展名）不识别，避免误匹配普通单词
const PATH_REGEX = /(?:\.{0,2}\/|~\/)?(?:[\w一-龥.\-]+\/)*[\w一-龥.\-]+\.[\w]+(?::\d+(?::\d+)?)?|(?:\/|~\/)[\w一-龥.\-/]*[\w一-龥.\-](?::\d+(?::\d+)?)?|\.{1,2}\/[\w一-龥.\-/]*[\w一-龥.\-](?::\d+(?::\d+)?)?|[\w一-龥.\-]+\/(?=\s|$)/g;

// 排除明显的伪路径（版本号、slash 命令、纯数字等）
function isLikelyPath(text: string): boolean {
  // 全数字+点（版本号 1.14.1 / IP 地址）
  if (/^[\d.]+$/.test(text)) return false;
  // 长度太短（<3 字符）
  if (text.length < 3) return false;
  // /xxx 形式但 xxx 没有任何斜杠也没有扩展名（slash 命令如 /reload-plugins）
  if (/^\/[\w\-]+$/.test(text)) return false;
  return true;
}

// 解析路径文本：剥离行号/列号
function parsePathText(text: string): { path: string; line?: number; col?: number } {
  const m = text.match(/^(.+?)(?::(\d+)(?::(\d+))?)?$/);
  if (!m) return { path: text };
  return {
    path: m[1],
    line: m[2] ? parseInt(m[2], 10) : undefined,
    col: m[3] ? parseInt(m[3], 10) : undefined,
  };
}

// 把相对路径解析为绝对路径（相对于 cwd）
// ~ 展开交给 Rust 端处理（前端不知道 HOME）
function resolvePath(text: string, cwd: string): string {
  if (text.startsWith("/")) return text;
  if (text.startsWith("~/") || text === "~") return text;
  if (text.startsWith("./")) return cwd.replace(/\/+$/, "") + "/" + text.slice(2);
  if (text.startsWith("../")) {
    const parts = cwd.replace(/\/+$/, "").split("/");
    let rest = text;
    while (rest.startsWith("../")) {
      parts.pop();
      rest = rest.slice(3);
    }
    return parts.join("/") + "/" + rest;
  }
  // 不带 ./ 的相对路径
  return cwd.replace(/\/+$/, "") + "/" + text;
}

function readTheme() {
  const c = getComputedStyle(document.documentElement);
  const v = (name: string, fb: string) => c.getPropertyValue(name).trim() || fb;
  return {
    background: v("--bg", "#1e1e1e"),
    foreground: v("--fg", "#d4d4d4"),
    cursor: v("--fg", "#d4d4d4"),
    selectionBackground: v("--selection-bg", "rgba(255, 255, 255, 0.22)"),
    selectionInactiveBackground: v("--selection-bg-inactive", "rgba(255, 255, 255, 0.12)"),
    black: v("--ansi-black", "#1e1e1e"),
    red: v("--ansi-red", "#f44747"),
    green: v("--ansi-green", "#6a9955"),
    yellow: v("--ansi-yellow", "#d7ba7d"),
    blue: v("--ansi-blue", "#569cd6"),
    magenta: v("--ansi-magenta", "#c586c0"),
    cyan: v("--ansi-cyan", "#4ec9b0"),white: v("--ansi-white", "#d4d4d4"),
    brightBlack: v("--ansi-bright-black", "#808080"),
    brightRed: v("--ansi-bright-red", "#f14c4c"),
    brightGreen: v("--ansi-bright-green", "#73c991"),
    brightYellow: v("--ansi-bright-yellow", "#e2c08d"),
    brightBlue: v("--ansi-bright-blue", "#6cb6ff"),
    brightMagenta: v("--ansi-bright-magenta", "#d2a8ff"),
    brightCyan: v("--ansi-bright-cyan", "#58d1c9"),
    brightWhite: v("--ansi-bright-white", "#e5e5e5"),
  };
}


class TerminalManager {
  private instances = new Map<string, TerminalInstance>();
  private themeObserver: MutationObserver | null = null;

  constructor() {
    this.themeObserver = new MutationObserver(() => {
      const theme = readTheme();
      for (const inst of this.instances.values()) {
        if (!inst.disposed) {
          inst.terminal.options.theme = theme;
        }
      }
    });
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
  }

  has(paneId: string): boolean {
    return this.instances.has(paneId);
  }

  get(paneId: string): TerminalInstance | undefined {
    return this.instances.get(paneId);
  }

  getElement(paneId: string): HTMLDivElement | undefined {
    return this.instances.get(paneId)?.element;
  }

  create(paneId: string, cwd: string, initCommand: string | null): TerminalInstance {
    if (this.instances.has(paneId)) {
      return this.instances.get(paneId)!;
    }

    const element = document.createElement("div");
    element.className = "xterm-inner";

    const terminal = new Xterm({
      allowProposedApi: true,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace, 'Apple SD Gothic Neo', 'Hiragino Sans GB', 'PingFang SC', 'Microsoft YaHei'",
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      theme: readTheme(),
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    const searchAddon = new SearchAddon();
    terminal.loadAddon(searchAddon);
    try {
      const unicode11Addon = new Unicode11Addon();
      terminal.loadAddon(unicode11Addon);
      terminal.unicode.activeVersion = "11";
    } catch (e) {
      console.warn("[TerminalManager] unicode11 addon failed:", e);
    }
    terminal.open(element);

    // 激活 ZWJ emoji 修复（必须在 open 之后）
    try {
      activateUnicodeProvider(terminal);
    } catch (e) {
      console.warn("[TerminalManager] unicode provider activation failed:", e);
    }

    // WebGL renderer：多 pane 渲染性能 3-5x，掉帧/拖影消失。
    // 失败时自动回退到默认 DOM 渲染（如 WKWebView 无 WebGL 支持）。
    let webglAddon: WebglAddon | null = null;
    try {
      webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        webglAddon?.dispose();
        webglAddon = null;
      });
      terminal.loadAddon(webglAddon);
    } catch (e) {
      console.warn("[TerminalManager] WebGL renderer unavailable, fallback to DOM:", e);
      webglAddon = null;
    }

    const detachShiftFix = attachMacShiftSymbolFix(terminal);
    const imeGate = attachIMECompositionGate(terminal);

    const inst: TerminalInstance = {
      terminal,
      element,
      fitAddon,
      searchAddon,
      paneId,
      cwd,
      initCommand,
      ptyReady: false,
      disposed: false,
      unlistenData: null,
      unlistenExit: null,
      resizeObserver: null,
      themeObserver: null,
      detachShiftFix,
      detachImeGate: imeGate.detach,
      isComposing: imeGate.isComposing,
      linkProviderDisposable: null,
    };

    // OSC 7：shell 通过 \033]7;file://host/path\033\\ 通知 cwd 变化
    // pty.rs 已通过 ZDOTDIR 注入 zsh chpwd_functions 自动发送
    try {
      terminal.parser.registerOscHandler(7, (data: string) => {
        try {
          const u = new URL(data);
          if (u.protocol === "file:") {
            const newCwd = decodeURIComponent(u.pathname);
            if (newCwd) inst.cwd = newCwd;
          }
        } catch {}
        return false;
      });
    } catch {}

    // 注册路径 link provider：扫描每行匹配本地路径，点击 → 用 open 打开
    inst.linkProviderDisposable = terminal.registerLinkProvider({
      provideLinks: (lineNumber, callback) => {
        const buffer = terminal.buffer.active;
        const line = buffer.getLine(lineNumber - 1);
        if (!line) { callback(undefined); return; }
        const text = line.translateToString(true);
        const links = [];
        let m: RegExpExecArray | null;
        const re = new RegExp(PATH_REGEX.source, "g");
        while ((m = re.exec(text)) !== null) {
          const matched = m[0];
          if (!isLikelyPath(matched)) continue;
          // 默认不显示下划线/pointer，只有按住 Cmd 时才作为链接（class 由全局 keydown 切换）
          const cmdPressed = document.body.classList.contains("cmd-pressed");
          links.push({
            range: {
              start: { x: m.index + 1, y: lineNumber },
              end: { x: m.index + matched.length, y: lineNumber },
            },
            text: matched,
            decorations: { underline: cmdPressed, pointerCursor: cmdPressed },
            activate: (e: MouseEvent, t: string) => {
              if (!e.metaKey && !e.ctrlKey) return;
              const { path } = parsePathText(t);
              const abs = resolvePath(path, inst.cwd);
              invoke("open_path", { path: abs, reveal: false }).catch(() => {});
            },
          });
        }
        callback(links.length ? links : undefined);
      },
    });

    this.instances.set(paneId, inst);
    // 不在 create 时 spawn PTY — 等 mount() 把 element 放进 DOM 后再 spawn
    // 这样 ResizeObserver / getBoundingClientRect 才能读到正确尺寸
    return inst;
  }

  private async _spawnPty(inst: TerminalInstance) {
    const { terminal, fitAddon, paneId, cwd, initCommand } = inst;
    const pendingInput: string[] = [];

    terminal.onData((data) => {
      // IME composition 期间，xterm.js 在 WKWebView 下偶尔把
      // insertCompositionText 的中间状态漏到 onData → 中文敲一下出 2 个。
      // 这里 gate 一下：composition 中丢弃，仅放行最终字符。
      if (inst.isComposing()) return;
      if (inst.ptyReady) {
        invoke("pty_write", { id: paneId, data }).catch(() => {});
      } else {
        pendingInput.push(data);
      }
    });

    // 等 element 被挂载到 DOM 且有实际尺寸后再 spawn PTY
    const ready = await new Promise<{ w: number; h: number }>((resolve) => {
      let done = false;
      const finish = (w: number, h: number) => {
        if (done) return;
        done = true;
        ro.disconnect();
        clearTimeout(timer);
        resolve({ w, h });
      };
      const ro = new ResizeObserver((entries) => {
        const r = entries[0]?.contentRect;
        if (r && r.width >= 50 && r.height >= 20) finish(r.width, r.height);
      });
      ro.observe(inst.element);
      const timer = setTimeout(() => {
        const rect = inst.element.getBoundingClientRect();
        finish(rect?.width || 800, rect?.height || 600);
      }, 500);
    });

    if (inst.disposed) return;

    try {
      const proposed = fitAddon.proposeDimensions();
      fitAddon.fit();
      const rect = inst.element.getBoundingClientRect();
      // 调试 #111：详细记录尺寸计算
      console.log(`[#111 Debug] paneId=${paneId}, containerWidth=${rect.width}, DPR=${window.devicePixelRatio}, proposed=${JSON.stringify(proposed)}, actual cols=${terminal.cols}, rows=${terminal.rows}`);
    } catch (e) {
      console.warn(`[#111 Debug] fit failed:`, e);
    }
    let cols = Math.max(20, terminal.cols);
    let rows = Math.max(5, terminal.rows);

    // batch write：累积 PTY 输出，每帧只 write 一次（减少 xterm 内部渲染次数）
    let outputBuffer = "";
    let outputRaf: number | null = null;
    inst.unlistenData = await listen<string>(`pty:data:${paneId}`, (e) => {
      outputBuffer += e.payload;
      if (outputRaf === null) {
        outputRaf = requestAnimationFrame(() => {
          outputRaf = null;
          if (outputBuffer) {
            terminal.write(outputBuffer);
            outputBuffer = "";
          }
        });
      }
    });
    inst.unlistenExit = await listen<number>(`pty:exit:${paneId}`, () => {
      terminal.writeln("\r\n\x1b[2m[进程已退出]\x1b[0m");
    });

    if (inst.disposed) {
      inst.unlistenData?.();
      inst.unlistenExit?.();
      return;
    }

    try {
      await invoke("pty_spawn", { id: paneId, cwd, cols, rows, initCommand: initCommand ?? null });
    } catch (e) {
      terminal.writeln(`\r\n\x1b[31mPTY 启动失败: ${e}\x1b[0m`);
      return;
    }

    if (inst.disposed) {
      invoke("pty_kill", { id: paneId }).catch(() => {});
      return;
    }

    inst.ptyReady = true;
    if (pendingInput.length > 0) {
      const buffered = pendingInput.join("");
      pendingInput.length = 0;
      invoke("pty_write", { id: paneId, data: buffered }).catch(() => {});
    }

    // ResizeObserver: 容器尺寸变化时 fit + resize PTY
    // 旧实现 cancelAnimationFrame + rAF 不去抖（rAF 同帧执行，下一帧已晚），导致 macOS
    // 全屏动画期间每帧 SIGWINCH，shell 重画 prompt 累积成 N 行。改为 leading + trailing：
    // burst 开始 fit 一次（响应性），最后 200ms 静默后再 fit 一次（精度），中间帧丢弃。
    // 同时去重 pty_resize：cols/rows 没变就不发 SIGWINCH。
    let lastFitAt = 0;
    let lastCols = -1, lastRows = -1;
    let pendingFitTimeout: ReturnType<typeof setTimeout> | null = null;
    const doFit = () => {
      if (inst.disposed) return;
      const el = inst.element;
      if (!el.offsetParent && !el.closest("[style*='display: block']")) return;
      const rect = el.getBoundingClientRect();
      if (rect.width < 50 || rect.height < 20) return;
      try {
        fitAddon.fit();
        const cols = Math.max(20, terminal.cols);
        const rows = Math.max(5, terminal.rows);
        if (cols !== lastCols || rows !== lastRows) {
          lastCols = cols; lastRows = rows;
          invoke("pty_resize", { id: paneId, cols, rows }).catch(() => {});
        }
      } catch {}
    };
    const fitNow = () => { lastFitAt = performance.now(); doFit(); };
    inst.resizeObserver = new ResizeObserver(() => {
      if (inst.disposed) return;
      // leading：burst 起始（>200ms 没 fit 过）立即响应
      if (performance.now() - lastFitAt > 200) fitNow();
      // trailing：burst 结束 200ms 后再 fit 一次校准
      if (pendingFitTimeout != null) clearTimeout(pendingFitTimeout);
      pendingFitTimeout = setTimeout(() => { pendingFitTimeout = null; fitNow(); }, 200);
    });
    // 观察 .container-terminal（containing block），WebKit 对 absolute 子元素的 ResizeObserver 不可靠
    const observeTarget = inst.element.closest(".container-terminal") ?? inst.element.parentElement ?? inst.element;
    inst.resizeObserver.observe(observeTarget);
  }

  mount(paneId: string, container: HTMLElement) {
    const inst = this.instances.get(paneId);
    if (!inst) return;
    if (inst.element.parentElement !== container) {
      container.appendChild(inst.element);
    }
    // 首次 mount 后才 spawn PTY（确保 element 在 DOM 里有真实尺寸）
    if (!inst.ptyReady && !inst.disposed && !inst.unlistenData) {
      this._spawnPty(inst);
    }
  }

  focus(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    try { inst.terminal.focus(); } catch {}
  }

  fit(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    try {
      inst.fitAddon.fit();
      if (inst.ptyReady) {
        invoke("pty_resize", {
          id: paneId,
          cols: Math.max(20, inst.terminal.cols),
          rows: Math.max(5, inst.terminal.rows),
        }).catch(() => {});
      }
    } catch {}
  }

  fitAll() {
    for (const [id, inst] of this.instances) {
      if (inst.disposed) continue;
      const el = inst.element;
      if (!el.offsetParent && !el.closest("[style*='display: block']")) continue;
      try {
        inst.fitAddon.fit();
        invoke("pty_resize", {
          id,
          cols: Math.max(20, inst.terminal.cols),
          rows: Math.max(5, inst.terminal.rows),
        }).catch(() => {});
      } catch {}
    }
  }

  // 局内搜索 API
  searchNext(paneId: string, term: string): boolean {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed || !term) return false;
    try { return inst.searchAddon.findNext(term); } catch { return false; }
  }
  searchPrevious(paneId: string, term: string): boolean {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed || !term) return false;
    try { return inst.searchAddon.findPrevious(term); } catch { return false; }
  }
  searchClear(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    try { inst.searchAddon.clearDecorations(); } catch {}
  }

  destroy(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst) return;
    inst.disposed = true;
    inst.detachShiftFix?.();
    inst.detachImeGate?.();
    inst.linkProviderDisposable?.dispose();
    inst.unlistenData?.();
    inst.unlistenExit?.();
    inst.resizeObserver?.disconnect();
    inst.themeObserver?.disconnect();
    invoke("pty_kill", { id: paneId }).catch(() => {});
    inst.terminal.dispose();
    inst.element.remove();
    this.instances.delete(paneId);
  }

  destroyAll() {
    for (const paneId of [...this.instances.keys()]) {
      this.destroy(paneId);
    }
  }
}

export const terminalManager = new TerminalManager();
