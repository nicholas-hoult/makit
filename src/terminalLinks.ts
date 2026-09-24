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
