/**
 * 侧栏项目组的折叠编码，以及 ⌘L 定位必须能强制展开。
 *
 * ⌘L 原来只做了两件事：翻页 + `querySelector(".tree-session.focused")?.scrollIntoView()`。
 * 折叠的项目组连 <li> 都不渲染 —— querySelector 拿回 null，`el?.` 静默什么也不做，
 * 于是「⌘L 不会把左边关掉的文件夹展开去定位，必须自己点开项目目录」。
 *
 * 编码本身是这里的第二个测点：折叠/展开两侧存在同一个 Set 里（默认值不是常量，
 * 取决于组里有没有活跃进程），改一侧忘了删另一侧就会出现「点了没反应」。
 */
import {
  projectCollapseKey,
  isProjectCollapsed,
  toggleProjectCollapsed,
  expandProjectForReveal,
  collapseOtherProjects,
} from "../src/projectCollapse.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  check(`${name}（得到 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}）`, actual === expected);
}

const ROOT = "/Users/x/makit";
const KEY = ROOT;

// ---- key ------------------------------------------------------------------
eq("有 git 仓库就用仓库根", projectCollapseKey(ROOT, "makit"), ROOT);
eq("没有 git 仓库退到组名", projectCollapseKey("", "其他"), "其他");

// ---- 默认值：由「组里有没有活跃进程」决定 ---------------------------------
check("有活跃 session 的组默认展开", !isProjectCollapsed(new Set(), KEY, true));
check("全是已停止 session 的组默认折叠", isProjectCollapsed(new Set(), KEY, false));

// ---- 两个显式态都能压过默认值 ---------------------------------------------
check("显式折叠压过「有活跃」的默认展开", isProjectCollapsed(new Set([KEY]), KEY, true));
check("显式展开压过默认折叠", !isProjectCollapsed(new Set(["__expanded__" + KEY]), KEY, false));
check(
  "显式折叠优先于显式展开（两个键同时在时以折叠为准）",
  isProjectCollapsed(new Set([KEY, "__expanded__" + KEY]), KEY, false),
);

// ---- toggle：必须删掉反面那个键 -------------------------------------------
// 只加不删的话，第二次点击加上的键会被上一次留下的键压住 —— 表现是「点了没反应」。
const afterExpand = toggleProjectCollapsed(new Set([KEY]), KEY, true);
check("从折叠点开：加上展开键", afterExpand.has("__expanded__" + KEY));
check("从折叠点开：删掉折叠键", !afterExpand.has(KEY));
check("从折叠点开后真的是展开的", !isProjectCollapsed(afterExpand, KEY, false));

const afterCollapse = toggleProjectCollapsed(afterExpand, KEY, false);
check("再点一次：加上折叠键", afterCollapse.has(KEY));
check("再点一次：删掉展开键", !afterCollapse.has("__expanded__" + KEY));
check("再点一次后真的是折叠的", isProjectCollapsed(afterCollapse, KEY, true));

check("toggle 不改原 Set", new Set([KEY]).size === 1 && !afterExpand.has(KEY));

// ---- ⌘L 定位：强制展开 ----------------------------------------------------
// 这三条是这次修的 bug 本体。
const revealFromDefault = expandProjectForReveal(new Set(), KEY, false);
check(
  "定位到「默认折叠」的组 → 展开（组里 session 都停了，这是最常见的一种）",
  !isProjectCollapsed(revealFromDefault, KEY, false),
);
const revealFromExplicit = expandProjectForReveal(new Set([KEY]), KEY, false);
check(
  "定位到「用户手动折叠过」的组 → 也展开（定位的意图压过之前的折叠）",
  !isProjectCollapsed(revealFromExplicit, KEY, false),
);
check("定位时删掉了折叠键，否则展开键会被压住", !revealFromExplicit.has(KEY));

// 已经展开的不要动 state：给一个默认展开的组补 __expanded__，等于把「默认」
// 偷偷改成「用户显式展开」，之后组里进程全停了它也不会再自动收起来。
const alreadyOpen = new Set<string>();
check("定位到本来就展开的组：返回同一个 Set（不触发 re-render）", expandProjectForReveal(alreadyOpen, KEY, true) === alreadyOpen);
check("而且没有偷偷加上显式展开键", !expandProjectForReveal(alreadyOpen, KEY, true).has("__expanded__" + KEY));
const explicitlyOpen = new Set(["__expanded__" + KEY]);
check("已经显式展开的也原样返回", expandProjectForReveal(explicitlyOpen, KEY, false) === explicitlyOpen);

// 别的组的键不能被顺手删掉。
const other = new Set([KEY, "/Users/x/other"]);
check("展开一个组不影响别的组的折叠状态", expandProjectForReveal(other, KEY, false).has("/Users/x/other"));

// ---- 右键「只看这个项目」 -------------------------------------------------
// 和 ⌘L 相反，这条**该**写显式态：用户主动说「我只想看这一个」，记成偏好正是他要的。
const A = "/Users/x/a";
const B = "/Users/x/b";
const C = "/Users/x/c";
const only = collapseOtherProjects(new Set<string>(), [A, B, C], B);
check("只看 B：A 被折叠", only.has(A));
check("只看 B：C 被折叠", only.has(C));
check("只看 B：B 自己不在折叠键里", !only.has(B));
check("只看 B：B 写上了显式展开键", only.has("__expanded__" + B));
check("只看 B：A 的显式展开键被删掉了，否则折叠压不住它", !only.has("__expanded__" + A));

// 别的组本来是显式展开的（用户手点开过），只看 B 时必须真的收起来。
const hadExplicit = new Set(["__expanded__" + A, "__expanded__" + C]);
const only2 = collapseOtherProjects(hadExplicit, [A, B, C], B);
check("原来显式展开的组会被真的折叠", only2.has(A) && !only2.has("__expanded__" + A));
check("入参 Set 不被修改", hadExplicit.has("__expanded__" + A) && !hadExplicit.has(A));

// 保留的那个组不管原来是折叠还是默认，都要变成展开。
const keepWasCollapsed = collapseOtherProjects(new Set([B]), [A, B], B);
check("保留的组原来是折叠的，也要展开", !isProjectCollapsed(keepWasCollapsed, B, false));
check("被折叠的组即使有活跃进程也压得住（显式折叠优先于默认）", isProjectCollapsed(only, A, true));

if (fails.length) {
  console.error(`✗ 项目组折叠规则失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 项目组折叠规则全部通过（${pass} 项）`);
