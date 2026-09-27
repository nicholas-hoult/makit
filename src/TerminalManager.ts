import { Terminal as Xterm } from "@xterm/xterm";
import { terminalSpan } from "./perf";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { attachMacShiftSymbolFix, attachIMECompositionGate } from "./terminalInputFix";
import { attachIMETrace, traceData } from "./imeTrace";
import { traceXtermResize, traceResizeSent, traceResizeAck } from "./sizeTrace";
import { claimResize, revertResize, forgetPane, clampSize } from "./ptySize";
import { activateUnicodeProvider } from "./terminal/unicode-provider";
import { isCmd } from "./keys";
import { openUrl } from "@tauri-apps/plugin-opener";
import { osc8Target, WORD_SEPARATORS } from "./terminalLinks";
import { installTerminalLinks } from "./terminalLinkInstall";
import { DEFAULT_FONT_SIZE, nextFontSize, type ZoomAction } from "./fontZoom";
import { installTerminalScrollbar } from "./terminalScrollbar";
import { installJumpLatest } from "./terminalJumpLatest";
import { installPreciseWheel } from "./terminalWheel";
import { COLS_FOLLOW_MS, createColsFollower, planResize, proposeGeometry } from "./resizePlan";

export type TerminalInstance = {
  terminal: Xterm;
  element: HTMLDivElement;
  searchAddon: SearchAddon;
  paneId: string;
  cwd: string;
  initCommand: string | null;
  /// 启动目录不存在时，允许不允许退到最近的存在的上级（`resolve_existing_cwd`）。
  /// shell / 新建 tab：允许，「起不来」比「起在别的目录」更糟。
  /// resume tab：不允许 —— 换了目录就换了 claude 的存储键，一个「目录没了、可恢复」
  /// 会变成「会话找不到」，而且只有一行黄字提示，用户无从下手。
  allowCwdFallback: boolean;
  /// 恢复目录之后就地重来一次 spawn（第一次被 cwd 闸拒绝时才有意义）
  retrySpawn: ((newCwd: string) => Promise<boolean>) | null;
  ptyReady: boolean;
  disposed: boolean;
  unlistenData: UnlistenFn | null;
  unlistenExit: UnlistenFn | null;
  resizeObserver: ResizeObserver | null;
  themeObserver: MutationObserver | null;
  detachShiftFix: (() => void) | null;
  detachImeGate: (() => void) | null;
  detachImeTrace: (() => void) | null;
  isComposing: () => boolean;
  linkProviderDisposable: { dispose: () => void } | null;
  /// 滚动条「滚时显形、停手淡出」（#188，见 terminalScrollbar.ts）
  scrollbarActivity: { dispose: () => void };
  /// 触控板滚动按 对标终端 算法换算（#189，见 terminalWheel.ts）
  preciseWheel: { dispose: () => void };
  /// 列数重排的跟随节流（#203）：拖动中每 COLS_FOLLOW_MS 跟一次，停手补最后一次，松手（flushResize）立即
  colsResize: { request(): void; flush(): void; dispose(): void };
  /// 「回到最新」浮层按钮（#205，见 terminalJumpLatest.ts）
  jumpLatest: { dispose: () => void };
};

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

// 终端 renderer 选择。默认 webgl；'dom' 是个 A/B 开关，留着排查渲染类问题。
//
// 花屏有**两个互相独立的根因**，都已修：
//   1. cols/rows 记账（#156）—— 三条 fit 路径各记一份账，见 `ptySize.ts` 和 `sendResize` 的注释。
//   2. WebGL 字形图集（xterm 上游 #6038）—— 图集是**按渲染配置共享**的（同字体/主题/DPR 的
//      所有 pane 共用一个 TextureAtlas），而 GlyphRenderer 是每个终端一份。页满时合并 4 页、
//      删页、后面的页整体挪进更小的槽位并重编所有 glyph 的 texturePage，改完必须通知**全部**
//      渲染器缓存失效。0.19.0 的通知是个读一次就清零的布尔，只有第一个来读的渲染器收到；
//      而槽位要不要重传只看页 version 相等，合并出的新页 version 从 0 起、会和槽位缓存的旧
//      值撞车 → 判「没变」→ 永不重传。修法在上游：PR #6042 换成单调递增的 pageLayoutVersion +
//      每渲染器各自 ack，PR #6055 补上 clearTexture() 不 bump 的漏洞。
//      **修复只在 0.20.0-beta，稳定版 0.19.0 没有**，所以 package.json 里 xterm 全家桶精确 pin
//      在 beta 上（见那边的版本号）。等 0.20.0 发稳定版再换回 caret。
//
// 别在 userland 调 `clearTextureAtlas()` 当补丁：图集是共享的，一个 pane 清图集会让**其它
// pane** 的 vertex 数据指向刚被清空的行（上游 #6014）。曾经这么修过，已 revert。
function rendererPref(): "webgl" | "dom" {
  try {
    return localStorage.getItem("makit-renderer") === "dom" ? "dom" : "webgl";
  } catch {
    return "webgl";
  }
}

