/**
 * 侧栏「打开中」段的顺序，以及键盘上下移动选中。
 *
 * 为什么这两个函数要抽出来单独测：
 *
 * 1. `openedOrder` 是「打开中」段的**唯一数据源**。它对应的是屏幕上的空间布局
 *    （左/上的 pane 先出现），不是 mtime —— 切会话时用户想的是「我刚才那个窗口」，
 *    按时间排会让同一屏的几个 pane 在侧栏里每次都换位置。顺序错了这段就废了，
 *    但顺序本身在 UI 上不好断言，只能在这里钉住。
 *
 * 2. `moveSelection` 的全部价值都在边界上：列表空、选中项被搜索筛掉了、走到头。
 *    这三种情况下如果返回 undefined / 越界 id / 循环回绕，表现分别是「按了没反应」
 *    「选中框消失」「按住 ↓ 永远到不了底」—— 都是只有边界才暴露的。
 */
import {
  collectContainers,
  openedOrder,
  type ContainerNode,
  type LayoutNode,
  type PaneTab,
  type WorkspaceState,
} from "../src/workspace-types.ts";
import { moveSelection } from "../src/treeNav.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  check(
    `${name}（得到 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}）`,
    JSON.stringify(actual) === JSON.stringify(expected),
  );
}

// ---- 构造工具 --------------------------------------------------------------

let n = 0;
function tab(over: Partial<PaneTab> = {}): PaneTab {
  n++;
  return {
    id: `t${n}`,
    kind: "resume",
    cwd: "/Users/x/p",
    initCommand: null,
    sessionId: `s${n}`,
    sessionShortId: `s${n}`,
    label: "",
    ...over,
  };
}

function container(tabs: PaneTab[], id = `c${++n}`): ContainerNode {
  return { kind: "container", id, tabs, activeTabId: tabs[0]?.id ?? "", tabHistory: [] };
}

function ws(root: LayoutNode): WorkspaceState {
  return { root, activeContainerId: collectContainers(root)[0]?.id ?? "" };
}

// ---- openedOrder ----------------------------------------------------------

eq("一个 container 里多个 tab：按 tab 条顺序",
  openedOrder(ws(container([tab({ sessionId: "a" }), tab({ sessionId: "b" }), tab({ sessionId: "c" })]))),
  ["a", "b", "c"]);

// 分屏的遍历顺序（a 先于 b）就是屏幕上的左/上先于右/下，这正是我们要的顺序。
const split: LayoutNode = {
  kind: "split",
  dir: "v",
  ratio: 0.5,
  a: container([tab({ sessionId: "left1" }), tab({ sessionId: "left2" })]),
  b: container([tab({ sessionId: "right1" })]),
};
eq("多 container：左/上的 pane 整段排在右/下之前",
  openedOrder(ws(split)),
  ["left1", "left2", "right1"]);

// 嵌套分屏也必须是深度优先的空间顺序，不能把嵌套那一支塞到最后。
const nested: LayoutNode = {
  kind: "split",
  dir: "v",
  ratio: 0.5,
  a: {
    kind: "split",
    dir: "h",
    ratio: 0.5,
    a: container([tab({ sessionId: "topLeft" })]),
    b: container([tab({ sessionId: "bottomLeft" })]),
  },
  b: container([tab({ sessionId: "right" })]),
};
eq("嵌套分屏按深度优先的空间顺序",
  openedOrder(ws(nested)),
  ["topLeft", "bottomLeft", "right"]);

// shell / new tab 没有会话可言；resume 但 sessionId 还没绑上的也一样（用户手打
// 了 claude、后端还没从 pid 文件认出来）。混进 null 会让「打开中」段渲染出空行。
eq("非 resume 的 tab 跳过",
  openedOrder(ws(container([
    tab({ kind: "shell", sessionId: null }),
    tab({ sessionId: "real" }),
    tab({ kind: "new", sessionId: null }),
  ]))),
  ["real"]);
eq("kind 是 resume 但 sessionId 为 null 的跳过",
  openedOrder(ws(container([tab({ kind: "resume", sessionId: null }), tab({ sessionId: "real" })]))),
  ["real"]);

// 同一条会话不可能真开在两个 pane（openResumeTab 会切过去而不是再开一个），
// 但坏掉的持久化状态里出现过。去重是因为「打开中」段用 session_id 做 React key，
// 重复 key 会让 React 只渲染一条、且行为诡异。
eq("同一会话开在两处只出现一次，位置取第一次出现",
  openedOrder(ws({
    kind: "split", dir: "v", ratio: 0.5,
    a: container([tab({ sessionId: "dup" }), tab({ sessionId: "x" })]),
    b: container([tab({ sessionId: "dup" })]),
  })),
  ["dup", "x"]);

eq("一个 tab 都没开：空数组（这段整个不渲染）", openedOrder(ws(container([]))), []);

// ---- moveSelection -------------------------------------------------------

const L = ["a", "b", "c"];

eq("空列表：没有可选的，返回 null", moveSelection([], null, "down"), null);
eq("空列表 + 已有选中：也返回 null", moveSelection([], "a", "down"), null);

// 从「没有选中」开始，两个方向都落到第一条。↑ 不去选最后一条 —— 192 条的历史
// 列表里那会直接把视图甩到底部，用户以为按错了键。
eq("没有选中时按 ↓：选第一条", moveSelection(L, null, "down"), "a");
eq("没有选中时按 ↑：也选第一条（不跳到列表底部）", moveSelection(L, null, "up"), "a");

// 选中的那条被搜索筛掉后，selectedId 还留在 state 里但已不在 order 里。
// 这时必须当成「没有选中」重新起步，而不是 indexOf 拿到 -1 再 ±1 去越界取值。
eq("选中项已被筛掉：按 ↓ 回到第一条", moveSelection(L, "gone", "down"), "a");
eq("选中项已被筛掉：按 ↑ 也回到第一条", moveSelection(L, "gone", "up"), "a");

eq("往下走一格", moveSelection(L, "a", "down"), "b");
eq("往上走一格", moveSelection(L, "b", "up"), "a");
eq("跨组连续（order 已经是拉平的渲染顺序，函数不需要知道组）", moveSelection(L, "b", "down"), "c");

// 到头停住，不循环：长列表里循环会让人彻底失去位置感。
eq("到底了按 ↓：停在最后一条", moveSelection(L, "c", "down"), "c");
eq("到顶了按 ↑：停在第一条", moveSelection(L, "a", "up"), "a");

eq("只有一条时按 ↓ 停住", moveSelection(["only"], "only", "down"), "only");
eq("只有一条时按 ↑ 停住", moveSelection(["only"], "only", "up"), "only");

if (fails.length) {
  console.error(`✗ 侧栏键盘导航失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 侧栏键盘导航全部通过（${pass} 项）`);
