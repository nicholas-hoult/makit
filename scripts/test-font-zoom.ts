/**
 * 字号缩放的按键判定 + 夹取逻辑。纯函数单测——键盘事件本身没法在这跑，
 * 但「⌘= 该不该触发」「连按到边界该不该再变」这两条规则的正确性完全
 * 由这两个函数决定，钉在这里比在真终端里手按快捷键回归划算得多。
 */
import {
  DEFAULT_FONT_SIZE,
  MIN_FONT_SIZE,
  MAX_FONT_SIZE,
  zoomAction,
  nextFontSize,
  type ZoomKey,
} from "../src/fontZoom.ts";
import { readFileSync } from "node:fs";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  check(`${name}（得到 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}）`, actual === expected);
}

// 造一个「干净」的按键事件，测试只覆盖要断言的字段
function key(code: string, mods: Partial<ZoomKey> = {}): ZoomKey {
  return { code, metaKey: true, ctrlKey: false, altKey: false, ...mods };
}

// ---- 1. 常量 --------------------------------------------------------------

eq("默认字号是 13", DEFAULT_FONT_SIZE, 13);
eq("下限是 8", MIN_FONT_SIZE, 8);
eq("上限是 32", MAX_FONT_SIZE, 32);

// ---- 2. 按键识别：正例 ------------------------------------------------------

eq("⌘=  → in", zoomAction(key("Equal")), "in");
// shiftKey 不在 ZoomKey 里、也不参与判定：⌘= 和 ⌘⇧+ 在 macOS 上是同一个物理键
// （e.code 都是 "Equal"），调用方不需要区分，所以这里不用单独测 shiftKey=true 的情形。
eq("小键盘 ⌘+ → in", zoomAction(key("NumpadAdd")), "in");
eq("⌘− → out", zoomAction(key("Minus")), "out");
eq("小键盘 ⌘− → out", zoomAction(key("NumpadSubtract")), "out");
eq("⌘0 → reset", zoomAction(key("Digit0")), "reset");
eq("小键盘 ⌘0 → reset", zoomAction(key("Numpad0")), "reset");

// ---- 3. 按键识别：反例 ------------------------------------------------------

eq("裸 = （无 meta）→ null", zoomAction(key("Equal", { metaKey: false })), null);
eq("⌘⌥= （带 alt）→ null，避免撞 App.tsx 的 ⌘⌥ 快捷键", zoomAction(key("Equal", { altKey: true })), null);
eq("⌘⌃= （带 ctrl）→ null，⌘⌃ 组合在本项目一律不触发任何动作", zoomAction(key("Equal", { ctrlKey: true })), null);
eq("⌘A（无关键位）→ null", zoomAction(key("KeyA")), null);

// ---- 4. 夹取：上下限 --------------------------------------------------------

eq("13 + in = 14", nextFontSize(13, "in"), 14);
eq("13 + out = 12", nextFontSize(13, "out"), 12);
eq("8 + out 停在 8（撞下限）", nextFontSize(8, "out"), 8);
eq("32 + in 停在 32（撞上限）", nextFontSize(32, "in"), 32);
check("撞下限时返回值等于入参——调用方靠这个判断要不要跳过 fit()", nextFontSize(MIN_FONT_SIZE, "out") === MIN_FONT_SIZE);
check("撞上限时返回值等于入参", nextFontSize(MAX_FONT_SIZE, "in") === MAX_FONT_SIZE);

// ---- 5. reset ---------------------------------------------------------------

eq("任意字号 reset → 13", nextFontSize(20, "reset"), 13);
eq("已经是 13 时 reset 还是 13（等于入参，不应触发 fit）", nextFontSize(13, "reset"), 13);

// ---- 6. 静态守卫：源码层面钉住两条只有读代码才能验证的约束 -------------------
//
// 「zoom() 必须复用 fit() 做 resize 记账」和「默认字号只有一个来源」都是
// 运行期看不出来的规则——两边都能各写一份、跑起来照样正常，直到 #156
// 那种花屏在某个边界条件下复发。钉源码文本比钉行为更直接。

const tmSrc = readFileSync("src/TerminalManager.ts", "utf8");

const zoomMatch = tmSrc.match(/zoom\(paneId: string, action: ZoomAction\) \{[\s\S]*?\n  \}/);
check("TerminalManager.zoom 方法存在", zoomMatch !== null);
if (zoomMatch) {
  const body = zoomMatch[0];
  // 逐行去掉 // 行注释再匹配，否则被注释掉的代码（比如调试时误留的
  // `// this.fit(paneId);`）也会被正则当成"确实调用了"，守卫形同虚设。
  const bodyNoComments = body
    .split("\n")
    .map((line) => line.slice(0, line.indexOf("//") === -1 ? line.length : line.indexOf("//")))
    .join("\n");
  check(
    "zoom() 里没有直接 invoke pty_resize——resize 必须经过 fit()，不能自己发",
    !/invoke\(\s*["']pty_resize["']/.test(bodyNoComments),
  );
  check("zoom() 确实调用了 this.fit(paneId)", /this\.fit\(paneId\)/.test(bodyNoComments));
}

check(
  "创建 Xterm 时不再有裸的 fontSize: 13——默认值只能来自 DEFAULT_FONT_SIZE 这一个常量",
  !/fontSize:\s*13\s*,/.test(tmSrc),
);
check("TerminalManager.ts 引用了 DEFAULT_FONT_SIZE", /DEFAULT_FONT_SIZE/.test(tmSrc));

console.log(fails.length === 0 ? `✅ ${pass} 项通过` : `❌ ${fails.length} 项失败：\n` + fails.join("\n"));
process.exit(fails.length === 0 ? 0 : 1);
