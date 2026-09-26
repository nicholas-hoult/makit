// 性能埋点（#218）：启动、交互（点标签 / 侧栏 / 快捷键）、打开终端的分阶段耗时，写进
// ~/.claude/makit/perf.log（Rust 侧 perf.rs）。纯逻辑在 perfCore.ts。
//
// 不常驻 rAF：空闲时一直跑 rAF 会让 WebView 持续出帧、费电。只在每次交互之后看 1 秒的帧。
// 这个模块要在 main.tsx 里最先 import —— 「JS 开始执行」的时刻就是它被执行的时刻。
import { invoke } from "@tauri-apps/api/core";
import { interactionKind, frameStats, stages } from "./perfCore";
import { isCmd } from "./keys";

const epochNow = () => performance.timeOrigin + performance.now();

// ── 批量落盘：每秒最多一次 IPC ──
let queue: object[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
function emit(event: object) {
  queue.push(event);
  if (flushTimer === null) {
    flushTimer = setTimeout(() => {
      flushTimer = null;
      const events = queue;
      queue = [];
      invoke("perf_record", { events }).catch(() => {});
    }, 1000);
  }
}

// ── 启动 ──
const startupMarks: { stage: string; at: number }[] = [{ stage: "JS 开始执行", at: epochNow() }];
requestAnimationFrame(() => startupMarks.push({ stage: "首帧绘制", at: epochNow() }));
let startupReported = false;

/** 侧栏第一次带着数据画出来之后调用（调用方负责等到那一帧） */
export function reportStartup(sessionCount: number) {
  if (startupReported) return;
  startupReported = true;
  startupMarks.push({ stage: "侧栏有数据", at: epochNow() });
  invoke<{ process_start: number; marks: [string, number][] }>("perf_startup")
    .then(({ process_start, marks }) => {
      const all = [...marks.map(([stage, at]) => ({ stage, at })), ...startupMarks];
      const timeline = stages(process_start, all);
      emit({ kind: "startup", ms: timeline[timeline.length - 1]?.at, sessions: sessionCount, stages: timeline });
    })
    .catch(() => {});
}

// ── 交互：点击之后 1 秒内的帧 ──
type Watch = { start: number; frames: number[]; done: (frames: number[]) => void };
const watches: Watch[] = [];
let rafRunning = false;
function frameLoop(now: number) {
  // rAF 的时间戳和 performance.now() 同一个时钟
  for (let i = watches.length - 1; i >= 0; i--) {
    const w = watches[i];
    w.frames.push(now);
    if (now - w.start >= 1000) {
      watches.splice(i, 1);
      w.done(w.frames);
    }
  }
  if (watches.length > 0) requestAnimationFrame(frameLoop);
  else rafRunning = false;
}
function watchFrames(start: number, done: (frames: number[]) => void) {
  watches.push({ start, frames: [], done });
  if (!rafRunning) {
    rafRunning = true;
    requestAnimationFrame(frameLoop);
  }
}

/** 最近一次交互：打开终端的时间线从这里起算（点侧栏会话 → 终端出字） */
let lastInteraction: { what: string; at: number } | null = null;

function classChain(el: Element | null): string[] {
  const out: string[] = [];
  for (let e = el; e && out.length < 8; e = e.parentElement) {
    if (typeof e.className === "string" && e.className) out.push(e.className);
  }
  return out;
}

function trackInteraction(what: string, label: string, always: boolean) {
  const start = performance.now();
  lastInteraction = { what, at: start };
  watchFrames(start, (frames) => {
    const s = frameStats(start, frames);
    // 标签 / 侧栏 / 快捷键每次都记；其他点击只记慢的
    if (!always && !(s.toPaint !== null && s.toPaint > 100) && s.longFrames === 0) return;
    emit({ kind: "interaction", what, label, ...s });
  });
}

window.addEventListener("pointerdown", (ev) => {
  const target = ev.target as Element | null;
  const chain = classChain(target);
  const kind = interactionKind(chain);
  // 标签 / 会话行记文字；其他点击记最近一层的 class，日志里才知道点的是什么
  const label = kind === "other"
    ? (chain[0] ?? target?.tagName?.toLowerCase() ?? "").split(/\s+/)[0]
    : (target?.closest(".container-tab, .tree-session")?.textContent ?? "").trim().slice(0, 20);
  trackInteraction(kind, label, kind !== "other");
}, true);

window.addEventListener("keydown", (ev) => {
  if (!isCmd(ev) || ev.repeat) return;
  if (ev.key === "Meta" || ev.key === "Control" || ev.key === "Shift" || ev.key === "Alt") return;
  trackInteraction(`key:⌘${ev.shiftKey ? "⇧" : ""}${ev.key.toLowerCase()}`, "", true);
}, true);

// ── 打开终端：从触发它的那次交互算起 ──
export function terminalSpan(paneId: string, kind: string) {
  const now = performance.now();
  // 2 秒内有交互就算是它触发的（点侧栏会话 / 新建标签）；否则是启动恢复出来的
  const origin = lastInteraction && now - lastInteraction.at < 2000 ? lastInteraction : null;
  const zero = origin ? origin.at : now;
  const marks: { stage: string; at: number }[] = [{ stage: "终端开始创建", at: now }];
  let ended = false;
  return {
    mark(stage: string) {
      if (!ended) marks.push({ stage, at: performance.now() });
    },
    end(stage: string) {
      if (ended) return;
      ended = true;
      marks.push({ stage, at: performance.now() });
      const timeline = stages(zero, marks);
      emit({
        kind: "terminal-open",
        pane: paneId.slice(0, 8),
        tab: kind,
        trigger: origin ? origin.what : "restore",
        ms: timeline[timeline.length - 1]?.at,
        stages: timeline,
      });
    },
  };
}
