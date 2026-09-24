/**
 * 终端里的链接识别（#214）：本地路径、网址、OSC 8 超链接，以及双击选词。
 * 从 TerminalManager.ts 挪出来是为了能单测 —— 识别错了在界面上是「下划线画歪了」「点了打不开」，
 * 都得盯着终端才看得出来。
 */

// 匹配本地路径（支持中文文件名 + 目录）：
// 1. 含扩展名的文件：foo.md、Lens智能选币器_TRD.md、src/bar.ts:12:34
// 2. 绝对路径或 ~ 路径（含目录）：/Users/foo、~/Pictures
// 3. 显式相对路径：./foo、../bar
// 4. ls -F 风格目录（后缀 /）：Pictures/
// 注：裸标识符（无前缀/后缀/扩展名）不识别，避免误匹配普通单词
export const PATH_REGEX = /(?:\.{0,2}\/|~\/)?(?:[\w一-龥.\-]+\/)*[\w一-龥.\-]+\.[\w]+(?::\d+(?::\d+)?)?|(?:\/|~\/)[\w一-龥.\-/]*[\w一-龥.\-](?::\d+(?::\d+)?)?|\.{1,2}\/[\w一-龥.\-/]*[\w一-龥.\-](?::\d+(?::\d+)?)?|[\w一-龥.\-]+\/(?=\s|$)/g;

// 排除明显的伪路径（版本号、slash 命令、纯数字等）
export function isLikelyPath(text: string): boolean {
  // 全数字+点（版本号 1.14.1 / IP 地址）
  if (/^[\d.]+$/.test(text)) return false;
  // 长度太短（<3 字符）
  if (text.length < 3) return false;
  // /xxx 形式但 xxx 没有任何斜杠也没有扩展名（slash 命令如 /reload-plugins）
  if (/^\/[\w\-]+$/.test(text)) return false;
  return true;
}

