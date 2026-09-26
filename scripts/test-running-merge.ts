// `running-changed` 合并的回归测试：改名必须实时生效，不能等 ⌘R。
//
// 为什么是独立脚本而不是 vitest：和 test-workspace-invariant.ts 同一个理由 ——
// applyRunningMeta 是纯函数、running-merge.ts 是零 import 的叶子模块，
// Node 22 的 --experimental-strip-types 直接能跑。
//
// 跑：node --experimental-strip-types scripts/test-running-merge.ts
import { applyRunningMeta, mergeRunning, type RunningMeta } from "../src/running-merge.ts";

type Session = {
  session_id: string;
  running: boolean;
  status: string;
  waiting_for: string;
  pid: number;
  display_name: string;
  name_source: string;
  first_user_msg: string;
};

function session(over: Partial<Session> = {}): Session {
  return {
    session_id: "s1",
    running: false,
    status: "idle",
    waiting_for: "",
    pid: 0,
    display_name: "",
    name_source: "",
    first_user_msg: "build当前项目",
    ...over,
  };
}

function meta(over: Partial<RunningMeta> = {}): RunningMeta {
  return { session_id: "s1", status: "busy", waiting_for: "", pid: 4242, name: "", ...over };
}

let pass = 0;
const failures: string[] = [];

function check(name: string, got: unknown, want: unknown) {
  if (got === want) { pass++; return; }
  failures.push(`  ✗ ${name}\n      期望 ${JSON.stringify(want)}，实际 ${JSON.stringify(got)}`);
}

// --- 核心回归：claude 改名只会触发 running-changed，标题必须跟着变 ---
// 改名写的是 ~/.claude/sessions/<pid>.json，不写 jsonl，所以 sessions-changed
// 不会发，parse_session 不会重跑。这条路搬不动标题的话，改名就要 ⌘R 才生效（#4 / #8）。
{
  const got = applyRunningMeta(session({ display_name: "旧名", name_source: "custom-title" }), meta({ name: "新名" }));
  check("改名后 display_name 实时更新", got.display_name, "新名");
  check("改名后 name_source 标成 rename", got.name_source, "rename");
}

// --- 反向护栏：不能靠「一律用 r.name 覆盖」来过上面那条 ---
// r.name 为空时（claude 没写名字）必须保留后端算好的 display_name，
// 否则本该显示 customTitle 的会话会被打成首条消息原文。
{
  const got = applyRunningMeta(session({ display_name: "oc-install-pack脚本编写", name_source: "custom-title" }), meta({ name: "" }));
  check("r.name 为空时不动 display_name", got.display_name, "oc-install-pack脚本编写");
  check("r.name 为空时不动 name_source", got.name_source, "custom-title");
}

// --- 原有职责不能丢：运行状态字段照旧搬 ---
{
  const got = applyRunningMeta(session(), meta({ status: "waiting", waiting_for: "permission", pid: 999 }));
  check("running 置真", got.running, true);
  check("status 搬过来", got.status, "waiting");
  check("waiting_for 搬过来", got.waiting_for, "permission");
  check("pid 搬过来", got.pid, 999);
}

// --- 进程退出：状态清干净，但标题**故意**不动 ---
// 回落目标算不出来（后端 rename > customTitle > agentName > 空，而 customTitle
// 没随 SessionMeta 下发），硬清成空是拿新错误换旧错误。见 running-merge.ts 注释。
{
  const got = applyRunningMeta(session({ running: true, status: "busy", pid: 123, display_name: "旧名", name_source: "rename" }), undefined);
  check("进程没了 → running 置假", got.running, false);
  check("进程没了 → status 归 idle", got.status, "idle");
  check("进程没了 → pid 清零", got.pid, 0);
  check("进程没了 → display_name 保持不动（已知边界）", got.display_name, "旧名");
}

// --- 本来就不 running 且这轮也不在 → 必须返回同一个引用 ---
// running-changed 在会话活跃时每 500ms 一发，对每条 session 都造新对象会让
// 整棵侧栏白重渲染一遍。
{
  const s = session({ running: false });
  check("无变化时返回同一引用", applyRunningMeta(s, undefined) === s, true);
}

// --- 一直在跑、这轮读到的状态和上轮一样 → 也必须返回同一个引用（#219）---
// 活跃会话恰恰是这种情况：每 500ms 一发的 running-changed 里它每次都在、值都没变。
// 以前照样造新对象 → sessions 数组换身份 → 所有 [sessions] 的 memo 重算、侧栏全量重渲染。
// 200+ 会话的项目里这就是「非常卡」的主因之一。
{
  const r: RunningMeta = { session_id: "s1", status: "busy", waiting_for: "", pid: 7, name: "" };
  const s = session({ running: true, status: "busy", pid: 7 });
  check("运行中且无变化 → 同一引用", applyRunningMeta(s, r) === s, true);
  const renamed = session({ running: true, status: "busy", pid: 7, display_name: "新名", name_source: "rename" });
  check("改名已生效过、名字没再变 → 同一引用", applyRunningMeta(renamed, { ...r, name: "新名" }) === renamed, true);
  check("状态变了 → 新对象", applyRunningMeta(s, { ...r, status: "waiting" }) === s, false);
  check("名字变了 → 新对象", applyRunningMeta(s, { ...r, name: "改了" }).display_name, "改了");
}

// --- 整个列表：没有任何一条变 → 返回原数组（React 据此跳过重渲染）---
{
  const a = session({ session_id: "a", running: true, status: "busy", pid: 1 });
  const b = session({ session_id: "b" });
  const prev = [a, b];
  const list: RunningMeta[] = [{ session_id: "a", status: "busy", waiting_for: "", pid: 1, name: "" }];
  check("列表无变化 → 原数组", mergeRunning(prev, list, () => false) === prev, true);
  const changed = mergeRunning(prev, [{ ...list[0], status: "waiting" }], () => false);
  check("有一条变了 → 新数组", changed === prev, false);
  check("没变的那条保持原对象", changed[1] === b, true);
  check("被跳过的（归档中）不合并", mergeRunning(prev, [{ ...list[0], status: "waiting" }], (s) => s.session_id === "a") === prev, true);
}

if (failures.length > 0) {
  console.error(`✗ running-changed 合并：${pass} 通过，${failures.length} 失败\n${failures.join("\n")}`);
  process.exit(1);
}
console.log(`✓ running-changed 合并全部通过（${pass} 项）`);