/**
 * **唯一一条**往 PTY 发尺寸的路。#156 的修复就是把它收成一条。
 *
 * 过去三条 fit 路径（`doFit` / `fit` / `fitAll`）各自决定要不要发，只有 `doFit` 带去重，
 * 用的是它闭包里的 `lastCols/lastRows` —— 那份表看不见另外两条路也改过 PTY。于是尺寸
 * **回到** `doFit` 记过的旧值时它判定「没变」而跳过，PTY 永久停在别的宽度上：程序按更宽
 * 的值排版 → xterm 自动折行 → 程序少算行数 → 旧行不被清 → 花屏，且不自愈。
 * 去重的判据是「PTY 是否已经知道」，那这份账就必须和 PTY 一一对应，见 `ptySize.ts`。
 */
function sendResize(paneId: string, site: string, cols: number, rows: number, terminal: Xterm): void {
  const { send, prev } = claimResize(paneId, { cols, rows });
  if (!send) return;
  traceResizeSent(paneId, site, cols, rows);
  const now = () => ({ cols: terminal.cols, rows: terminal.rows });
  invoke("pty_resize", { id: paneId, cols, rows })
    // 回执这一刻才是判据：PTY 已经知道新尺寸，xterm 此时若已经又变了，就是一次会留疤的不一致
    .then(() => traceResizeAck(paneId, site, { cols, rows }, now()))
    .catch((e) => {
      // 失败必须退账。不退的话这个尺寸被永久当成「PTY 已经知道」，后果和跳过一次 resize 完全一样
      revertResize(paneId, { cols, rows }, prev);
      traceResizeAck(paneId, site, { cols, rows }, now(), e);
    });
}

/**
 * xterm 现在多大（夹到 20x5 下限），**并把 xterm 一起夹过去**。
 *
 * 旧代码只抬 PTY 那一边（`Math.max(20, terminal.cols)`），那是我们主动造出来的 #156：
 * 50px 宽的 pane 里 xterm 只有 7 列，却告诉 PTY 20 列 —— 和漏发一次 resize 一模一样的后果。
 * 下限要留（极窄 pane 里的 TUI 会彻底错乱），但两边必须是同一个数。
 */
function fitSize(terminal: Xterm): { cols: number; rows: number } {
  const size = clampSize({ cols: terminal.cols, rows: terminal.rows });
  if (size.cols !== terminal.cols || size.rows !== terminal.rows) {
    terminal.resize(size.cols, size.rows);
  }
  return size;
}

/**
 * 按行列分离策略把 xterm 调到容器的尺寸（#203，见 resizePlan.ts）：行数立即，列数（要重排整段回滚）
 * 除非 `immediate` 或缓冲区很小，否则延后。返回列数是否还欠着 —— 欠着的由调用方交给 `colsResize` 防抖。
 * 不用 `FitAddon.fit()`（#111 之后连这个 addon 都不装了）：它一次把行列一起调，
 * 列数的重排代价全压在拖动的每一帧上。
 */
/**
 * 量出容器现在能放下多少行列。取代 `fitAddon.proposeDimensions()` —— 少减一条滚动条宽度（#111），
 * 原因和取舍见 `proposeGeometry` 的注释。读 DOM 的部分留在这里，算的部分是纯函数，能单测。
 */
