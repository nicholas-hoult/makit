/**
 * 从 Finder 拖文件进终端 → 在命令行里插入文件路径。
 *
 * 为什么要解析 URL 而不是直接拿路径：`tauri.conf.json` 里 `dragDropEnabled: false`，
 * 走的是 HTML5 拖拽（这是刻意的 —— 打开原生 onDragDrop 会**关掉整个 HTML5 拖拽**，
 * tab 拖动和拖边缘分屏全废）。而 WebKit 里 `DataTransfer.files` 给的 `File` 对象
 * **没有 `.path`**，所以真实路径只能从 `text/uri-list` 的 `file://` URL 里解出来。
 *
 * 抽成独立模块是为了能测两类边界：百分号解码、以及要不要加引号。两者错了都不是
 * 「不好看」，而是往用户的命令行里插进一个错的东西。
 */

/** 拖进来的东西里能读出路径的两个 MIME，按优先级排列 */
const URI_LIST = "text/uri-list";
const PLAIN = "text/plain";

/**
 * 单行 → 绝对路径，不是文件就返回 null。
 *
 * 拖网页链接进来也会给 `text/uri-list`，所以「不是 file://」必须挡掉 —— 否则
 * 会把一个 http 地址当路径插进去。
 */
export function fileUrlToPath(raw: string): string | null {
  // uri-list 按 RFC 2483 是 CRLF 分隔，行尾的 \r 得吃掉
  const s = raw.trim();
  if (!s) return null;
  // 有些来源（拖到 text/plain 的场景）直接给裸路径，不用解码
  if (s.startsWith("/")) return s;
  if (!s.toLowerCase().startsWith("file://")) return null;
  let rest = s.slice("file://".length);
  // file:///path 和 file://localhost/path 都合法，后者要把 host 摘掉
  if (rest.toLowerCase().startsWith("localhost/")) rest = rest.slice("localhost".length);
  if (!rest.startsWith("/")) return null;
  try {
    // 空格是 %20、中文是一串 %E6%96%87，不解码就会把字面的 % 序列插进终端
    return decodeURIComponent(rest);
  } catch {
    // 坏的百分号编码会抛，宁可不插也不要插一个半解码的路径
    return null;
  }
}

/**
 * shell 里有特殊含义的字符。含了就整体套单引号。
 *
 * **这里刻意用黑名单而不是白名单**（`shlex.quote` 那种「只有 ASCII 安全字符才不引」）：
 * 非 ASCII 在 shell 里就是普通字符，白名单会把每一个中文文件名都套上引号 —— 正确但
 * 满屏噪音。清单包含 POSIX 元字符、glob、大括号展开，以及 `!`（交互式 shell 的历史
 * 展开，我们插进去的正是交互式 shell）。
 */
const SHELL_SPECIAL = /[\s|&;<>()$`\\"'*?[\]{}~!#]/;

/** 按 POSIX 规则给路径加引号。单引号串里唯一需要处理的就是单引号本身。 */
export function quoteForShell(path: string): string {
  if (!SHELL_SPECIAL.test(path)) return path;
  return "'" + path.replace(/'/g, "'\\''") + "'";
}

/**
 * 从 DataTransfer 里把所有文件路径抠出来。
 *
 * 收 `getData` 而不是收两个字符串，是为了把「读哪个 MIME」这个知识也关在本模块里，
 * 同时还能用一个假对象测。
 */
export function parseDroppedPaths(dt: { getData(type: string): string }): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  const collect = (blob: string) => {
    for (const line of blob.split(/\r?\n/)) {
      // RFC 2483：# 开头是注释
      if (line.trim().startsWith("#")) continue;
      const p = fileUrlToPath(line);
      if (p && !seen.has(p)) {
        seen.add(p);
        out.push(p);
      }
    }
  };
  collect(dt.getData(URI_LIST));
  // uri-list 是首选；只有它什么都没给出来时才看 text/plain，避免同一批文件解析两遍
  if (out.length === 0) collect(dt.getData(PLAIN));
  return out;
}

/**
 * 拼成要写进 PTY 的文本。空数组返回空串，调用方据此判断「这次拖拽没东西可插」。
 *
 * 尾随空格是终端惯例（Terminal.app / iTerm2 都加）：插完能直接接着打下一个参数。
 */
export function formatPathsForTerminal(paths: string[]): string {
  if (paths.length === 0) return "";
  return paths.map(quoteForShell).join(" ") + " ";
}
