/**
 * 改尺寸的行列分离策略（#203，照 VS Code terminalResizeDebouncer）。
 *
 * 为什么单独测：
 *
 * 1. `planResize` 决定这一帧做什么。实测改行数 ~1ms、改列数要把整段回滚按新宽度重排（5000 行 ~21ms，4 个 pane 叠加
 *    每帧近 50ms）。判错了：该延后的列数被当场重排 → 拖分割线一顿一顿；该当场的行数被延后 → 拖高度时内容不跟手。
 *
 * 2. `createColsFollower` 决定列数什么时候重排：拖动中每 33ms 跟一次（文字跟手），两次之间不重排（否则每帧
 *    重排整段回滚 = 卡），停手后补最后一次（停在哪就是哪），松手（flush）立即重排。
 *    快了卡、慢了文字跟不上手 —— 实测 5000 行回滚每次重排 ~21ms，所以必须限流；回滚降到 2000 行后 33ms 跟得动。
 */
import { COLS_FOLLOW_MS, createColsFollower, planResize, SMALL_BUFFER_LINES } from "../src/resizePlan.ts";

let n = 0;
function check(name: string, ok: boolean) {
  n++;
  if (!ok) { console.error(`✗ ${name}`); process.exit(1); }
}

function eq(name: string, got: unknown, want: unknown) {
  n++;
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    console.error(`✗ ${name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
    process.exit(1);
  }
}

const cur = { cols: 120, rows: 40 };
const BIG = 5000;

// ── planResize ──
eq("尺寸没变：什么都不做", planResize(cur, { cols: 120, rows: 40 }, BIG, false), { rowsNow: false, colsNow: false, colsLater: false });
eq("只改行数：立即", planResize(cur, { cols: 120, rows: 30 }, BIG, false), { rowsNow: true, colsNow: false, colsLater: false });
eq("只改列数（大缓冲区）：延后", planResize(cur, { cols: 90, rows: 40 }, BIG, false), { rowsNow: false, colsNow: false, colsLater: true });
eq("行列都改（大缓冲区）：行立即、列延后", planResize(cur, { cols: 90, rows: 30 }, BIG, false), { rowsNow: true, colsNow: false, colsLater: true });
eq(`小缓冲区（< ${SMALL_BUFFER_LINES} 行）：列也立即`, planResize(cur, { cols: 90, rows: 30 }, SMALL_BUFFER_LINES - 1, false), { rowsNow: true, colsNow: true, colsLater: false });
eq("显式立即（松手 / 缩放字号 / 最大化）：列也立即", planResize(cur, { cols: 90, rows: 40 }, BIG, true), { rowsNow: false, colsNow: true, colsLater: false });
eq("显式立即但尺寸没变：什么都不做", planResize(cur, { cols: 120, rows: 40 }, BIG, true), { rowsNow: false, colsNow: false, colsLater: false });

// ── createColsFollower：假时钟 ──
let now = 0;
let timers: { at: number; fn: () => void; id: number }[] = [];
let nextId = 1;
const clock = {
  now: () => now,
  setTimeout: (fn: () => void, ms: number) => { const id = nextId++; timers.push({ at: now + ms, fn, id }); return id; },
  clearTimeout: (id: number) => { timers = timers.filter((t) => t.id !== id); },
};
function advance(ms: number) {
  now += ms;
  const due = timers.filter((t) => t.at <= now);
  timers = timers.filter((t) => t.at > now);
  due.forEach((t) => t.fn());
}

let runs = 0;
const f = createColsFollower(() => runs++, COLS_FOLLOW_MS, clock);

f.request();
eq("第一次请求立即跟（起手不延迟）", runs, 1);
advance(16); f.request();
eq("16ms 后再请求：还没到节奏，不跟", runs, 1);
advance(17); f.request();
eq(`满 ${COLS_FOLLOW_MS}ms 再跟一次`, runs, 2);
const runsBeforeDrag = runs;
for (let i = 0; i < 30; i++) { advance(16); f.request(); }  // 继续拖约 0.5 秒（每帧一次请求）
const dragRuns = runs - runsBeforeDrag;
const cadence = (30 * 16) / dragRuns;
// 定时器排在 33ms，但请求只在每帧（16ms）到来，所以实际落在下一帧 → 节奏 33~50ms。
// 关键是「远少于每帧一次、又明显快于停手才折」：30 帧里只重排 8~15 次。
check(`连续拖动按节奏跟随：30 帧重排 ${dragRuns} 次（节奏 ${cadence.toFixed(0)}ms）`, dragRuns >= 8 && dragRuns <= 15);
const beforeIdle = runs;
advance(COLS_FOLLOW_MS);
eq("停手后补最后一次（停在哪就折到哪）", runs, beforeIdle + 1);
advance(1000);
eq("之后不再重复", runs, beforeIdle + 1);

// 拖动中途松手：第一次 request 立即跟（离上次已久），5ms 后再 request 只是排上待办，flush 立刻兑现它
f.request();
eq("久未拖动后再拖，第一次立即跟", runs, beforeIdle + 2);
advance(5); f.request(); f.flush();
eq("松手 flush 立即兑现待办", runs, beforeIdle + 3);
advance(1000);
eq("flush 后待办已取消，不会再跟一次", runs, beforeIdle + 3);

const before = runs;
f.request(); f.dispose(); advance(1000);
eq("dispose 后待办被取消", runs, before + 1);
f.request(); advance(1000);
eq("dispose 后请求无效", runs, before + 1);

console.log(`✓ 改尺寸行列分离与列跟随策略全部通过（${n} 项）`);