function proposeSize(inst: TerminalInstance): { cols: number; rows: number } | undefined {
  const t = inst.terminal;
  const el = t.element;
  const parent = el?.parentElement;
  if (!el || !parent) return undefined;
  const cell = (t as unknown as {
    _core: { _renderService: { dimensions?: { css?: { cell?: { width: number; height: number } } } } };
  })._core._renderService.dimensions?.css?.cell;
  if (!cell?.width || !cell?.height) return undefined;   // 还没测出字符尺寸，这一轮不动
  const es = window.getComputedStyle(el);
  const ps = window.getComputedStyle(parent);
  const num = (v: string) => parseInt(v) || 0;
  return proposeGeometry(
    num(ps.getPropertyValue("width")), num(ps.getPropertyValue("height")),
    num(es.paddingLeft) + num(es.paddingRight), num(es.paddingTop) + num(es.paddingBottom),
    cell.width, cell.height,
  );
}

function resizeByPlan(inst: TerminalInstance, immediate: boolean): boolean {
  const next = proposeSize(inst);
  if (!next || !Number.isFinite(next.cols) || !Number.isFinite(next.rows)) return false;
  const t = inst.terminal;
  const plan = planResize({ cols: t.cols, rows: t.rows }, next, t.buffer.active.length, immediate);
  if (plan.colsNow || plan.rowsNow) {
    // 和上游 `FitAddon.fit()` 一样：改尺寸前清掉渲染缓存，避免残影
    (t as unknown as { _core: { _renderService: { clear(): void } } })._core._renderService.clear();
    t.resize(plan.colsNow ? next.cols : t.cols, next.rows);
    renderNow(t);
  }
  return plan.colsLater;
}

/**
 * 改完尺寸在**这一帧**就重画（#203「拖动时字短暂消失」）。
 *
 * 改尺寸会立刻重设 WebGL 画布大小（画布随之被清空），而 xterm 的重画由 RenderDebouncer 排到**下一帧**的
 * requestAnimationFrame —— 中间这一帧合成出来就是空白。拖动时每帧都在改尺寸，于是字一直闪。
 * 实测（4 pane 拖高度 2s）：改行数 108 次，当帧没重画 108 次（100%）；当帧立即重画后 0 次，每帧耗时不增加。
 * xterm 没有同步重画的公开 API，这里取消排队的那一帧、直接执行它；内部结构变了就什么都不做（退回原行为）。
 */
function renderNow(t: Xterm): void {
  const d = (t as unknown as {
    _core?: { _renderService?: { _renderDebouncer?: {
      _animationFrame?: number;
      _innerRefresh?: () => void;
      _coreBrowserService?: { window: Window };
    } } };
  })._core?._renderService?._renderDebouncer;
  if (!d || d._animationFrame === undefined || typeof d._innerRefresh !== "function" || !d._coreBrowserService) return;
  d._coreBrowserService.window.cancelAnimationFrame(d._animationFrame);
  d._innerRefresh();
}

// ⌘F 命中高亮。不传 decorations 的话 search addon 只做一件事：把命中项**选中** ——
// 于是"高亮"实际用的是 --selection-bg（半透明、故意做得安静的正文选择色），
// 而其余命中项完全没有标记。所以之前找东西全靠盯着光标跳到哪。
// 颜色由主题推导（见 theme.ts 里 --search-match-* 那段），必须是不透明 #RRGGBB。
function searchDecorations() {
  const c = getComputedStyle(document.documentElement);
  const v = (name: string, fb: string) => c.getPropertyValue(name).trim() || fb;
  const border = v("--search-match-border", "#e2c08d");
  const activeBorder = v("--search-match-active-border", "#e08a5d");
  return {
    matchBackground: v("--search-match-bg", "#5d5242"),
    matchBorder: border,
    matchOverviewRuler: border,
    activeMatchBackground: v("--search-match-active-bg", "#885145"),
    activeMatchBorder: activeBorder,
    activeMatchColorOverviewRuler: activeBorder,
  };
}


class TerminalManager {
  private instances = new Map<string, TerminalInstance>();
  private themeObserver: MutationObserver | null = null;

  /// 「启动目录不存在，且这个 pane 不许降级」时的回调。App 挂上去弹恢复对话框。
  ///
  /// 放在这一层而不是各个"打开会话"的入口，是因为**只有这里是真正的收口点**：
  /// 侧栏点击 / 侧栏拖拽 / SessionTree 拖拽 / ⌘K+修饰键 / 重启后从持久化 workspace
  /// 恢复 tab —— 五条路最后都落到 `_spawnPty`。最后那条最要命：目录通常就是在 app
  /// 关着的时候被删/改名的，而它绕过所有前端入口。
  onCwdMissing: ((paneId: string, cwd: string) => void) | null = null;

