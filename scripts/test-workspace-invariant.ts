// 放大态不变量的回归测试。
//
// 为什么是一个独立脚本而不是 vitest：前端侧没有测试框架，而 normalizeWorkspace
// 是个纯函数、且 workspace-types.ts 是零 import 的叶子模块 —— Node 22 的
// --experimental-strip-types 能直接跑它，不用为一个纯函数引入 vitest + 配置。
//
// 跑：node --experimental-strip-types scripts/test-workspace-invariant.ts
//     （或 pnpm test:workspace）
import {
  normalizeWorkspace,
  type WorkspaceState,
  type ContainerNode,
  type LayoutNode,
} from "../src/workspace-types.ts";

function container(id: string, tabIds: string[] = ["t1"]): ContainerNode {
  return {
    kind: "container",
    id,
    tabs: tabIds.map((t) => ({
      id: t, kind: "shell" as const, cwd: "~", initCommand: null,
      sessionId: null, sessionShortId: null, label: "",
    })),
    activeTabId: tabIds[0] ?? "",
    tabHistory: [],
  };
}

function split(a: LayoutNode, b: LayoutNode): LayoutNode {
  return { kind: "split", dir: "h", ratio: 0.5, a, b };
}

let pass = 0;
const failures: string[] = [];

function check(name: string, got: unknown, want: unknown) {
  if (got === want) { pass++; return; }
  failures.push(`  ✗ ${name}\n      期望 ${JSON.stringify(want)}，实际 ${JSON.stringify(got)}`);
}

// 基础布局：C1 | C2
const twoPane = () => split(container("C1"), container("C2"));

// --- 症状 1：关掉「被放大 pane」里的一个 tab，不该退出放大 ---
// （handleTabClose 补上 ...ws 之后，到达 normalize 的状态是 max=C1 active=C1）
check(
  "关放大 pane 里的一个 tab（还剩别的）→ 保持放大",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "C1", maximizedContainerId: "C1" }).maximizedContainerId,
  "C1",
);

// --- 症状 2：放大态下切 pane，放大跟着焦点走 ---
check(
  "放大 C1 时 ⌥⌘→ 到 C2 → 放大跟到 C2（不留在 C1）",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "C2", maximizedContainerId: "C1" }).maximizedContainerId,
  "C2",
);

// --- 顺序依赖：① 必须先于 ② ---
// 关掉放大 container 的最后一个 tab：C1 从树上消失，active 落到 C2。
// 若 ② 先跑就会把 C2 放大（错：用户没要求放大 C2）；① 先跑才是退出放大。
check(
  "放大的 container 整个消失 → 退出放大（而不是把新落脚的放大）",
  normalizeWorkspace({ root: container("C2"), activeContainerId: "C2", maximizedContainerId: "C1" }).maximizedContainerId,
  null,
);

// --- 边界：焦点落在不存在的 id（handleMoveTab 的 `?? ""` 兜底路径）---
check(
  "焦点是空串 → 退出放大，不把 maximized 指向空 id",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "", maximizedContainerId: "C1" }).maximizedContainerId,
  null,
);

// --- 非放大态不受影响 ---
check(
  "未放大（null）→ 原样返回",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "C2", maximizedContainerId: null }).maximizedContainerId,
  null,
);
check(
  "未放大（字段缺失）→ 不凭空造出 maximized",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "C2" }).maximizedContainerId,
  undefined,
);

// --- 幂等 + 一致态不产生新对象（避免多余 re-render）---
const stable: WorkspaceState = { root: twoPane(), activeContainerId: "C1", maximizedContainerId: "C1" };
check("已一致的状态原样返回同一个对象引用", normalizeWorkspace(stable), stable);
const once = normalizeWorkspace({ root: twoPane(), activeContainerId: "C2", maximizedContainerId: "C1" });
check("幂等：再跑一次不再变化", normalizeWorkspace(once), once);

// --- toggleMaximize 的两个方向 ---
check(
  "toggleMaximize 开：max 与 active 同时设为同一个 → 不变",
  normalizeWorkspace({ root: twoPane(), activeContainerId: "C2", maximizedContainerId: "C2" }).maximizedContainerId,
  "C2",
);

// --- split 刻意置 null 的语义要保住 ---
check(
  "split 后 max=null、焦点在新 pane → 不该被 normalize 重新放大",
  normalizeWorkspace({ root: split(container("C1"), container("Cnew")), activeContainerId: "Cnew", maximizedContainerId: null }).maximizedContainerId,
  null,
);

if (failures.length > 0) {
  console.error(`✗ 放大态不变量：${pass} 通过，${failures.length} 失败\n${failures.join("\n")}`);
  process.exit(1);
}
console.log(`✓ 放大态不变量全部通过（${pass} 项）`);
