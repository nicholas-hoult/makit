/**
 * 全局快捷键的修饰键规则：**⌃ 属于终端，不属于 app**。
 *
 * 这条不变量承担的是：任何一个终端控制字符（⌃A…⌃Z）都不许被 app 的全局快捷键吃掉。
 * 之所以值得单独测，是因为这个 bug 极易复发 —— `e.metaKey || e.ctrlKey` 是从
 * Windows/Linux 抄来的顺手写法，一处一处散在 9 个 handler 里，靠 review 看不住。
 * 收成一个谓词 + 一张清单，复发时这里直接红。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { isCmd } from "../src/keys.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

/** 终端里有实际含义、必须原样送进 PTY 的 ⌃ 组合（readline / zsh / claude 通用） */
const TERMINAL_CTRL_KEYS: Array<[string, string]> = [
  ["a", "行首"],
  ["b", "后退一字符 / tmux 前缀"],
  ["c", "中断（SIGINT）"],
  ["d", "EOF / 删右边一字符"],
  ["e", "行尾"],
  ["f", "前进一字符"],
  ["g", "取消当前操作"],
  ["i", "Tab 字符（补全）"],
  ["k", "删到行尾"],
  ["l", "清屏"],
  ["n", "下一条历史"],
  ["p", "上一条历史"],
  ["r", "反向历史搜索"],
  ["t", "交换两字符"],
  ["u", "删整行"],
  ["w", "删前一个词"],
  ["y", "粘回删掉的内容"],
  ["z", "挂起（SIGTSTP）"],
];

for (const [k, meaning] of TERMINAL_CTRL_KEYS) {
  check(
    `⌃${k.toUpperCase()}（${meaning}）不许被当成 app 快捷键`,
    !isCmd({ metaKey: false, ctrlKey: true }),
  );
}

// 正面：⌘ 组合必须照旧命中，否则等于把快捷键全废了
check("⌘ 组合仍然命中", isCmd({ metaKey: true, ctrlKey: false }));

// 手滑同按 ⌘⌃ 时什么都不做 —— 本 app 没有任何 ⌘⌃ 组合快捷键
check("⌘⌃ 组合不命中", !isCmd({ metaKey: true, ctrlKey: true }));

// 裸按键不命中（否则每个字母都会触发快捷键）
check("无修饰键不命中", !isCmd({ metaKey: false, ctrlKey: false }));

// ---------------------------------------------------------------------------
// 上面测的是谓词本身。但真正会复发的不是谓词，是**新写的调用点又抄一遍**
// `e.metaKey || e.ctrlKey` —— 那种情况下上面 21 项照样全绿，bug 照样回来。
// 所以这里再扫一遍源码：除了 keys.ts 自己（注释里要引用这个写法）和 ⌃Tab 那处
// 显式例外，src/ 下不许再出现把 ctrl 当 mod 键用的写法。
// ---------------------------------------------------------------------------

/** keys.ts 讲的就是这个反面写法；⌃Tab 例外见 App.tsx 里那段注释 */
const ALLOWED = new Set(["src/keys.ts"]);
/** 已知且刻意的 ⌃ 用法：只按 e.code 判定的 ⌃Tab、以及排除 ctrl 的守卫 */
const DELIBERATE = [
  'e.ctrlKey && !e.metaKey && e.code === "Tab"',
  "!event.ctrlKey",
];

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.tsx?$/.test(name)) out.push(p);
  }
  return out;
}

const offenders: string[] = [];
for (const file of walk("src")) {
  if (ALLOWED.has(file)) continue;
  const lines = readFileSync(file, "utf8").split("\n");
  lines.forEach((line, i) => {
    if (!/ctrlKey/.test(line)) return;
    if (line.trim().startsWith("//") || line.trim().startsWith("*")) return;
    if (DELIBERATE.some((d) => line.includes(d))) return;
    offenders.push(`${file}:${i + 1} ${line.trim()}`);
  });
}
check(
  `src/ 下没有新的 ctrlKey 用法（发现 ${offenders.length} 处：${offenders.join(" | ")}）`,
  offenders.length === 0,
);

if (fails.length) {
  console.error(`✗ 快捷键修饰键规则失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 快捷键修饰键规则全部通过（${pass} 项）`);
