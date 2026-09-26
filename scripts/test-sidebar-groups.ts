/**
 * 侧栏分组（#187 第二期）：状态分组链、历史按日期分段、项目组内的优先级排序、统一的状态词表。
 *
 * 为什么单独测：
 * - 分组链是「先到先得」的，一条会话只出现在它符合条件的最靠前那一组。顺序错一位，界面上就是
 *   「这条明明在跑却躺在历史里」「置顶的会话找不到」—— 只有盯着列表数才能发现。
 * - 日期分段按**本地日历日**切，不是按 24 小时滚动窗口；跨午夜、跨月错了，UI 上表现为昨天的会话
 *   出现在「今天」下面，很难肉眼回归。
 * - 状态叫法以前侧栏和 ⌘K 各写一套（侧栏把「等待回答」也显示成「等待审批」），这里钉住只有一套。
 */
import { statusGroups, dayBuckets, priorityCompare, pageProject, PROJECT_PAGE } from "../src/sidebarGroups.ts";
import { STATUS_LABEL, waitingLabel } from "../src/sessionStatus.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}

type S = { session_id: string; running: boolean; status: string; mtime: number };
const mk = (id: string, running: boolean, status: string, mtime: number): S => ({ session_id: id, running, status, mtime });
const ids = (l: { session_id: string }[]) => l.map((s) => s.session_id);

// ── 状态分组链：置顶 → 已打开 → 需要回应 → 进行中 → 空闲 → 历史 ──
const all = [
  mk("wait", true, "waiting", 50),
  mk("busy", true, "busy", 40),
  mk("idle", true, "idle", 30),
  mk("dead", false, "", 20),
  mk("pinDead", false, "", 10),
  mk("pinBusy", true, "busy", 60),
  mk("openIdle", true, "idle", 5),
  mk("openPinned", false, "", 70),
];
const g = statusGroups(all, {
  openedIndex: new Map([["openIdle", 0], ["openPinned", 1]]),
  pinned: new Set(["pinDead", "pinBusy", "openPinned"]),
  sort: (a, b) => b.mtime - a.mtime,
});
eq("置顶在最上面，且拿走所有置顶的（哪怕在跑、哪怕开着）", ids(g.pinned), ["openPinned", "pinBusy", "pinDead"]);
eq("已打开：置顶拿走的不再重复", ids(g.opened), ["openIdle"]);
eq("需要回应", ids(g.attention), ["wait"]);
eq("进行中", ids(g.busy), ["busy"]);
eq("空闲", ids(g.idle), ["idle"]);
eq("历史 = 剩下的已停止", ids(g.history), ["dead"]);
eq("每条会话恰好出现一次", [...g.pinned, ...g.opened, ...g.attention, ...g.busy, ...g.idle, ...g.history].length, all.length);

// 已打开按屏幕上的标签顺序，不按时间
const g2 = statusGroups([mk("a", true, "idle", 1), mk("b", true, "idle", 9)], {
  openedIndex: new Map([["a", 0], ["b", 1]]), pinned: new Set(), sort: (x, y) => y.mtime - x.mtime,
});
eq("已打开按标签顺序", ids(g2.opened), ["a", "b"]);

// ── 历史按本地日历日分段 ──
const now = new Date(2026, 8, 24, 10, 0, 0);              // 2026-09-24 10:00 本地
const t = (y: number, mo: number, d: number, h = 12) => new Date(y, mo - 1, d, h).getTime() / 1000;
const hist = [
  mk("today-early", false, "", t(2026, 9, 24, 0)),
  mk("yest-late", false, "", t(2026, 9, 23, 23)),        // 不到 24 小时前，但属于昨天
  mk("d2", false, "", t(2026, 9, 22)),
  mk("d6", false, "", t(2026, 9, 18)),
  mk("d7", false, "", t(2026, 9, 17)),                   // 第 7 天起算「更早」
  mk("lastMonth", false, "", t(2026, 8, 31)),
];
const b = dayBuckets(hist, now);
eq("分段标签", b.map((x) => x.label), ["今天", "昨天", "9月22日", "9月18日", "更早"]);
eq("今天", ids(b[0].list), ["today-early"]);
eq("跨午夜：不到 24 小时但属于昨天", ids(b[1].list), ["yest-late"]);
eq("更早", ids(b[4].list), ["d7", "lastMonth"]);
eq("组 id 稳定（折叠状态靠它存，不能随日期变）", [b[0].id, b[1].id, b[4].id], ["day-today", "day-yesterday", "day-older"]);
eq("空段不出现", dayBuckets([], now).length, 0);
// 跨月：10 月 1 日看 9 月 30 日是昨天
eq("跨月的昨天", dayBuckets([mk("x", false, "", t(2026, 9, 30))], new Date(2026, 9, 1, 8)).map((x) => x.label), ["昨天"]);

// ── 项目组内的优先级排序：需要回应 → 进行中 → 空闲 → 已停止，同级按最近活动 ──
const p = [mk("s-old", false, "", 1), mk("i", true, "idle", 5), mk("s-new", false, "", 9), mk("w", true, "waiting", 2), mk("b", true, "busy", 3)];
eq("优先级排序", ids([...p].sort(priorityCompare)), ["w", "b", "i", "s-new", "s-old"]);

// ── 统一的状态词表 ──
eq("词表", [STATUS_LABEL.waiting_approval, STATUS_LABEL.waiting_user, STATUS_LABEL.busy, STATUS_LABEL.idle, STATUS_LABEL.stopped, STATUS_LABEL.archived],
   ["等待审批", "等待回答", "进行中", "空闲", "已停止", "已归档"]);
eq("等你回答问题时显示「等待回答」（以前侧栏一律显示等待审批）", waitingLabel("user"), "等待回答");
eq("其余等待显示「等待审批」", waitingLabel("permission"), "等待审批");
eq("没给原因也按等待审批", waitingLabel(""), "等待审批");

// ── 项目组分页（#219）：一个项目 250 条会话，展开时一次渲染全部要卡 1 秒以上 ──
// 渲染和 ↑↓ 导航必须用同一份截断结果，否则 ↑↓ 会走到没渲染出来的行上。
{
  const many = Array.from({ length: 120 }, (_, i) => ({ session_id: `s${i}` }));
  const p1 = pageProject(many, undefined, null);
  eq("默认只显示第一页", [p1.shown.length, p1.hidden], [PROJECT_PAGE, 120 - PROJECT_PAGE]);
  eq("按原顺序取前面的", p1.shown[0].session_id, "s0");
  const p2 = pageProject(many, 100, null);
  eq("点过「显示更多」的按记住的上限", [p2.shown.length, p2.hidden], [100, 20]);
  const p3 = pageProject(many, undefined, "s87");
  eq("⌘L 定位的会话在后面 → 放宽到把它包进来（按页取整）", [p3.shown.length, p3.shown.some((s) => s.session_id === "s87")], [100, true]);
  const small = many.slice(0, 7);
  const p4 = pageProject(small, undefined, null);
  eq("不满一页的原样返回（同一个数组，不白分配）", [p4.shown === small, p4.hidden], [true, 0]);
  eq("上限超过总数 → 全显示", pageProject(many, 500, null).hidden, 0);
}

console.log(`✓ 侧栏分组、日期分段、优先级排序、状态词表全部通过（${n} 项）`);
