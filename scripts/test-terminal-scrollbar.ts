/**
 * 终端滚动条的显隐时序（#188）。
 *
 * 为什么单独测：
 *
 * 1. `isUserScroll` 是「这次滚动要不要亮条」的唯一判据。终端在输出时 xterm 也会发 onScroll
 *    （视口跟着底部走），如果把它也算上，Claude 一刷屏条子就一直闪 —— 这个错在 UI 上
 *    很难断言，只能在这里钉住：贴底 = 输出跟随，不亮；离开底部 = 人在翻，亮。
 *
 * 2. `createActivity` 的价值全在边沿：连续 ping 只能 show 一次、停手 idleMs 后 hide 一次、
 *    hide 之后再 ping 要能重新 show。错一个就是「条子闪烁」「永远不藏」「藏了就回不来」。
 *
 * 3. `isInScrollbarGutter`（#206）：指针落在右侧这条窄区域时滚动条要显形，否则没法直接抓住滑块拖
 *    —— 之前隐藏时还设了 pointer-events: none，必须先滚一下才能拖，「想直接拖到底」反而做不到。
 *    判据错了就是「条子乱冒」或「还是抓不住」。
 *
 * 4. `capSlider`（#188 追加）：滑块最长只占轨道 1/4。截短后可移动距离变长，位置必须按比例重映射 ——
 *    否则滚到底时滑块停在半空、或者冲出轨道。错了在 UI 上就是「滚到底了条子还没到底」。
 *
 * 5. `dragToLine`（#228）：截短滑块后，拖动还交给 xterm 的话它按自己原来的滑块长度换算，
 *    滑块比指针走得快 —— 用户说「拖动的时候会自己跑」。改成自己接管拖动，用和 capSlider 同一套
 *    几何换算：滑块必须一直贴着指针。错了在 UI 上就是拖着拖着滑块离开了指针。
 */
import { GUTTER_PX, capSlider, createActivity, dragToLine, isInScrollbarGutter, isUserScroll } from "../src/terminalScrollbar.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  if (JSON.stringify(got) !== JSON.stringify(want)) {
    console.error(`✗ ${name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
    process.exit(1);
  }
}

// ── isUserScroll ──
eq("贴底（输出跟随）不算", isUserScroll(120, 120), false);
eq("离开底部算", isUserScroll(80, 120), true);
eq("没有回滚历史不算", isUserScroll(0, 0), false);
eq("翻到最顶算", isUserScroll(0, 500), true);

// ── createActivity：假时钟 ──
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

const log: string[] = [];
const a = createActivity(() => log.push("show"), () => log.push("hide"), 900, clock);

a.ping();
eq("第一次 ping 立刻 show", log, ["show"]);
a.ping(); advance(100); a.ping(); advance(100); a.ping();
eq("连续 ping 不重复 show", log, ["show"]);
advance(899);
eq("最后一次 ping 后不到 idleMs 不 hide", log, ["show"]);
advance(1);
eq("满 idleMs 后 hide 一次", log, ["show", "hide"]);
advance(5000);
eq("hide 之后不再重复 hide", log, ["show", "hide"]);
a.ping();
eq("hide 之后再 ping 能重新 show", log, ["show", "hide", "show"]);
a.dispose();
eq("dispose 时若在显示则 hide", log, ["show", "hide", "show", "hide"]);
advance(5000);
eq("dispose 后定时器已清", log, ["show", "hide", "show", "hide"]);
a.ping();
eq("dispose 后 ping 无效", log, ["show", "hide", "show", "hide"]);

// ── capSlider：轨道 400，上限 1/4 = 100，最小 20 ──
eq("本来就短于上限：原样", capSlider(400, 60, 170), { size: 60, top: 170 });
eq("等于上限：原样", capSlider(400, 100, 150), { size: 100, top: 150 });
eq("超上限、在顶：截到 100 贴顶", capSlider(400, 320, 0), { size: 100, top: 0 });
eq("超上限、在底：截到 100 贴底", capSlider(400, 320, 80), { size: 100, top: 300 });
eq("超上限、居中：按比例居中", capSlider(400, 320, 40), { size: 100, top: 150 });
eq("整条轨道（不可滚）：截短但贴顶，不除零", capSlider(400, 400, 0), { size: 100, top: 0 });
eq("轨道很短：上限低于最小值时取最小值 20", capSlider(60, 50, 5), { size: 20, top: 20 });
eq("轨道为 0（未布局）：原样", capSlider(0, 0, 0), { size: 0, top: 0 });
eq("自定义上限 1/2", capSlider(400, 320, 80, 0.5), { size: 200, top: 200 });

// ── isInScrollbarGutter：终端右边缘 GUTTER_PX 宽的一条 ──
eq("正好在右边缘：在", isInScrollbarGutter(800, 800), true);
eq(`离右边缘 ${GUTTER_PX - 1}px：在`, isInScrollbarGutter(800 - (GUTTER_PX - 1), 800), true);
eq(`离右边缘 ${GUTTER_PX}px：不在（刚好在界外）`, isInScrollbarGutter(800 - GUTTER_PX, 800), false);
eq("终端中间：不在", isInScrollbarGutter(400, 800), false);
eq("指针跑到终端右侧之外：不在", isInScrollbarGutter(820, 800), false);


// ── dragToLine（#228）：轨道顶 100、高 400，滑块封顶后 100 高，可移动 300；baseY 3000 ──
{
  const g = { trackTop: 100, trackHeight: 400, sliderSize: 100, baseY: 3000 };
  // 在滑块中间（偏移 50）按下，滑块顶在 150（= 第 1500 行），不动
  eq("按下不动：还在原来那行", dragToLine({ ...g, grabOffset: 50, pointerY: 100 + 150 + 50 }), 1500);
  eq("拖到轨道最下面之外：夹到最后一行", dragToLine({ ...g, grabOffset: 50, pointerY: 900 }), 3000);
  eq("拖到轨道上面之外：夹到第 0 行", dragToLine({ ...g, grabOffset: 50, pointerY: 0 }), 0);
  eq("没有可滚内容（baseY 0）：恒为 0", dragToLine({ ...g, baseY: 0, grabOffset: 10, pointerY: 300 }), 0);
  eq("滑块和轨道一样长（无处可移）：0，不除以零", dragToLine({ ...g, sliderSize: 400, grabOffset: 10, pointerY: 300 }), 0);
  // 滑块贴着指针：拖到任意位置后，按这行重新算出来的（封顶后的）滑块顶 = 指针 - 偏移
  const xtermSize = 160;                       // xterm 原本想画的长度（比封顶的 100 长）
  let maxDrift = 0;
  for (let y = 100; y <= 400; y += 7) {   // 滑块顶的合法范围：轨道顶 100 到 100 + 可移动 300
    const line = dragToLine({ ...g, grabOffset: 50, pointerY: y + 50 });
    const xtermTop = (line / g.baseY) * (g.trackHeight - xtermSize);   // xterm 对这行画的位置
    const shown = capSlider(g.trackHeight, xtermSize, xtermTop).top;  // 我们改写后的位置
    maxDrift = Math.max(maxDrift, Math.abs(g.trackTop + shown - y));
  }
  eq("拖动全程滑块贴着指针（误差 ≤1px）", maxDrift <= 1, true);
}

console.log(`✓ 终端滚动条显隐时序与滑块上限全部通过（${n} 项）`);
