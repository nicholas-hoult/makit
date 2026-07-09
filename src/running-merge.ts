// `running-changed` 事件的合并逻辑。
//
// 为什么单独一个零 import 的叶子模块：这段是纯函数，但它原来内联在 App.tsx 的
// setSessions 回调里，测不到 —— 而它承担的正是「标题能不能实时更新」这条不变量。
// 抽出来之后 Node 的 --experimental-strip-types 可以直接跑
// （见 scripts/test-running-merge.ts），不用为一个纯函数引入 vitest。

/** `list_running_sessions` 的返回项。后端 `RunningMeta`（lib.rs）的镜像。 */
export type RunningMeta = {
  session_id: string;
  status: string;
  waiting_for: string;
  pid: number;
  /** claude 写在 `~/.claude/sessions/<pid>.json` 里的会话名。可能为空。 */
  name: string;
};

/** 合并只碰这几个字段，用结构类型约束，SessionMeta 的其余字段原样透传。 */
type Mergeable = {
  running: boolean;
  status: string;
  waiting_for: string;
  pid: number;
  display_name: string;
  name_source: string;
};

/**
 * 把一次 `running-changed` 的结果合进一条 session。
 *
 * 关键点是 `name`：`~/.claude/sessions/` 的写入（= claude 改名）**只**会触发
 * `running-changed`。标题的最高优先级来源就是那个文件的 `name`，但它以前只在
 * `parse_session` 里算，而 `parse_session` 只跑在启动 / ⌘R / jsonl 变化上 ——
 * 纯改名不写 jsonl，于是标题要手动刷新才更新（#4 / #8）。
 *
 * 已知边界：**名字被清空**（`r.name` 变空、或进程退出后 pid 文件消失）这两种情况
 * 这里故意不动 display_name。原因是回落目标算不出来：后端的优先级是
 * `rename > customTitle > agentName > 空`，而 `customTitle / agentName` 没有随
 * SessionMeta 下发，前端只有一个已经算好的 display_name。硬清成空会把本该显示
 * customTitle 的会话打成首条消息原文 —— 那是用一个新错误换一个旧错误。留着旧名字，
 * 等下一次全量 / 增量重算（⌘R，或该会话再来一条消息）自己纠正。
 */
export function applyRunningMeta<T extends Mergeable>(s: T, r: RunningMeta | undefined): T {
  if (!r) {
    if (!s.running) return s;
    return { ...s, running: false, status: "idle", waiting_for: "", pid: 0 };
  }
  const next: T = { ...s, running: true, status: r.status, waiting_for: r.waiting_for, pid: r.pid };
  if (r.name) {
    next.display_name = r.name;
    next.name_source = "rename";
  }
  return next;
}
