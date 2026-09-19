/**
 * resume tab 的不变量：**`kind === "resume"` 且有 sessionId 的 tab 必须带 initCommand。**
 *
 * 这条是「在纯 shell 里手打 claude，那个 tab 关掉 makit 后不会自动恢复，还占着
 * 恢复的位置」的根因。`initCommand` 是唯一能到达 PTY 的字段 —— `kind` 只是 UI
 * 元信息，TerminalManager 生成 PTY 时根本不看它。旧版 bindSessionToTab 只改了
 * kind / sessionId / label，把 initCommand 留在 shell tab 的初始值 null 上，于是：
 *   - 下次启动没有命令可跑，只起一个空 shell；
 *   - sessionId 却已经填上，App.findSessionLocation 认为这个会话「已经开着」，
 *     点侧栏只会聚焦到那个空 shell。
 * 两个症状是同一个漏掉的字段，所以这里测的是**字段本身**，不是两个症状。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import {
  resumeCmd,
  resumeInitCommand,
  bindSessionToPaneTab,
  repairResumeTabs,
  type PaneTab,
  type WorkspaceState,
} from "../src/workspace-types.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

function eq(name: string, actual: unknown, expected: unknown) {
  check(`${name}（得到 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}）`, actual === expected);
}

const SID = "df178096-1111-2222-3333-444455556666";

// ---- 命令拼装 -------------------------------------------------------------
eq("claude 的恢复命令", resumeCmd(SID), `claude -r ${SID}`);
eq("codex 的恢复命令", resumeCmd(SID, "codex"), `codex resume ${SID}`);
eq("tool 缺失时按 claude", resumeCmd(SID, undefined), `claude -r ${SID}`);
eq("initCommand 前面要 clear", resumeInitCommand(SID), `clear && claude -r ${SID}`);
eq("codex 的 initCommand", resumeInitCommand(SID, "codex"), `clear && codex resume ${SID}`);

// ---- 认领：shell tab 升级成 resume tab ------------------------------------
const shellTab: PaneTab = {
  id: "t_1", kind: "shell", cwd: "/Users/x/proj",
  initCommand: null, sessionId: null, sessionShortId: null, label: "",
};
const bound = bindSessionToPaneTab(shellTab, SID, "df178096", "改个名字");

eq("认领后 kind 变 resume", bound.kind, "resume");
eq("认领后 sessionId 填上", bound.sessionId, SID);
eq("认领后 shortId 填上", bound.sessionShortId, "df178096");
// ↓ 这一条就是那个 bug。少了它，上面三条全绿而 tab 是坏的。
eq("认领后必须带 initCommand", bound.initCommand, `clear && claude -r ${SID}`);
eq("认领后用 claude 自己写的会话名当标题", bound.label, "改个名字");
eq("cwd 不动（PTY 起在哪由它决定）", bound.cwd, "/Users/x/proj");
eq("tab id 不动", bound.id, "t_1");

const noName = bindSessionToPaneTab(shellTab, SID, "df178096");
eq("没有会话名时回退到 [shortId]", noName.label, "[df178096]");
eq("空白会话名也回退", bindSessionToPaneTab(shellTab, SID, "df178096", "   ").label, "[df178096]");

// ---- 修存档：localStorage 里已经存在的坏 tab ------------------------------
function ws(tabs: PaneTab[]): WorkspaceState {
  return {
    root: { kind: "container", id: "c_1", tabs, activeTabId: tabs[0]?.id ?? "", tabHistory: [] },
    activeContainerId: "c_1",
  };
}
const brokenTab: PaneTab = {
  id: "t_2", kind: "resume", cwd: "/Users/x/proj",
  initCommand: null, sessionId: SID, sessionShortId: "df178096", label: "[df178096]",
};
const repaired = repairResumeTabs(ws([brokenTab]));
eq(
  "存档里的坏 tab 被补上 initCommand",
  (repaired.root as { tabs: PaneTab[] }).tabs[0].initCommand,
  `clear && claude -r ${SID}`,
);

// 不该动的三种：好的 resume tab、shell tab、没有 sessionId 的 resume tab。
const goodResume: PaneTab = { ...brokenTab, id: "t_3", initCommand: "clear && codex resume abc" };
eq(
  "已经有 initCommand 的 resume tab 原样不动（别把 codex 改成 claude）",
  (repairResumeTabs(ws([goodResume])).root as { tabs: PaneTab[] }).tabs[0].initCommand,
  "clear && codex resume abc",
);
eq(
  "shell tab 的 initCommand 保持 null",
  (repairResumeTabs(ws([shellTab])).root as { tabs: PaneTab[] }).tabs[0].initCommand,
  null,
);
const resumeNoSid: PaneTab = { ...brokenTab, id: "t_4", sessionId: null };
eq(
  "没有 sessionId 就没得恢复，不瞎补",
  (repairResumeTabs(ws([resumeNoSid])).root as { tabs: PaneTab[] }).tabs[0].initCommand,
  null,
);

// 没有坏 tab 时原样返回同一个引用：它挂在 useState 的初始值上，多造一棵树
// 就是多一次全量 re-render 的机会。
const clean = ws([shellTab, goodResume]);
check("无坏 tab 时返回原对象", repairResumeTabs(clean) === clean);

// 分屏树要递归到底 —— 坏 tab 常常在某个分屏的深处。
const splitWs: WorkspaceState = {
  root: {
    kind: "split", dir: "h", ratio: 0.5,
    a: { kind: "container", id: "c_1", tabs: [shellTab], activeTabId: "t_1", tabHistory: [] },
    b: {
      kind: "split", dir: "v", ratio: 0.5,
      a: { kind: "container", id: "c_2", tabs: [goodResume], activeTabId: "t_3", tabHistory: [] },
      b: { kind: "container", id: "c_3", tabs: [brokenTab], activeTabId: "t_2", tabHistory: [] },
    },
  },
  activeContainerId: "c_1",
};
const fixedSplit = repairResumeTabs(splitWs);
function findTab(node: any, id: string): PaneTab | null {
  if (node.kind === "container") return node.tabs.find((t: PaneTab) => t.id === id) ?? null;
  return findTab(node.a, id) || findTab(node.b, id);
}
eq(
  "分屏深处的坏 tab 也被修",
  findTab(fixedSplit.root, "t_2")?.initCommand,
  `clear && claude -r ${SID}`,
);
eq("同一棵树里好的 tab 不受影响", findTab(fixedSplit.root, "t_3")?.initCommand, "clear && codex resume abc");

// ---------------------------------------------------------------------------
// 源码扫描。函数自己对不对是一回事，真正会复发的是**别处又手拼一遍命令** ——
// 原来就有 5 处各拼一遍，而第 6 条路（认领）漏了。
// ---------------------------------------------------------------------------
function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.tsx?$/.test(name)) out.push(p);
  }
  return out;
}

const handRolled: string[] = [];
for (const file of walk("src")) {
  if (file === "src/workspace-types.ts") continue;
  readFileSync(file, "utf8")
    .split("\n")
    .forEach((line, i) => {
      // 只抓「拼字符串」，不抓注释里提到的命令名 —— 所以要求命令前面紧贴一个引号
      if (/[`"'](claude -r|codex resume) /.test(line)) handRolled.push(`${file}:${i + 1}`);
    });
}
check(
  `恢复命令只在 workspace-types.ts 里拼（发现 ${handRolled.length} 处别的：${handRolled.join(", ")}）`,
  handRolled.length === 0,
);

if (fails.length) {
  console.error(`✗ resume tab 不变量失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ resume tab 不变量全部通过（${pass} 项）`);
