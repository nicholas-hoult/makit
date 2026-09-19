/**
 * ⌘F 终端内搜索的计数文本。
 *
 * 值得单独测的原因全在边界上：xterm 的 `onDidChangeResults` 给的是
 * `{ resultIndex, resultCount }`，其中 **`resultIndex` 是 0-based，而且匹配数超过
 * 高亮上限（highlightLimit，默认 1000）时是 `-1`**。这两条任一处理错，用户看到的
 * 就是「0/17」这种差一错误，或者一个匹配上千次的搜索显示「0/1234」。
 */
import { formatSearchCount } from "../src/searchCount.ts";

let pass = 0;
const fails: string[] = [];

function eq(name: string, actual: unknown, expected: unknown) {
  if (actual === expected) pass++;
  else fails.push(`${name}\n      期望 ${JSON.stringify(expected)}\n      实际 ${JSON.stringify(actual)}`);
}

// ---- 正常情况：0-based → 1-based ----------------------------------------
// 这是最容易写错的一条：addon 给 0 表示"第一个"，直接显示就成了「0/17」。
eq("第一个匹配显示 1/17", formatSearchCount("foo", { index: 0, count: 17 }), "1/17");
eq("中间的匹配", formatSearchCount("foo", { index: 4, count: 17 }), "5/17");
eq("最后一个匹配", formatSearchCount("foo", { index: 16, count: 17 }), "17/17");
eq("只有一个匹配", formatSearchCount("foo", { index: 0, count: 1 }), "1/1");

// ---- 没有匹配 ------------------------------------------------------------
// 此时 addon 给的是 { resultIndex: -1, resultCount: 0 }，所以 count===0 的判断
// 必须排在 index<0 的判断**前面**，否则会显示「0 个结果」而不是「无结果」。
eq("没有匹配显示无结果", formatSearchCount("zzz", { index: -1, count: 0 }), "无结果");

// ---- 超出高亮上限：index 是 -1，说不出当前是第几个 ------------------------
eq(
  "匹配数超过高亮上限时只报总数",
  formatSearchCount("a", { index: -1, count: 1000 }),
  "1000 个结果",
);

// ---- 什么都不显示的情况 --------------------------------------------------
// 搜索条刚按 ⌘F 打开、还没输入时不该先闪一个「无结果」出来。
eq("搜索词为空 → 不显示", formatSearchCount("", { index: -1, count: 0 }), null);
eq("搜索词只有空白 → 不显示", formatSearchCount("  ", { index: -1, count: 0 }), null);
eq("还没收到任何结果事件 → 不显示", formatSearchCount("foo", null), null);

// ---- 防御：index 越界时退回只报总数，不显示 18/17 -------------------------
eq(
  "index 越界时退回只报总数",
  formatSearchCount("foo", { index: 17, count: 17 }),
  "17 个结果",
);

if (fails.length) {
  console.error(`✗ 搜索计数文本失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 搜索计数文本全部通过（${pass} 项）`);
