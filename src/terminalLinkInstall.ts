/**
 * 终端的链接和双击（#214）：网址 / 本地路径识别（整条逻辑行，折行也认）、⌘+点击打开，
 * 以及双击点在链接上选整个链接、点在中文上按词选。
 *
 * 从 TerminalManager 挪出来是为了让真 WKWebView 的验证工具能直接加载**同一份代码**
 * （private/scripts/webkit/），不在测试里另抄一份 —— 抄的那份对了不代表这份对。
 * 纯逻辑（识别、换算、分词范围）在 terminalLinks.ts，那里有单测。
 */
import type { IDisposable, Terminal as Xterm } from "@xterm/xterm";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isCmd } from "./keys";
import { parsePathText, resolvePath, rowLinks, linkSelectionAt, cjkWordRange, type Cell, type Row } from "./terminalLinks";

/**
 * 把缓冲区第 y 行所在的**整条逻辑行**读出来（被折成几行显示的那一行，#214）。
 * 链接识别和双击选链接都要在整行上做：以前逐行识别，折到下一行的网址被当成两段，哪段都不认。
 * 最多往上下各摸 50 行，防止一条超长输出拖慢悬停。
 */
function logicalRows(term: Xterm, y: number): Row[] {
  const buf = term.buffer.active;
  let top = y;
  while (top > 0 && y - top < 50 && buf.getLine(top)?.isWrapped) top--;
  let bottom = y;
  while (bottom - y < 50 && buf.getLine(bottom + 1)?.isWrapped) bottom++;
  const rows: Row[] = [];
  const scratch = buf.getNullCell();
  for (let r = top; r <= bottom; r++) {
    const line = buf.getLine(r);
    if (!line) continue;
    const cells: Cell[] = [];
    for (let x = 0; x < line.length; x++) {
      const c = line.getCell(x, scratch);
      cells.push({ chars: c?.getChars() ?? "", width: c?.getWidth() ?? 1 });
    }
    rows.push({ y: r, cells });
  }
  return rows;
}

/** 双击中文按词选用的系统分词（#214）。WebKit 自带 ICU 词典 */
const zhSegmenter = new Intl.Segmenter("zh", { granularity: "word" });
const segmentZh = (s: string) => [...zhSegmenter.segment(s)].map((x) => ({ index: x.index, segment: x.segment }));

/** 把本地路径（可能带 :行:列）按终端的当前目录解析后交给系统打开：文件用默认程序，目录进 Finder */
function openLocalPath(text: string, cwd: string) {
  const { path } = parsePathText(text);
  invoke("open_path", { path: resolvePath(path, cwd), reveal: false }).catch(() => {});
}

/** 候选路径存在与否的缓存：悬停一行就要查一批，5 秒内不重复扫盘；目录刚建 / 刚删的，最多晚 5 秒反映出来 */
const existCache = new Map<string, { ok: boolean; at: number }>();
const EXIST_TTL_MS = 5000;

async function existingPaths(paths: string[]): Promise<Set<string>> {
  const now = Date.now();
  if (existCache.size > 2000) existCache.clear();
  const need = [...new Set(paths)].filter((p) => {
    const c = existCache.get(p);
    return !c || now - c.at > EXIST_TTL_MS;
  });
  if (need.length) {
    try {
      const res = await invoke<boolean[]>("paths_exist", { paths: need });
      need.forEach((p, i) => existCache.set(p, { ok: !!res[i], at: now }));
    } catch {
      need.forEach((p) => existCache.set(p, { ok: false, at: now }));
    }
  }
  return new Set(paths.filter((p) => existCache.get(p)?.ok));
}

export function installTerminalLinks(terminal: Xterm, element: HTMLElement, getCwd: () => string): IDisposable {
  // 网址和本地路径，在整条逻辑行上认（折行的也行）。
  // 按住 ⌘ 才画下划线（class 由全局 keydown 切换），⌘+点击打开，同 对标终端 / 对标产品。
  const provider = terminal.registerLinkProvider({
    provideLinks: (lineNumber, callback) => {
      const cmdPressed = document.body.classList.contains("cmd-pressed");
      const found = rowLinks(logicalRows(terminal, lineNumber - 1))
        .filter((l) => l.start.y <= lineNumber && l.end.y >= lineNumber);
      const abs = (text: string) => resolvePath(parsePathText(text).path, getCwd());
      const build = (existing: Set<string>) => {
        const links = found
          // 候选（裸目录名、不带斜杠结尾的相对目录）存在才算链接，见 terminalLinks.ts 的 verify
          .filter((l) => !l.verify || existing.has(abs(l.text)))
          .map((l) => ({
            range: { start: l.start, end: l.end },
            text: l.text,
            decorations: { underline: cmdPressed, pointerCursor: cmdPressed },
            activate: (e: MouseEvent, t: string) => {
              if (!isCmd(e)) return;
              if (l.kind === "url") openUrl(t).catch(() => {});
              else openLocalPath(t, getCwd());
            },
          }));
        callback(links.length ? links : undefined);
      };
      const toCheck = found.filter((l) => l.verify).map((l) => abs(l.text));
      if (toCheck.length === 0) build(new Set());
      else existingPaths(toCheck).then(build);
    },
  });

  // 双击：点在链接上选整个链接（跨行也行）；点在中文上按词选；其余交给 xterm 按分隔符选（#214）。
  // xterm 自己没做选择时不插手 —— 那是程序（比如 claude 的全屏界面）接管了鼠标，双击归它。
  const onDblClick = (e: MouseEvent) => {
    if (!terminal.hasSelection()) return;
    const cellSize = (terminal as unknown as {
      _core?: { _renderService?: { dimensions?: { css?: { cell?: { width: number; height: number } } } } };
    })._core?._renderService?.dimensions?.css?.cell;
    const screen = element.querySelector(".xterm-screen");
    if (!cellSize?.width || !cellSize.height || !screen) return;
    const rect = screen.getBoundingClientRect();
    const col = Math.floor((e.clientX - rect.left) / cellSize.width);
    const row = Math.floor((e.clientY - rect.top) / cellSize.height) + terminal.buffer.active.viewportY;
    if (col < 0 || col >= terminal.cols) return;
    const rows = logicalRows(terminal, row);
    const link = linkSelectionAt(rows, col, row);
    if (link) { terminal.select(link.col, link.row, link.length); return; }
    const here = rows.find((r) => r.y === row);
    const word = here && cjkWordRange(here.cells, col, segmentZh);
    if (word) terminal.select(word.startCol, row, word.cells);
  };
  element.addEventListener("dblclick", onDblClick);

  return {
    dispose() {
      provider.dispose();
      element.removeEventListener("dblclick", onDblClick);
    },
  };
}
