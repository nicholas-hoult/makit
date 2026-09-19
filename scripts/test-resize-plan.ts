/**
 * 改尺寸的行列分离策略（#203，照 VS Code terminalResizeDebouncer）。
 *
 * 为什么单独测：
 *
 * 1. `planResize` 决定这一帧做什么。实测改行数 ~1ms、改列数要把整段回滚按新宽度重排（5000 行 ~21ms，4 个 pane 叠加
 *    每帧近 50ms）。判错了：该延后的列数被当场重排 → 拖分割线一顿一顿；该当场的行数被延后 → 拖高度时内容不跟手。
 *
 * 2. `createTrailingDebounce` 决定列数什么时候真正重排：连续拖动只能在停手后重排一次，松手（flush）要立即重排，
 *    不能多也不能丢。错了就是「拖完不折行」或「拖动中仍然在折行」。
 */
import { createTrailingDebounce, planResize, SMALL_BUFFER_LINES } from "../src/resizePlan.ts";

let n = 0;
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

// ── createTrailingDebounce：假时钟 ──
let now = 0;
let timers: { at: number; fn: () => void; id: number }[] = [];
let nextId = 1;
const clock = {
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
const d = createTrailingDebounce(() => runs++, 100, clock);
for (let i = 0; i < 30; i++) { d.schedule(); advance(16); } // 拖动约 0.5 秒，每帧一次
eq("连续拖动中不执行", runs, 0);
advance(83); // 循环最后一次 schedule 之后已过 16ms，这里累计 99ms
eq("停手不到 100ms 不执行", runs, 0);
advance(1);
eq("停手满 100ms 执行一次", runs, 1);
advance(1000);
eq("之后不再重复执行", runs, 1);

d.schedule(); advance(30); d.flush();
eq("松手 flush 立即执行", runs, 2);
advance(1000);
eq("flush 后待办已取消，不会再执行一次", runs, 2);
d.flush();
eq("没有待办时 flush 不执行", runs, 2);

d.schedule(); d.dispose(); advance(1000);
eq("dispose 后待办被取消", runs, 2);
d.schedule(); advance(1000);
eq("dispose 后 schedule 无效", runs, 2);

console.log(`✓ 改尺寸行列分离策略全部通过（${n} 项）`);
