/**
 * 右键菜单的两条纯规则：菜单落点修正、tab 批量关闭算哪些。
 *
 * clampMenuPosition —— 这条规则原来**只有一处有**：侧栏 session 菜单写了边界翻转，
 * pane 容器菜单没写，于是在屏幕右下角右键，容器菜单一半在屏幕外，够不着最后一项。
 * 菜单从 2 处变 4 处，靠"记得抄"必然再漏，所以收成一个纯函数在这里钉住。
 *
 * tabsToClose —— 锚点必须是**右键的那个 tab**，不是 activeTabId。容器菜单的「关闭」
 * 一直用 activeTabId，在非激活 tab 上右键点关闭关掉的是别人；批量关闭把这个坑放大
 * N 倍，所以这里专门测「锚点是谁」。
 */
import { clampMenuPosition } from "../src/menuPosition.ts";
import { tabsToClose } from "../src/workspace-types.ts";
import type { PaneTab } from "../src/workspace-types.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eqJson(name: string, actual: unknown, expected: unknown) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  check(`${name}（得到 ${a}，期望 ${e}）`, a === e);
}

// ---- clampMenuPosition ----------------------------------------------------
const VW = 1000;
const VH = 800;
const W = 160;
const H = 200;

eqJson("屏幕中间：原样不动", clampMenuPosition(300, 400, W, H, VW, VH), { x: 300, y: 400 });

// 右边缘：菜单右侧会超出，往左推到留 8px 空隙。
eqJson("贴右边：往左推", clampMenuPosition(900, 100, W, H, VW, VH), { x: VW - W - 8, y: 100 });
// 下边缘：往上推。这条就是原来 pane 菜单缺的那半 —— 屏幕右下角右键点不到最后一项。
eqJson("贴下边：往上推", clampMenuPosition(300, 700, W, H, VW, VH), { x: 300, y: VH - H - 8 });
eqJson("右下角：两个方向都推", clampMenuPosition(980, 790, W, H, VW, VH), { x: VW - W - 8, y: VH - H - 8 });

// 正好压线：菜单右下角落在 viewport - MARGIN 上，不该动。
eqJson("正好留够 8px 空隙：不动", clampMenuPosition(VW - W - 8, VH - H - 8, W, H, VW, VH), { x: VW - W - 8, y: VH - H - 8 });

// 菜单比视口还高（菜单项多 + 窗口很矮）：第一步算出负数，第二步按回 MARGIN。
// 底部仍然超出，由 CSS 的 max-height + overflow-y 接管 —— 滚动比"顶部被切掉"好，
// 顶部切掉的话第一项永远够不着，而第一项通常是最常用的那个。
const tall = clampMenuPosition(100, 300, W, 900, VW, VH);
eqJson("菜单比视口还高：压在上边界而不是负数", tall, { x: 100, y: 8 });
const wide = clampMenuPosition(100, 300, 1200, H, VW, VH);
check("菜单比视口还宽：压在左边界", wide.x === 8);

// 鼠标本来就在边缘外侧（多屏拖出去过）：不能算出负坐标。
const negative = clampMenuPosition(-50, -50, W, H, VW, VH);
eqJson("鼠标坐标为负：夹回左上", negative, { x: 8, y: 8 });

// ---- tabsToClose ---------------------------------------------------------
function tab(id: string): PaneTab {
  return { id, kind: "shell", cwd: "/tmp", initCommand: null, sessionId: null, sessionShortId: null, label: id };
}
const tabs = [tab("a"), tab("b"), tab("c"), tab("d")];

eqJson("关闭其他：锚点自己留着，其余按显示顺序", tabsToClose(tabs, "b", "others"), ["a", "c", "d"]);
eqJson("关闭右侧：只要锚点后面的", tabsToClose(tabs, "b", "right"), ["c", "d"]);
eqJson("在最后一个 tab 上「关闭右侧」：空", tabsToClose(tabs, "d", "right"), []);
eqJson("在第一个 tab 上「关闭右侧」：其余全部", tabsToClose(tabs, "a", "right"), ["b", "c", "d"]);

// 锚点是右键的那个 tab，和「哪个是激活的」无关 —— 这就是原来的 bug：
// 在非激活 tab 上右键点关闭，关掉的是 activeTabId。
eqJson("单 tab 时「关闭其他」为空（不能把自己也关了）", tabsToClose([tab("a")], "a", "others"), []);

// 找不到锚点时返回空数组而不是"全关"：调用方拿到的是一串 destroy 调用，
// 空数组是唯一安全的降级。
eqJson("锚点不在列表里：空数组（不能退化成全关）", tabsToClose(tabs, "zzz", "others"), []);
eqJson("锚点不在列表里，right 也是空", tabsToClose(tabs, "zzz", "right"), []);
eqJson("空列表", tabsToClose([], "a", "others"), []);

// 不能改动入参数组。
const orig = [tab("a"), tab("b")];
tabsToClose(orig, "a", "right");
check("不修改传进来的 tabs 数组", orig.length === 2 && orig[0].id === "a");

if (fails.length) {
  console.error(`✗ 右键菜单规则失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 右键菜单规则全部通过（${pass} 项）`);
