/**
 * 「折叠一个分组」和「键盘还能不能走」之间的那条不变量。
 *
 * 为什么必须测：折叠是唯一会让「有哪些 session」≠「渲染了哪些行」的功能，而键盘导航
 * 的顺序严格是后者。忘了从顺序里摘掉折叠组的表现是 ↑↓ 走到一个不存在的行上、选中框
 * 凭空消失 —— 界面上完全看不出来源，肉眼回归也很难碰到（要先折叠、再按方向键、再
 * 数第几下）。纯函数化之后这件事一行断言就能钉住。
 */
import { visibleSessionOrder, type OrderGroup, type OrderProject } from "../src/treeOrder.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  const a = JSON.stringify(actual), b = JSON.stringify(expected);
  check(`${name}（得到 ${a}，期望 ${b}）`, a === b);
}

const rows = (...ids: string[]) => ids.map((session_id) => ({ session_id }));

const opened: OrderGroup = { id: "opened", list: rows("o1", "o2") };
const labeled: OrderGroup[] = [
  { id: "attention", list: rows("a1") },
  { id: "running", list: rows("r1", "r2") },
  { id: "pinned", list: rows("p1") },
];
const history = rows("h1", "h2");
const projects: OrderProject[] = [
  { collapsed: false, sessions: rows("x1", "x2") },
  { collapsed: true, sessions: rows("y1") },
];

function order(over: Partial<Parameters<typeof visibleSessionOrder>[0]> = {}) {
  return visibleSessionOrder({
    opened, viewMode: "status", labeled, history, projects,
    collapsedGroups: new Set(),
    ...over,
  });
}

// ---- 1. 基线：状态视图的顺序 = 已打开 → 带标签的组（按给定顺序）→ 历史 ----------
//
// 顺序本身是不变量：↑↓ 必须和眼睛从上往下扫的顺序一致，组的先后换了就是行为变了。

eq("状态视图基线顺序", order(), ["o1", "o2", "a1", "r1", "r2", "p1", "h1", "h2"]);

// 项目视图里带标签的状态组和历史组都不该出现 —— 那是另一种分组方式的产物。
// 第二个项目组 collapsed=true，所以 y1 不在里面。
eq("项目视图基线顺序", order({ viewMode: "project" }), ["o1", "o2", "x1", "x2"]);

// ---- 2. 折叠状态组 → 组里的行全部离开导航顺序 ------------------------------

eq(
  "折叠「后台运行」后 r1/r2 不在顺序里",
  order({ collapsedGroups: new Set(["running"]) }),
  ["o1", "o2", "a1", "p1", "h1", "h2"],
);

// 「已打开」也可折叠（它有标签、有可点表头），而且它在两个视图里都置顶常驻，
// 所以两个视图都要跟着摘掉它。
eq(
  "折叠「已打开」（状态视图）",
  order({ collapsedGroups: new Set(["opened"]) }),
  ["a1", "r1", "r2", "p1", "h1", "h2"],
);
eq(
  "折叠「已打开」（项目视图）",
  order({ viewMode: "project", collapsedGroups: new Set(["opened"]) }),
  ["x1", "x2"],
);

// 全折起来之后状态视图只剩历史组：历史组不可折叠，折它等于把整列清空。
eq(
  "所有可折叠组都折起来，历史组仍在",
  order({ collapsedGroups: new Set(["opened", "attention", "running", "pinned"]) }),
  ["h1", "h2"],
);
// 就算有人给历史组塞了个 id 进 collapsedGroups，也不该生效（它压根不查这个集合）。
eq(
  "collapsedGroups 里的 \"history\" 不影响历史组",
  order({ collapsedGroups: new Set(["history"]) }),
  ["o1", "o2", "a1", "r1", "r2", "p1", "h1", "h2"],
);

// ---- 3. 两套折叠状态互不干扰 ------------------------------------------------
//
// 状态组的折叠键是组 id，项目组的折叠结论由调用方算好（编码归 projectCollapse.ts）。
// 拿状态组的 id 去影响项目组必须是无事发生 —— 两个视图各自只认自己那套。

eq(
  "状态组的 id 不会误折项目组",
  order({ viewMode: "project", collapsedGroups: new Set(["running", "attention", "pinned"]) }),
  ["o1", "o2", "x1", "x2"],
);
eq(
  "折叠全部项目组后只剩「已打开」",
  order({ viewMode: "project", projects: projects.map((g) => ({ ...g, collapsed: true })) }),
  ["o1", "o2"],
);
eq(
  "展开全部项目组",
  order({ viewMode: "project", projects: projects.map((g) => ({ ...g, collapsed: false })) }),
  ["o1", "o2", "x1", "x2", "y1"],
);

// ---- 4. 历史组只算传进来的那一页 -------------------------------------------
//
// 顺序的定义是「这一帧真的渲染出来的行」。调用方喂的是 historyVisible（切到
// visibleRecent 那一页），所以 ↓ 走到分页边界就停 —— 自动翻页会把「按住 ↓」变成
// 无限滚动，用户彻底失去位置感。

eq("历史组为空时顺序里没有历史行", order({ history: [] }), ["o1", "o2", "a1", "r1", "r2", "p1"]);

// ---- 5. 空组不留痕 ----------------------------------------------------------
//
// 空组在界面上整块不渲染（renderGroup 的 list.length === 0 直接 return null），
// 顺序里自然也不该有它的位置。

eq(
  "空的中间组不影响顺序",
  order({ labeled: [{ id: "attention", list: [] }, { id: "running", list: rows("r1") }] }),
  ["o1", "o2", "r1", "h1", "h2"],
);
eq(
  "全空 → 空顺序",
  order({ opened: { id: "opened", list: [] }, labeled: [], history: [] }),
  [],
);

// ---- 6. 顺序里不出现重复 ----------------------------------------------------
//
// 同一个 session_id 出现两次会让 ↑↓ 在那一格卡住（moveSelection 用 indexOf 定位，
// 永远命中第一个）。分组逻辑本身保证互斥，这里当兜底。

const all = order();
check(`顺序里无重复（${all.length} 行）`, new Set(all).size === all.length);

console.log(fails.length === 0 ? `✅ ${pass} 项通过` : `❌ ${fails.length} 项失败：\n` + fails.join("\n"));
process.exit(fails.length === 0 ? 0 : 1);
