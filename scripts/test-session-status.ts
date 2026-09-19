/**
 * 侧栏状态点的 2×2 映射，以及「idle 和 stopped 必须可分」这条不变量。
 *
 * 为什么要单独测：这一整块的价值就是让人一眼分清四种状态，而它在界面上是一个
 * 10px 的圆点 —— 肉眼回归不了，两个灰点的差别在截图里也看不出来。曾经的实际状态是
 * `.idle` 和 `.stopped` 都写着 `color: var(--fg-muted)`，唯一区别是 ● / ○ 的填充
 * 和 0.4 的透明度，于是「打开中的和运行中的状态区别不明了」。
 *
 * 所以钉两件事：
 *   1. 四种输入组合各自落到哪一档（JS 层）；
 *   2. 四档在 CSS 里的 color 声明互不相同（样式层）—— 这是当年真正塌掉的地方，
 *      JS 层从来都是对的，只测 JS 抓不到它。
 */
import { readFileSync } from "node:fs";
import { runState, runStateIcon, runStateClass, runStateTitle, anyAlive, type RunState } from "../src/sessionStatus.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  check(`${name}（得到 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}）`, actual === expected);
}

// ---- 1. 映射表 -------------------------------------------------------------

// running × status 的组合。status 只有 waiting / busy / 其它（idle 等）三种取值有意义。
const CASES: [boolean, string, RunState][] = [
  [true, "waiting", "waiting"],
  [true, "busy", "busy"],
  [true, "idle", "idle"],
  [false, "idle", "stopped"],
  // 进程活着 + 空字符串 status（后端还没算出来）→ 按「活着但闲着」处理，不是已停止。
  [true, "", "idle"],
  [false, "", "stopped"],
  // 进程没了但 status 还挂着 waiting：宁可相信 status。行上还画着「等待审批」药丸，
  // 圆点画成空心灰会让同一行自相矛盾；这种组合只在 pid 扫描漏掉、或等待时被杀了才出现。
  [false, "waiting", "waiting"],
  // busy 但进程没了：status 是过期的声明，进程生死更可信 → 空心。
  [false, "busy", "busy"],
];

for (const [running, status, expected] of CASES) {
  eq(`runState(running=${running}, status="${status}")`, runState({ running, status }), expected);
}

// ---- 2. 形状通道：实心 = 活着，空心 = 已停止 -------------------------------

eq("waiting 实心", runStateIcon("waiting"), "●");
eq("busy 实心", runStateIcon("busy"), "●");
eq("idle 实心", runStateIcon("idle"), "●");
eq("stopped 空心", runStateIcon("stopped"), "○");

// ---- 3. 四档各自有独立的 class 和 tooltip ----------------------------------

const ALL: RunState[] = ["waiting", "busy", "idle", "stopped"];
check("四档 class 互不相同", new Set(ALL.map(runStateClass)).size === 4);
check("四档 tooltip 互不相同", new Set(ALL.map(runStateTitle)).size === 4);
for (const st of ALL) {
  check(`${st} 的 tooltip 是人话（非空、不是 class 名本身）`, runStateTitle(st).length > 4 && runStateTitle(st) !== st);
}

// ---- 4. CSS：四档的 color 声明互不相同 -------------------------------------
//
// 这是当年塌掉的那一层。写成读 CSS 文本而不是跑浏览器：这条不变量是「作者有没有
// 给它们不同的颜色」，纯静态，不需要渲染引擎就能回答。

const css = readFileSync("src/SessionTree.css", "utf8");
const colors = new Map<string, string>();
for (const st of ALL) {
  // 抓 `.tree-status-icon.<st> { ... color: X; ... }` 里的 color 声明
  const block = css.match(new RegExp(`\\.tree-status-icon\\.${st}\\s*\\{([^}]*)\\}`));
  check(`CSS 里有 .tree-status-icon.${st} 规则`, block !== null);
  if (!block) continue;
  const color = block[1].match(/(?:^|[;\s])color\s*:\s*([^;]+)/);
  check(`.tree-status-icon.${st} 声明了 color`, color !== null);
  if (color) colors.set(st, color[1].trim());
}

check(
  `四档颜色互不相同（得到 ${JSON.stringify([...colors])}）`,
  new Set(colors.values()).size === colors.size && colors.size === 4,
);

// 单独把 idle / stopped 拎出来断言：这一对是整列里最该分清的（活的点进去接着用，
// 死的要 claude -r 重启并重载上下文），也是当年唯一塌掉的那一对。
check(
  `idle 和 stopped 的颜色不同（idle=${colors.get("idle")}，stopped=${colors.get("stopped")}）`,
  colors.get("idle") !== colors.get("stopped"),
);

// ---- 5. anyAlive：要不要给这一组画状态列 -----------------------------------
//
// 「历史」和「收藏」两组里的圆点恒为灰空心（活着的会话一定被上游的「需要回应」/
// 「后台运行」先拿走），一列常量却每行占 18px。按组关掉它就得先答对这一问。

const dead = { running: false, status: "idle" };
const live = { running: true, status: "idle" };

check("空组没有活的", anyAlive([]) === false);
check("全是已停止 → 没有活的", anyAlive([dead, dead, dead]) === false);
check("混着一个活的 → 有活的", anyAlive([dead, live, dead]) === true);
check("只有一个活的 → 有活的", anyAlive([live]) === true);
// 进程死了但 status 还挂着 waiting / busy 也算活（和 runState 保持一致：这种行上
// 还画着「等待审批」，把它当已停止会让同一行自相矛盾）。
check("死 pid + waiting 算活", anyAlive([{ running: false, status: "waiting" }]) === true);
check("死 pid + busy 算活", anyAlive([{ running: false, status: "busy" }]) === true);

// anyAlive 必须和 runState 的「非 stopped」严格等价 —— 它就是按组的那个判断，
// 两边一旦漂了，会出现「组里有活的但不画状态列」（那个活的就彻底看不出来了）。
for (const [running, status, expected] of CASES) {
  eq(
    `anyAlive([{${running}, "${status}"}]) 和 runState 一致`,
    anyAlive([{ running, status }]),
    expected !== "stopped",
  );
}

// 这一问原来以 hasActive 的名字内联在 SessionTree.tsx 的 groupByGitRoot 里
// （项目组默认折不折 + 组排序都用它）。合并成一个定义之后，钉住那份内联实现没有
// 残留 —— 留着两份等价谓词，改一处忘一处就是「侧栏这里说有活的、那里说没有」。
const treeSrc = readFileSync("src/SessionTree.tsx", "utf8");
check(
  "SessionTree.tsx 里没有第二份 hasActive 内联谓词",
  !/status === "busy" \|\| s\.status === "waiting"/.test(treeSrc),
);
check("hasActive 走的是 anyAlive", /const hasActive = anyAlive\(sessions\)/.test(treeSrc));

console.log(fails.length === 0 ? `✅ ${pass} 项通过` : `❌ ${fails.length} 项失败：\n` + fails.join("\n"));
process.exit(fails.length === 0 ? 0 : 1);
