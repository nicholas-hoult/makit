/**
 * session 的**进程维度**状态。
 *
 * 侧栏一直有两个正交的维度，但界面曾经把它们渲染成一串互斥的分组：
 *
 *   位置维度 —— 这条 session 在我的 tab 里开着吗？（前端 workspace 状态）
 *   进程维度 —— 它的 claude 进程还活着吗、在忙什么？（后端 pid 扫描 + hook 状态）
 *
 * 四种组合都真实存在：tab 开着但 pty 还没起、进程活着但没在这儿打开、两者都有、
 * 两者都没有。而分组链是「先到先得」的，「打开中」排在最前面，于是一条**又开着又
 * 在跑**的 session 只出现在「打开中」组里 —— 组名就暗示了「它不在运行」，是假的。
 *
 * 所以分工定死：**分组只表达位置，进程状态一律由行内的状态点表达**。这个模块是
 * 状态点的唯一数据源，抽出来是为了能把那张 2×2 的映射表钉在测试里 —— 它在 UI 上
 * 是一个 10px 的圆点，肉眼回归不了。
 */

/**
 * 状态的**唯一一套叫法**，侧栏和 ⌘K 命令面板都从这里取（#187）。
 * 以前两边各写一套：侧栏叫「后台运行」、行上一律显示「等待审批」（等你回答问题时也是），
 * ⌘K 叫「工作中」「空闲（运行中）」「已停止（未归档）」—— 同一个会话在两处读起来像两种状态。
 */
export const STATUS_LABEL = {
  waiting_approval: "等待审批",
  waiting_user: "等待回答",
  busy: "进行中",
  idle: "空闲",
  stopped: "已停止",
  archived: "已归档",
} as const;

/** 在等你的会话具体在等什么：`waiting_for === "user"` 是等你回答问题，其余都是等你批准操作 */
export function waitingLabel(waitingFor: string | null | undefined): string {
  return waitingFor === "user" ? STATUS_LABEL.waiting_user : STATUS_LABEL.waiting_approval;
}

/** 状态点的四档。顺序即优先级：等你处理 > 正在跑 > 活着但闲着 > 已停止。 */
export type RunState = "waiting" | "busy" | "idle" | "stopped";

export type RunStateInput = { running: boolean; status: string };

export function runState(s: RunStateInput): RunState {
  if (s.status === "waiting") return "waiting";
  if (s.status === "busy") return "busy";
  if (s.running) return "idle";
  return "stopped";
}

/**
 * 形状只表达**进程生死**：实心 = 还活着，空心 = 已停止。
 *
 * 注意 waiting 走的是 `status` 而不是 `running`：一条在等审批的 session 按定义
 * 就是活着的（它正阻塞在提示上），只有 pid 扫描漏掉它、或它在等待时被杀了才会
 * running=false。这种情况下宁可相信 status —— 行上还挂着「等待审批」，圆点画成
 * 空心会让同一行自相矛盾。
 *
 * 曾经用 ▶ 表示 running：整行本来就可点，一个播放三角会被读成「点这里运行」的
 * 按钮，而不是一个状态位；而且它和同列的 ● / ○ 不是一套形状语言。
 */
export function runStateIcon(st: RunState): string {
  return st === "stopped" ? "○" : "●";
}

/** 颜色表达**它在干什么**。CSS 见 SessionTree.css 的 .tree-status-icon.*。 */
export function runStateClass(st: RunState): string {
  return st;
}

/**
 * 鼠标悬停在状态点上的文字说明。
 *
 * 为什么状态点需要 tooltip，而不是像「等待审批」那样在行里挂个药丸：`idle`
 * （活着但闲着）是**常态**，每一行都挂一个「空闲」药丸信息量为零，却每行都在和
 * 标题抢宽度 —— 之前就是因为这个把「空闲」「已停止」两个药丸删掉的。删掉之后
 * 全部重量压在圆点上，而当时 idle 和 stopped 用的是同一个 var(--fg-muted)，
 * 唯一区别是填充和 0.4 的透明度，于是「区别不明了」。
 *
 * 现在两条路一起走：颜色让 idle / stopped 在扫视层面就能分开（绿 vs 灰），
 * tooltip 负责把这个 10px 的符号翻译成人话，且零横向占用。
 */
export function runStateTitle(st: RunState): string {
  switch (st) {
    // 开头的词和 STATUS_LABEL / 侧栏分组名一致（#187）
    case "waiting":
      return "需要回应：在等你批准操作或回答问题";
    case "busy":
      return `${STATUS_LABEL.busy}：进程活着且在产出`;
    case "idle":
      return `${STATUS_LABEL.idle}：进程活着，点进去可以直接接着用`;
    case "stopped":
      return `${STATUS_LABEL.stopped}：进程不在了，打开会恢复之前的上下文`;
  }
}

/**
 * 这一组里有没有活着的会话 —— 决定要不要给这组画状态点那一列。
 *
 * 为什么要按组开关这一列：分组链是先到先得的，活着的会话一定被上游的「需要回应」/
 * 「后台运行」先拿走，所以掉进「历史」和「收藏」的**定义上**全是 stopped。那两组里
 * 每行都是同一个灰空心点，看它等于没看，却各占 12px 的盒子 + 6px 的间隙 —— 而侧栏
 * 常被拖到 180px 上下用，标题本来就在截断。
 *
 * 按组而不是按行判断，是为了组内左边缘还齐着：混着活的死的的组（「已打开」）整组
 * 画，全死的组整组不画。也不能写成「历史组和收藏组不画」—— 那是把分组链现在的
 * 顺序焊进渲染层，以后谁调一下顺序（比如收藏排到运行前面），这一列就开始说谎。
 * 问的是数据本身，链子怎么改都不会错。
 */
export function anyAlive(list: RunStateInput[]): boolean {
  return list.some((s) => runState(s) !== "stopped");
}