  /// 后端把 resume tab 改到会话起始目录启动了（#190，见 pty.rs 的 pty_spawn）。
  /// App 挂上去改掉持久化的 tab 记录 —— 否则从这个 tab 新开的终端还会继承旧目录。
  onCwdCorrected: ((paneId: string, cwd: string) => void) | null = null;

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

  /// 当前选中的文本，没有选中时是空串。右键菜单靠它判断弹不弹（没选中就不弹）。
  /// 包在 try 里：xterm 在 dispose 之后读 selection 会抛。
  getSelection(paneId: string): string {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return "";
    try {
      return inst.terminal.getSelection();
    } catch {
      return "";
    }
  }

  create(
    paneId: string,
    cwd: string,
    initCommand: string | null,
    allowCwdFallback: boolean = true,
  ): TerminalInstance {
    if (this.instances.has(paneId)) {
      return this.instances.get(paneId)!;
    }

    const element = document.createElement("div");
    element.className = "xterm-inner";

    const terminal = new Xterm({
      allowProposedApi: true,
      // 双击选词的分隔符照 对标终端（#214）：双击 `foo.ts:12` 只选 `foo.ts`，路径里的 / 和 . 不算分隔
      wordSeparator: WORD_SEPARATORS,
      // OSC 8 超链接（程序主动嵌在输出里的链接）。不设这个，xterm 会用 confirm() 弹窗确认，
      // 而这个 WebView 里 confirm 恒为 false，结果是一个都点不开；默认还只放行 http(s)，file:// 被丢掉（#214）
      linkHandler: {
        allowNonHttpProtocols: true,
        activate: (e: MouseEvent, uri: string) => {
          if (!isCmd(e)) return;   // 和网址、路径一样 ⌘+点击才开（同 对标终端）
          const t = osc8Target(uri);
          if (t?.kind === "url") openUrl(t.url).catch(() => {});
          else if (t?.kind === "file") invoke("open_path", { path: t.path, reveal: false }).catch(() => {});
        },
      },
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace, 'Apple SD Gothic Neo', 'Hiragino Sans GB', 'PingFang SC', 'Microsoft YaHei'",
      fontSize: DEFAULT_FONT_SIZE,
      cursorBlink: true,
      // 2000 而不是 5000（#203）：改宽度要把整段回滚按新列数重排，耗时与回滚行数成正比
      // （实测 5000 行 ~21ms/次、2000 行 ~8ms）。5000 行时「文字跟着手走」和「不掉帧」无法兼得，
      // 用户选了「跟手」：降到 2000 行，配合 33ms 跟随节奏，2 pane 快拖每帧忙碌中位 15ms。
      scrollback: 2000,
      theme: readTheme(),
      // 终端"内容"的颜色不归 theme.ts 的推导管：那边的 readable() 只兜派生的 UI 变量，
      // ANSI 16 色是照抄上游配色原封不动交给 xterm 的（theme.ts:95）。上游浅色配色里
      // 低到不能用的槽位是常态而不是意外 —— gruvbox-light 的 ANSI black 就等于它的
      // bg（#fbf1c7，1.00:1，纯不可见），tokyo-night-light 的 bright-black 1.96:1、
      // everforest-light 3.08:1。再叠上 CLI 普遍用 SGR dim 画次要文字，浅色底上就成了
      // 一片看不见的字（Claude Code 的 diff 上下文行、shell prompt —— 就是 #150）。
      //
      // 不在 theme.ts 里给 ANSI 加下限，是因为那只能按"色板 vs bg"静态算，管不到
      // dim 的 alpha 和 256 色立方体（TERM=xterm-256color，CLI 的 hex 会被量化成
      // 立方体索引，那些索引根本不过主题色板）。xterm 这个选项是按**每格实际背景**
      // 算的，三条路径一视同仁。
      //
      // 取 4.5 = 正文 WCAG AA。注意 xterm 对 dim 刻意只要求一半
      // （addon-webgl: `minimumContrastRatio / (halfContrast ? 2 : 1)`），
      // 所以 dim 文字落在 2.25:1 —— 明显更淡但读得出，正是它该有的层级。
      // 想整体更狠就调这一个数字，不要回去改 25 套配色。
      minimumContrastRatio: 4.5,
    });

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
    //
    // 花屏的第二个根因出在这个 renderer 的共享字形图集里（见上面 rendererPref 的注释），
    // 靠升级到 0.20.0-beta 拿到上游修复。开关留着做 A/B、也留作最后的退路：
    //   localStorage.setItem('makit-renderer','dom')  → 关掉 WebGL，重启后生效
    let webglAddon: WebglAddon | null = null;
    try {
      if (rendererPref() === "dom") throw new Error("renderer=dom (用户显式关闭 WebGL)");
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
    // #137 取证用，默认整体是空函数。**必须排在 attachMacShiftSymbolFix 之后** ——
    // 两者都用 bubble 阶段，只有后注册的才能看到前面有没有 preventDefault（见 imeTrace.ts）。
    const detachImeTrace = attachIMETrace(terminal, paneId);

    const inst: TerminalInstance = {
      terminal,
      element,
      searchAddon,
      paneId,
      cwd,
      initCommand,
      allowCwdFallback,
      retrySpawn: null,
      ptyReady: false,
      disposed: false,
      unlistenData: null,
      unlistenExit: null,
      resizeObserver: null,
      themeObserver: null,
      colsResize: createColsFollower(() => this.applyDeferredCols(paneId), COLS_FOLLOW_MS),
      detachShiftFix,
      detachImeGate: imeGate.detach,
      detachImeTrace,
      isComposing: imeGate.isComposing,
      linkProviderDisposable: null,
      scrollbarActivity: installTerminalScrollbar(terminal, element),
      preciseWheel: installPreciseWheel(terminal, element),
      jumpLatest: installJumpLatest(terminal, element),
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

    // 链接识别 + 双击选链接 / 中文按词选（#214），见 terminalLinkInstall.ts
    const links = installTerminalLinks(terminal, element, () => inst.cwd);
    inst.linkProviderDisposable = links;

    this.instances.set(paneId, inst);
    // 不在 create 时 spawn PTY — 等 mount() 把 element 放进 DOM 后再 spawn
    // 这样 ResizeObserver / getBoundingClientRect 才能读到正确尺寸
    return inst;
  }

  private async _spawnPty(inst: TerminalInstance) {
    const { terminal, paneId, cwd, initCommand } = inst;
    const pendingInput: string[] = [];
    const span = terminalSpan(paneId, initCommand ? "resume" : "shell"); // #218
    let firstOutput = true;

    terminal.onData((data) => {
      // IME composition 期间，xterm.js 在 WKWebView 下偶尔把
      // insertCompositionText 的中间状态漏到 onData → 中文敲一下出 2 个。
      // 这里 gate 一下：composition 中丢弃，仅放行最终字符。
      const discarded = inst.isComposing();
      traceData(paneId, data, discarded); // #137 取证，关掉时是空函数
      if (discarded) return;
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

    span.mark("容器有尺寸");
    if (inst.disposed) return;

    resizeByPlan(inst, true);
    const { cols, rows } = fitSize(terminal);
    terminal.onResize(({ cols: c, rows: r }) => traceXtermResize(paneId, c, r));

    // batch write：累积 PTY 输出，每帧只 write 一次（减少 xterm 内部渲染次数）。
    // 背压（#229）：xterm 处理完这一批后 pty_ack 这批的事件数，后端在途事件到上限就停发、
    // 进而停读 PTY —— 否则 `yes` 刷屏时事件在主线程无限积压。rAF 在窗口被遮住时会停，
    // 所以再挂一个 100ms 定时器兜底，不然后台终端每批都要等后端 3 秒超时才放行。
    let outputBuffer = "";
    let pendingEvents = 0;
    let outputRaf: number | null = null;
    let outputTimer: ReturnType<typeof setTimeout> | null = null;
    const flushOutput = () => {
      if (outputRaf !== null) cancelAnimationFrame(outputRaf);
      if (outputTimer !== null) clearTimeout(outputTimer);
      outputRaf = null;
      outputTimer = null;
      if (!outputBuffer) return;
      const n = pendingEvents;
      pendingEvents = 0;
      terminal.write(outputBuffer, () => { invoke("pty_ack", { id: paneId, n }).catch(() => {}); });
      outputBuffer = "";
    };
    inst.unlistenData = await listen<string>(`pty:data:${paneId}`, (e) => {
      if (firstOutput) { firstOutput = false; span.end("首次输出"); }
      outputBuffer += e.payload;
      pendingEvents++;
      if (outputRaf === null) {
        outputRaf = requestAnimationFrame(flushOutput);
        outputTimer = setTimeout(flushOutput, 100);
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

    // ResizeObserver: 容器尺寸变化时 fit + resize PTY
    // 旧实现 cancelAnimationFrame + rAF 不去抖（rAF 同帧执行，下一帧已晚），导致 macOS
    // 全屏动画期间每帧 SIGWINCH，shell 重画 prompt 累积成 N 行。改为 leading + trailing：
    // burst 开始 fit 一次（响应性），最后 200ms 静默后再 fit 一次（精度），中间帧丢弃。
    // 同时去重 pty_resize：cols/rows 没变就不发 SIGWINCH。
    //
    // 收成闭包由 spawn 成功那一刻调用：目录没了的 resume tab 第一次 spawn 会被拒，
    // 用户恢复后走 `retrySpawn` 重来 —— 挂载点必须跟着成功的那一次走，
    // 否则恢复出来的 pane 不跟随尺寸变化。
    const attachResize = () => {
      let framePending = false;
      const doFit = () => {
        if (inst.disposed) return;
        const el = inst.element;
        if (!el.offsetParent && !el.closest("[style*='display: block']")) return;
        const rect = el.getBoundingClientRect();
        if (rect.width < 50 || rect.height < 20) return;
        try {
          const colsLater = resizeByPlan(inst, false);
          const { cols, rows } = fitSize(terminal);
          sendResize(paneId, "doFit", cols, rows, terminal);
          if (colsLater) inst.colsResize.request();
        } catch {}
      };
      // 每帧最多一次（#203）。原来是「距上次 >200ms 才 fit + 200ms 尾随」—— 为了躲开列数重排的代价，
      // 结果拖动时终端每 200ms 跳一下。现在列数的代价由 resizeByPlan 延后，行数每帧跟手也只要 ~1ms。
      inst.resizeObserver = new ResizeObserver(() => {
        if (inst.disposed || framePending) return;
        framePending = true;
        requestAnimationFrame(() => { framePending = false; doFit(); });
      });
      // 观察 .container-terminal（containing block），WebKit 对 absolute 子元素的 ResizeObserver 不可靠
      const observeTarget = inst.element.closest(".container-terminal") ?? inst.element.parentElement ?? inst.element;
      inst.resizeObserver.observe(observeTarget);
    };

    // spawn 收成一个能重跑的闭包：resume tab 的启动目录没了时后端会拒绝启动（`cwd-missing:`），
    // 用户在恢复对话框里选完之后要能就地重来。放在这里是因为 `pendingInput` / `terminal`
    // 只在这个作用域里 —— 重跑必须接上同一份缓冲输入，不能新起一套。
    const spawnAt = async (spawnCwd: string): Promise<boolean> => {
      let corrected: string | null = null;
      try {
        corrected = await invoke<string | null>("pty_spawn", {
          id: paneId,
          cwd: spawnCwd,
          cols,
          rows,
          initCommand: initCommand ?? null,
          allowCwdFallback: inst.allowCwdFallback,
        });
      } catch (e) {
        // 机器可读前缀（pty.rs）：这不是 PTY 故障，是「目录没了但会话还能救」。
        // 不写那行红字 —— 交给恢复对话框，红字只会让用户以为坏了。
        const msg = String(e);
        if (msg.startsWith("cwd-missing:")) {
          this.onCwdMissing?.(paneId, msg.slice("cwd-missing:".length));
          return false;
        }
        terminal.writeln(`\r\n\x1b[31mPTY 启动失败: ${e}\x1b[0m`);
        return false;
      }

      if (inst.disposed) {
        invoke("pty_kill", { id: paneId }).catch(() => {});
        return false;
      }

      span.mark("shell 已启动");
      inst.cwd = corrected ?? spawnCwd;
      if (corrected) this.onCwdCorrected?.(paneId, corrected);
      inst.ptyReady = true;
      // spawn 自己就带着 cols/rows，PTY 现在就是这个尺寸 —— 记上账，否则第一次 doFit
      // 会白发一次 SIGWINCH。若期间 xterm 已经又变了，第一次 doFit 会发现不同并补发
      claimResize(paneId, { cols, rows });
      if (pendingInput.length > 0) {
        const buffered = pendingInput.join("");
        pendingInput.length = 0;
        invoke("pty_write", { id: paneId, data: buffered }).catch(() => {});
      }
      attachResize();
      return true;
    };

    // 重跑时允许降级：这一刻的 cwd 是用户刚刚确认过的，再拦一次没有意义
    inst.retrySpawn = async (newCwd: string) => {
      if (inst.disposed || inst.ptyReady) return false;
      inst.allowCwdFallback = true;
      return spawnAt(newCwd);
    };

    await spawnAt(cwd);
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

  /// 往某个 pane 的屏幕上写一行提示。
  /// 给「启动被拒」这种情况用：对话框可以被关掉，关掉之后那块空白必须自己能说明白怎么了，
  /// 否则就是一个没有任何线索的死面板。
  notice(paneId: string, text: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    try { inst.terminal.writeln(text); } catch {}
  }

  /// 恢复完启动目录之后就地重来一次 spawn。返回是否真的起来了。
  async retrySpawn(paneId: string, newCwd: string): Promise<boolean> {
    const inst = this.instances.get(paneId);
    if (!inst?.retrySpawn) return false;
    return inst.retrySpawn(newCwd);
  }

  fit(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    // 祖先有 display:none 时 getBoundingClientRect 返回全零。不守住就直接 fit 到 MINIMUM_COLS=2
    // 再被 clampSize 夹到 20 列，PTY 告知 20、xterm 只有真实宽度 → #156 花屏且不自愈。
    // zoom() 走这条路，doFit 有这道守卫，fitAll 有，这里补齐。
    const rect = inst.element.getBoundingClientRect();
    if (rect.width < 50 || rect.height < 20) return;
    try {
      resizeByPlan(inst, true);
      if (inst.ptyReady) {
        const { cols, rows } = fitSize(inst.terminal);
        sendResize(paneId, "fit", cols, rows, inst.terminal);
      }
    } catch {}
  }

  /**
   * 缩放当前 pane 的字号（⌘= / ⌘− / ⌘0）。
   *
   * 只改 xterm 自己的 options.fontSize，字号本身不存进任何地方——关掉 tab
   * 就没了，跟 iTerm2 的 session 字号覆盖一样，是刻意选择，不是漏做持久化。
   *
   * 字号变了列数就变，必须让 PTY 知道，否则就是 #156 那个花屏：程序按一个
   * 宽度排版、xterm 按另一个宽度折行。这里复用已经做过记账的 fit()，不自己
   * 算 cols/rows、不自己 invoke pty_resize——那正是 #156 的根因（三条 resize
   * 路径各记一份账），加第四条自算路径等于把那个 bug 种回来。
   */
  zoom(paneId: string, action: ZoomAction) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    const cur = inst.terminal.options.fontSize ?? DEFAULT_FONT_SIZE;
    const next = nextFontSize(cur, action);
    if (next === cur) return; // 撞边界，或 reset 时已经是默认值：不用碰 fit()
    inst.terminal.options.fontSize = next;
    this.fit(paneId);
  }

  /**
   * 布局变了（分屏、关 pane、最大化、拖分割线）时调所有终端。
   * `immediate = false` 用于拖分割线的过程中：布局每帧都在变，这里每帧都会被调 ——
   * 列数重排延后到松手（flushResize）或停手 100ms（#203）。
   */
  fitAll(immediate = true) {
    for (const [id, inst] of this.instances) {
      if (inst.disposed) continue;
      const el = inst.element;
      // offsetParent + closest 守卫有一个已知漏洞：Terminal.tsx 给可见 tab 写了
      // style="display: block"，closest 会命中 el 自身，于是祖先 display:none（maximize
      // 隐藏的 pane）也能穿过这道守卫。getBoundingClientRect 在祖先 display:none 时返回全零，
      // 是唯一可靠的过滤手段，补在这里与 doFit 对齐。
      const rect = el.getBoundingClientRect();
      if (rect.width < 50 || rect.height < 20) continue;
      try {
        if (resizeByPlan(inst, immediate)) inst.colsResize.request();
        const { cols, rows } = fitSize(inst.terminal);
        // fitAll 不看 ptyReady：还没 spawn 的 pane 这次 invoke 必然被拒，站点名里标出来，
        // 免得把「无害的早发」和「真的漏了一次 resize」记成同一件事
        const site = inst.ptyReady ? "fitAll" : "fitAll(未spawn)";
        sendResize(id, site, cols, rows, inst.terminal);
      } catch {}
    }
  }

  /** 拖分割线松手：欠着的那次列数重排立即做掉，不等下一个节奏点（#203） */
  flushResize() {
    for (const inst of this.instances.values()) {
      if (!inst.disposed) inst.colsResize.flush();
    }
  }

  /** 跟随节奏到点：把欠着的列数补上（此时按立即处理，行列一起到位） */
  private applyDeferredCols(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    const rect = inst.element.getBoundingClientRect();
    if (rect.width < 50 || rect.height < 20) return;
    try {
      resizeByPlan(inst, true);
      const { cols, rows } = fitSize(inst.terminal);
      if (inst.ptyReady) sendResize(paneId, "colsDeferred", cols, rows, inst.terminal);
    } catch {}
  }

  // 局内搜索 API
  searchNext(paneId: string, term: string): boolean {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed || !term) return false;
    try { return inst.searchAddon.findNext(term, { decorations: searchDecorations() }); } catch { return false; }
  }
  searchPrevious(paneId: string, term: string): boolean {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed || !term) return false;
    try { return inst.searchAddon.findPrevious(term, { decorations: searchDecorations() }); } catch { return false; }
  }
  searchClear(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return;
    try { inst.searchAddon.clearDecorations(); } catch {}
  }
  /**
   * 订阅搜索结果变化，给搜索条上的「3/17」用。
   *
   * 这个事件**只在搜索带了 decorations 时才发** —— 上面 searchNext / searchPrevious
   * 都传了 `searchDecorations()`，所以没问题；哪天有人图省事去掉那个参数，计数会
   * 静静地不再更新。
   *
   * 返回反订阅函数。pane 不存在或已销毁时返回一个空函数，调用方不用判空。
   */
  onSearchResults(paneId: string, cb: (p: { index: number; count: number }) => void): () => void {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed) return () => {};
    try {
      const d = inst.searchAddon.onDidChangeResults((e) =>
        cb({ index: e.resultIndex, count: e.resultCount }),
      );
      return () => { try { d.dispose(); } catch {} };
    } catch { return () => {}; }
  }

  /**
   * 往终端插一段文本（拖文件进来插路径用），语义等同于一次粘贴。
   *
   * **用括号粘贴（bracketed paste）而不是裸写**：macOS 的文件名里可以有换行，裸写进
   * PTY 的话那个换行就是回车，会把用户半截的命令直接执行掉。括号粘贴的整个用途就是
   * 让行编辑器把中间的内容当字面文本。但它只在对端开了 DECSET 2004 时才有意义 ——
   * shell 都开（zsh / bash 的行编辑器），`cat` 之类不开，那时候包上标记只会把
   * `\x1b[200~` 原样回显出来，所以要看 `terminal.modes` 的实际状态。
   *
   * PTY 还没起来时直接返回 false，不排队：排队用的 `pendingInput` 是 `_spawnPty` 的
   * 局部变量，从这儿够不着；而一个还在 spawn 中的 pane 也不是个合理的落点。
   */
  writeText(paneId: string, text: string): boolean {
    const inst = this.instances.get(paneId);
    if (!inst || inst.disposed || !inst.ptyReady || !text) return false;
    let data = text;
    try {
      if (inst.terminal.modes.bracketedPasteMode) data = `\x1b[200~${text}\x1b[201~`;
    } catch {}
    invoke("pty_write", { id: paneId, data }).catch(() => {});
    return true;
  }

  destroy(paneId: string) {
    const inst = this.instances.get(paneId);
    if (!inst) return;
    inst.disposed = true;
    inst.detachShiftFix?.();
    inst.detachImeGate?.();
    inst.detachImeTrace?.();
    inst.linkProviderDisposable?.dispose();
    inst.scrollbarActivity.dispose();
    inst.preciseWheel.dispose();
    inst.colsResize.dispose();
    inst.jumpLatest.dispose();
    inst.unlistenData?.();
    inst.unlistenExit?.();
    inst.resizeObserver?.disconnect();
    inst.themeObserver?.disconnect();
    invoke("pty_kill", { id: paneId }).catch(() => {});
    // 尺寸账要跟着 pane 一起销毁，否则 paneId 复用时会继承上一条命的账，
    // 第一次 fit 就被判成「没变」而跳过 —— 又是一次 #156
    forgetPane(paneId);
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
