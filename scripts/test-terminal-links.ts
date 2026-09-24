/**
 * 终端里的链接识别与双击选词（#214）。
 *
 * 为什么单独测：
 * - 网址以前根本不识别，还会被路径正则切走一截（`github.com/a/b` 当成相对路径），点了打不开；
 * - OSC 8 链接要把 `file://` 解成本地路径，还得拒绝 `javascript:` 这类协议 —— 错了就是安全问题；
 * - 链接位置以前按「字符串下标」算，而一个汉字占两格，前面有中文时下划线整体往左偏；
 * - 双击中文要按词选，这得在「终端格子」和「字符串」之间来回换算，宽字符算错就选歪。
 * 这些在界面上都得盯着终端才看得出来。
 */
import { findLinks, osc8Target, cellsToText, cjkWordRange, WORD_SEPARATORS, rowLinks, linkSelectionAt } from "../src/terminalLinks.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}
const texts = (s: string) => findLinks(s).map((l) => `${l.kind}:${l.text}`);

// ── 网址 ──
eq("网址和路径各认各的", texts("see https://github.com/a/b and src/foo.ts:12"), ["url:https://github.com/a/b", "path:src/foo.ts:12"]);
eq("网址不会再被路径正则切走一截", texts("https://example.com/docs/x.md"), ["url:https://example.com/docs/x.md"]);
eq("句尾标点和外层括号不算进网址", texts("(见 https://x.com/a)."), ["url:https://x.com/a"]);
eq("网址里配对的括号保留", texts("https://en.wikipedia.org/wiki/A_(b)"), ["url:https://en.wikipedia.org/wiki/A_(b)"]);
eq("紧挨着中文也能切干净", texts("打开https://x.com/a看看"), ["url:https://x.com/a"]);
eq("位置是字符串下标（https://a.io 共 12 个字符）", findLinks("x https://a.io y").map((l) => [l.start, l.end]), [[2, 14]]);

// ── 本地路径（原有行为不退化）──
eq("绝对路径", texts("cd /Users/me/proj"), ["path:/Users/me/proj"]);
eq("~ 路径", texts("open ~/Pictures"), ["path:~/Pictures"]);
eq("显式相对路径", texts("./a/b"), ["path:./a/b"]);
eq("ls -F 风格目录", texts("Pictures/"), ["path:Pictures/"]);
eq("中文文件名", texts("看 设计稿_v2.md"), ["path:设计稿_v2.md"]);
eq("版本号不是路径", texts("v 1.14.1"), []);
eq("slash 命令不是路径", texts("/reload-plugins"), []);

// ── OSC 8 ──
eq("OSC 8 网址", osc8Target("https://a.com/x"), { kind: "url", url: "https://a.com/x" });
eq("OSC 8 file:// 解成本地路径（含转义）", osc8Target("file:///Users/me/a%20b.ts"), { kind: "file", path: "/Users/me/a b.ts" });
eq("OSC 8 file:// 带主机名", osc8Target("file://myhost/Users/me/x"), { kind: "file", path: "/Users/me/x" });
eq("拒绝 javascript: 协议", osc8Target("javascript:alert(1)"), null);
eq("拒绝乱写的", osc8Target("not a uri"), null);

// ── 格子 ↔ 字符串：一个汉字占两格（第二格是宽度 0 的占位）──
const cell = (c: string, w: number) => ({ chars: c, width: w });
const zh = (s: string) => [...s].flatMap((c) => /[　-鿿＀-￯]/.test(c) ? [cell(c, 2), cell("", 0)] : [cell(c, 1)]);
eq("格子转字符串", cellsToText(zh("打开 a")), { text: "打开 a", colOf: [0, 2, 4, 5] });
eq("emoji（代理对）占一格两个 UTF-16 单位", cellsToText([cell("😀", 2), cell("", 0), cell("a", 1)]), { text: "😀a", colOf: [0, 0, 2] });

