/**
 * 拖文件进终端 → 插入路径。
 *
 * 两件事值得测，都不是「好不好看」而是「会不会插错东西」：
 *
 * 1. **路径怎么拿到**。`dragDropEnabled: false`（HTML5 拖拽，必须保留 —— 换成 Tauri
 *    原生 onDragDrop 会关掉整个 HTML5 拖拽，分屏拖拽全废），而 WebKit 的 `File` 对象
 *    **没有 `.path`**。所以只能解析 `text/uri-list` 里的 `file://` URL，那意味着要处理
 *    百分号编码（空格是 `%20`、中文是一串 `%E6%96%87`）。
 * 2. **要不要加引号**。插进去的路径下一步会被 shell 解析，带空格不加引号就断成两个参数。
 *    但反过来**不能一律加引号** —— 中文文件名满屏都是，套上引号又丑又没必要（非 ASCII
 *    在 shell 里是普通字符）。所以判定必须按「shell 元字符黑名单」而不是「ASCII 白名单」。
 */
import {
  fileUrlToPath,
  quoteForShell,
  parseDroppedPaths,
  formatPathsForTerminal,
} from "../src/dropPaths.ts";

let pass = 0;
const fails: string[] = [];

function eq(name: string, actual: unknown, expected: unknown) {
  if (actual === expected) pass++;
  else fails.push(`${name}\n      期望 ${JSON.stringify(expected)}\n      实际 ${JSON.stringify(actual)}`);
}

/** 假 DataTransfer：只需要 getData */
function dt(map: Record<string, string>) {
  return { getData: (t: string) => map[t] ?? "" };
}

// ---- file:// URL → 路径 --------------------------------------------------
eq("普通 file URL", fileUrlToPath("file:///Users/me/a.txt"), "/Users/me/a.txt");
eq(
  "空格是 %20，必须解码 —— 不解码会插进去一个字面的 %20",
  fileUrlToPath("file:///Users/me/My%20File.txt"),
  "/Users/me/My File.txt",
);
eq(
  "中文文件名（百分号编码的 UTF-8）",
  fileUrlToPath("file:///Users/me/%E6%96%87%E6%A1%A3.txt"),
  "/Users/me/文档.txt",
);
eq(
  "file://localhost/… 也合法，host 要摘掉",
  fileUrlToPath("file://localhost/Users/me/a.txt"),
  "/Users/me/a.txt",
);
eq("行尾的 CRLF 要吃掉（uri-list 按 RFC 是 CRLF 分隔）", fileUrlToPath("file:///a.txt\r"), "/a.txt");
eq("有些来源直接给裸路径", fileUrlToPath("/Users/me/a.txt"), "/Users/me/a.txt");
// 拖链接进来不该变成一个路径插进终端
eq("http 链接不是路径", fileUrlToPath("https://example.com/x"), null);
eq("空串", fileUrlToPath("  "), null);
eq("file:// 后面没有路径", fileUrlToPath("file://"), null);
// 坏的百分号编码会让 decodeURIComponent 抛异常，不能把异常漏出去
eq("坏的百分号编码不抛异常，返回 null", fileUrlToPath("file:///a%zz"), null);

// ---- 引号 ----------------------------------------------------------------
eq("普通路径不加引号", quoteForShell("/Users/me/a.txt"), "/Users/me/a.txt");
eq("带空格必须加引号", quoteForShell("/Users/me/My File.txt"), "'/Users/me/My File.txt'");
// 这条是本模块最容易写错的地方：用 ASCII 白名单判定的话中文路径会被无谓地套上引号
eq("中文路径不加引号（非 ASCII 在 shell 里是普通字符）", quoteForShell("/Users/me/文档.txt"), "/Users/me/文档.txt");
eq(
  "路径里有单引号：单引号串里要用 '\\'' 拼接",
  quoteForShell("/Users/me/it's.txt"),
  "'/Users/me/it'\\''s.txt'",
);
eq("$ 要加引号（否则被当变量展开）", quoteForShell("/Users/me/a$b"), "'/Users/me/a$b'");
eq("& 要加引号（否则被当后台运行）", quoteForShell("/Users/me/a&b"), "'/Users/me/a&b'");
eq("! 要加引号（交互式 shell 的历史展开）", quoteForShell("/Users/me/a!b"), "'/Users/me/a!b'");
eq("* 要加引号（glob）", quoteForShell("/Users/me/a*b"), "'/Users/me/a*b'");
eq("这些常见字符不必加引号", quoteForShell("/Users/me/a-b_c.1,2@3=4"), "/Users/me/a-b_c.1,2@3=4");

// ---- 从 DataTransfer 解析 ------------------------------------------------
eq(
  "uri-list 多行 → 多个路径",
  JSON.stringify(parseDroppedPaths(dt({ "text/uri-list": "file:///a.txt\r\nfile:///b.txt" }))),
  JSON.stringify(["/a.txt", "/b.txt"]),
);
eq(
  "uri-list 里 # 开头是注释（RFC 2483），要跳过",
  JSON.stringify(parseDroppedPaths(dt({ "text/uri-list": "# comment\r\nfile:///a.txt" }))),
  JSON.stringify(["/a.txt"]),
);
eq(
  "uri-list 空时退回 text/plain",
  JSON.stringify(parseDroppedPaths(dt({ "text/plain": "/Users/me/a.txt" }))),
  JSON.stringify(["/Users/me/a.txt"]),
);
eq(
  "两处都给同一个文件时去重",
  JSON.stringify(
    parseDroppedPaths(dt({ "text/uri-list": "file:///a.txt\nfile:///a.txt" })),
  ),
  JSON.stringify(["/a.txt"]),
);
eq("什么都没有 → 空数组", JSON.stringify(parseDroppedPaths(dt({}))), JSON.stringify([]));
eq(
  "拖的是网页链接 → 空数组（不插东西，交给别的分支或什么都不做）",
  JSON.stringify(parseDroppedPaths(dt({ "text/uri-list": "https://example.com/" }))),
  JSON.stringify([]),
);

// ---- 拼成要写进 PTY 的文本 -----------------------------------------------
// 尾随空格是终端惯例（Terminal.app / iTerm2 都加）：插完能直接接着打字。
eq("单个文件，带尾随空格", formatPathsForTerminal(["/a.txt"]), "/a.txt ");
eq("多个文件用空格分隔", formatPathsForTerminal(["/a.txt", "/b.txt"]), "/a.txt /b.txt ");
eq(
  "多个文件各自判断要不要引号",
  formatPathsForTerminal(["/a b.txt", "/c.txt"]),
  "'/a b.txt' /c.txt ",
);
eq("空数组 → 空串（调用方据此判断不写）", formatPathsForTerminal([]), "");

if (fails.length) {
  console.error(`✗ 拖入路径失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 拖入路径全部通过（${pass} 项）`);