// 解析路径文本：剥离行号/列号
export function parsePathText(text: string): { path: string; line?: number; col?: number } {
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
export function resolvePath(text: string, cwd: string): string {
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

/**
 * 双击选词的分隔符，照 对标终端 的 `selection-word-chars` 默认值（对标产品 的内核，#214）。
 * 比 xterm 默认的 `` ()[]{}',"` `` 多了 `: ; , | < > $` 和制表符：双击 `foo.ts:12` 只选 `foo.ts`。
 * `/` 和 `.` 故意不算，双击路径能整体选中。
 */
export const WORD_SEPARATORS = " \t'\"│`|:;,()[]{}<>$";

export type FoundLink = { kind: "url" | "path"; start: number; end: number; text: string };

/** 网址：到空白、引号、尖括号、中文（含全角标点）为止。中文紧挨着网址是常态（「打开https://…看看」） */
const URL_REGEX = /\bhttps?:\/\/[^\s<>"'`　-鿿＀-￯]+/g;

/** 去掉网址尾巴上的句读和不配对的右括号：「(见 https://x.com/a).」里的 `).` 不属于网址，维基那种配对的括号要留 */
function trimUrl(u: string): string {
  const pairs: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
  let s = u;
  while (s.length) {
    const last = s[s.length - 1];
    if (".,;:!?'\"".includes(last)) { s = s.slice(0, -1); continue; }
    const open = pairs[last];
    if (open && s.split(last).length > s.split(open).length) { s = s.slice(0, -1); continue; }
    break;
  }
  return s;
}

/**
 * 在一行文字里找网址和本地路径，返回字符串下标（`end` 不含）。
 * 网址优先：和网址重叠的路径匹配丢掉 —— 以前没有网址识别，`github.com/a/b` 会被当成相对路径，点了打不开。
 */
export function findLinks(text: string): FoundLink[] {
  const out: FoundLink[] = [];
  for (const m of text.matchAll(URL_REGEX)) {
    const t = trimUrl(m[0]);
    if (/^https?:\/\/./.test(t)) out.push({ kind: "url", start: m.index!, end: m.index! + t.length, text: t });
  }
  const re = new RegExp(PATH_REGEX.source, "g");
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const start = m.index, end = start + m[0].length;
    if (!isLikelyPath(m[0])) continue;
    if (out.some((l) => l.kind === "url" && start < l.end && end > l.start)) continue;
    out.push({ kind: "path", start, end, text: m[0] });
  }
  return out.sort((a, b) => a.start - b.start);
}

/**
 * OSC 8 超链接（程序主动嵌在输出里的链接）指向哪。只放行 http / https 和 `file://`，
 * 其余（`javascript:` 之类）一律不认 —— 这是程序输出里带来的地址，不能什么都拿去打开。
 */
export function osc8Target(uri: string): { kind: "url"; url: string } | { kind: "file"; path: string } | null {
  let u: URL;
  try { u = new URL(uri); } catch { return null; }
  if (u.protocol === "http:" || u.protocol === "https:") return { kind: "url", url: uri };
  if (u.protocol === "file:") {
    const path = decodeURIComponent(u.pathname);
    return path ? { kind: "file", path } : null;
  }
  return null;
}

/** 终端一行里的一个格子：宽字符（汉字、emoji）占两格，第二格 `width === 0`、没有字 */
export type Cell = { chars: string; width: number };

/**
 * 把一行格子拼成字符串，同时给出「字符串第 i 个 UTF-16 单位在第几格」。
 * 以前链接位置直接拿字符串下标当列号，一个汉字占两格，于是前面有中文时下划线整体往左偏。
 */
export function cellsToText(cells: Cell[]): { text: string; colOf: number[] } {
  let text = "";
  const colOf: number[] = [];
  cells.forEach((c, col) => {
    if (c.width === 0) return;
    const ch = c.chars || " ";
    for (let k = 0; k < ch.length; k++) { text += ch[k]; colOf.push(col); }
  });
  return { text, colOf };
}

const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u;

export type Segmenter = (s: string) => { index: number; segment: string }[];

/**
 * 双击中文时按词选（#214）：点到的是中日韩文字，就用分词器找出它所在的词，返回要选中的格子范围；
 * 点到别的返回 null，交回 xterm 按分隔符的默认规则。分词器注入进来，测试里用固定切法。
 */
export function cjkWordRange(cells: Cell[], col: number, segment: Segmenter): { startCol: number; cells: number } | null {
  let base = col;
  if (cells[base]?.width === 0 && base > 0) base -= 1;          // 点在宽字符的右半格
  const cell = cells[base];
  if (!cell || !CJK.test(cell.chars)) return null;
  const { text, colOf } = cellsToText(cells);
  const i = colOf.indexOf(base);
  if (i < 0) return null;
  const seg = segment(text).find((s) => i >= s.index && i < s.index + s.segment.length);
  if (!seg) return null;
  const startCol = colOf[seg.index];
  const lastCol = colOf[seg.index + seg.segment.length - 1];
  return { startCol, cells: lastCol + (cells[lastCol]?.width || 1) - startCol };
}

/** 一行终端的格子，`y` 是缓冲区里的行号（从 0 开始） */
export type Row = { y: number; cells: Cell[] };

export type RowLink = {
  kind: FoundLink["kind"];
  text: string;
  /** xterm 链接范围的约定：x、y 从 1 开始，end 含 */
  start: { x: number; y: number };
  end: { x: number; y: number };
  /** 在拼接后的格子序列里从第几格开始、占几格（给双击选中用） */
  startCell: number;
  cells: number;
};

/**
 * 在「一整行逻辑行」（被折成几行显示的那一行，按顺序传进来）里找链接，位置按格子换算回行、列。
 * 以前逐行识别，折到下一行的网址被当成两段，哪段都不认（用户 2026-09-25 反馈）。
 */
export function rowLinks(rows: Row[]): RowLink[] {
  const cells: Cell[] = [];
  const pos: { x: number; y: number }[] = [];
  for (const r of rows) r.cells.forEach((c, x) => { cells.push(c); pos.push({ x, y: r.y }); });
  const { text, colOf } = cellsToText(cells);
  return findLinks(text).map((l) => {
    const startCell = colOf[l.start];
    const lastCell = colOf[l.end - 1];
    const endCell = lastCell + (cells[lastCell]?.width || 1) - 1;
    return {
      kind: l.kind,
      text: l.text,
      start: { x: pos[startCell].x + 1, y: pos[startCell].y + 1 },
      end: { x: pos[endCell].x + 1, y: pos[endCell].y + 1 },
      startCell,
      cells: endCell - startCell + 1,
    };
  });
}

/**
 * 双击落在链接（网址或路径）上时，选中整个链接，跨几行都行（用户 2026-09-25 反馈：折行的网址双击选不中）。
 * `col`、`row` 是被点的格子（从 0 开始，row 是缓冲区行号）；返回值直接喂 `terminal.select(col, row, length)`。
 * 不在链接上返回 null，交回别的规则（中文按词、其余按分隔符）。
 */
export function linkSelectionAt(rows: Row[], col: number, row: number): { col: number; row: number; length: number } | null {
  let offset = 0;
  let hit = -1;
  for (const r of rows) {
    if (r.y === row) { hit = offset + col; if (r.cells[col]?.width === 0 && col > 0) hit -= 1; break; }
    offset += r.cells.length;
  }
  if (hit < 0) return null;
  const link = rowLinks(rows).find((l) => hit >= l.startCell && hit < l.startCell + l.cells);
  if (!link) return null;
  // 路径选到文件名为止，不带末尾的 `:行号:列号`：双击是为了拿到能用的路径，`foo.ts:12` 贴进 cd / open 都不认。
  // 行列号都是 ASCII，一个字符占一格，直接按字符数扣
  const suffix = link.kind === "path" ? (link.text.match(/(?::\d+){1,2}$/)?.[0].length ?? 0) : 0;
  return { col: link.start.x - 1, row: link.start.y - 1, length: link.cells - suffix };
}