// ── 双击中文按词选（分词器注入，结果确定）──
const fakeSeg = (s: string) => {
  const words = ["打开", "滚动条", "拖不动", " ", "x"];
  const out: { index: number; segment: string }[] = [];
  let i = 0;
  for (const w of words) { const j = s.indexOf(w, i); if (j < 0) continue; out.push({ index: j, segment: w }); i = j + w.length; }
  return out;
};
const line = zh("打开滚动条拖不动 x");
// 「滚动条」从第 4 格开始（打开占 0~3），共 3 个字 = 6 格；点「动」（第 6 格）
eq("点中文词里任意一格，选中整个词", cjkWordRange(line, 6, fakeSeg), { startCol: 4, cells: 6 });
eq("点在汉字的右半格（占位格）也一样", cjkWordRange(line, 7, fakeSeg), { startCol: 4, cells: 6 });
eq("点英文不管，交给 xterm 默认规则", cjkWordRange(line, 17, fakeSeg), null);
// 真分词器：只要求选中范围包住点的那个字（不同系统词典切法可能不同）
const real = (s: string) => [...new Intl.Segmenter("zh", { granularity: "word" }).segment(s)].map((x) => ({ index: x.index, segment: x.segment }));
const r = cjkWordRange(line, 6, real);
eq("真分词器也能给出包住所点汉字的范围", r !== null && r.startCol <= 6 && r.startCol + r.cells > 6, true);

// ── 双击分隔符照 对标终端 ──
eq("分隔符照 对标终端 的 selection-word-chars", WORD_SEPARATORS, " \t'\"│`|:;,()[]{}<>$");

// ── 折行（用户 2026-09-25 追加）：网址折到下一行，要当成一个整体识别和选中 ──
// 终端 10 列宽；「see https://a.io/abcdef x」折成三行：
//   第 5 行: "see https:"   第 6 行（续行）: "//a.io/abcd"的前 10 格   第 7 行（续行）: 剩下的
const row = (y: number, s: string) => ({ y, cells: [...s.padEnd(10, " ")].map((c) => cell(c, 1)) });
const wrapped = [row(5, "see https:"), row(6, "//a.io/abc"), row(7, "def x")];
const links = rowLinks(wrapped);
eq("折行的网址认成一整个", links.map((l) => `${l.kind}:${l.text}`), ["url:https://a.io/abcdef"]);
// 链接范围沿用 xterm 的约定：x、y 从 1 开始，end 含
eq("起点在第 5 行第 5 格，终点在第 7 行第 3 格", [links[0].start, links[0].end], [{ x: 5, y: 6 }, { x: 3, y: 8 }]);
// 双击折行网址的任何一截，都选中整个网址（xterm.select 的列、行从 0 开始，长度按格子数跨行）
eq("双击第二行那截", linkSelectionAt(wrapped, 3, 6), { col: 4, row: 5, length: 19 });
eq("双击第一行那截（https: 的冒号以前是分隔符，只能选一半）", linkSelectionAt(wrapped, 6, 5), { col: 4, row: 5, length: 19 });
eq("双击非链接处不管", linkSelectionAt(wrapped, 1, 5), null);

// 路径链接双击：选整个路径，但不带末尾的 :行号:列号（真引擎验证时发现选成了整个 foo.ts:12）
const pathRow = [row(3, "foo.ts:12 "), row(4, "")].slice(0, 1);
eq("双击 foo.ts:12 的 foo 只选 foo.ts", linkSelectionAt(pathRow, 1, 3), { col: 0, row: 3, length: 6 });
eq("双击带行列号的深路径，选到文件名为止", linkSelectionAt([row(0, "a/b.ts:3:4")], 2, 0), { col: 0, row: 0, length: 6 });
eq("点在 :12 上也按路径算，同样不带行号", linkSelectionAt(pathRow, 7, 3), { col: 0, row: 3, length: 6 });

console.log(`✓ 终端链接识别与双击选词全部通过（${n} 项）`);
