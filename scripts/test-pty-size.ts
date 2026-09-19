/**
 * #156（终端花屏 / 重复行）的回归测试。
 *
 * 被测的不变量只有一条：**PTY 最后被告知的尺寸，必须恒等于 xterm 当前的尺寸**。
 * 破了它，程序就会按错的宽度排版，终端自动折行，程序少算行数，旧行留在缓冲区里 ——
 * 那就是花屏，而且永久不自愈。
 *
 * 这里同时跑两套实现：
 *   · `legacy`  复刻旧行为（只有 doFit 带私有去重，另两条路不记账、失败不回滚）
 *   · `ptySize` 现在的共享记账本
 * 前者必须在关键序列上**破掉**不变量（这就是 bug 的证据），后者必须守住。
 * 只断言新实现是不够的 —— 那样测试永远是绿的，看不出它到底防住了什么。
 */
import { claimResize, revertResize, knownSize, forgetPane, clampSize } from "../src/ptySize.ts";

let pass = 0;
const fails: string[] = [];

function eq(name: string, actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) === JSON.stringify(expected)) pass++;
  else fails.push(`${name}\n      期望 ${JSON.stringify(expected)}\n      实际 ${JSON.stringify(actual)}`);
}
function ok(name: string, cond: boolean, extra?: string) {
  if (cond) pass++;
  else fails.push(name + (extra ? `\n      ${extra}` : ""));
}

/* ── 一个最小的世界：xterm 的尺寸 + PTY 以为的尺寸 ───────────────────── */

type Size = { cols: number; rows: number };

/** 旧实现：doFit 有私有去重，fit/fitAll 直接发，失败不回滚 */
function makeLegacy() {
  let lastCols = -1, lastRows = -1;   // doFit 闭包里的那两个变量
  let pty: Size | null = null;         // PTY 真正知道的
  return {
    doFit(s: Size, sendOk = true) {
      if (s.cols !== lastCols || s.rows !== lastRows) {
        lastCols = s.cols; lastRows = s.rows;
        if (sendOk) pty = { ...s };    // 失败被 .catch(() => {}) 吞掉，但 last* 已经改了
      }
    },
    fit(s: Size, sendOk = true) { if (sendOk) pty = { ...s }; },   // 不碰 last*
    fitAll(s: Size, sendOk = true) { if (sendOk) pty = { ...s }; },
    pty: () => pty,
  };
}

/** 新实现：三条路共用 claimResize */
function makeShared(pane: string) {
  forgetPane(pane);
  let pty: Size | null = null;
  const go = (s: Size, sendOk: boolean) => {
    const { send, prev } = claimResize(pane, s);
    if (!send) return;
    if (sendOk) pty = { ...s };
    else revertResize(pane, s, prev);
  };
  return {
    doFit: (s: Size, sendOk = true) => go(s, sendOk),
    fit: (s: Size, sendOk = true) => go(s, sendOk),
    fitAll: (s: Size, sendOk = true) => go(s, sendOk),
    pty: () => pty,
  };
}

const S = (cols: number, rows: number): Size => ({ cols, rows });

/* ── 1. 根因序列：doFit → fit → doFit 回到旧值 ─────────────────────────
   全是日常操作：拖分割线 → 切个 tab 回来 → 再拖回原位。
   第 3 步旧实现判定「没变」而跳过 pty_resize，PTY 停在第 2 步的 100 列。 */
{
  const L = makeLegacy();
  L.doFit(S(80, 24));            // 1. 拖分割线
  L.fit(S(100, 30));             // 2. 切 tab 回来，布局已变
  L.doFit(S(80, 24));            // 3. 拖回原位 —— 旧实现在这里跳过
  ok(
    "【红】旧实现：尺寸回到 doFit 记过的旧值时漏发 pty_resize",
    JSON.stringify(L.pty()) !== JSON.stringify(S(80, 24)),
    `PTY 以为 ${JSON.stringify(L.pty())}，xterm 实际 80x24 —— 差 20 列，程序会按 100 排版`,
  );

  const N = makeShared("p1");
  N.doFit(S(80, 24));
  N.fit(S(100, 30));
  N.doFit(S(80, 24));
  eq("【绿】共享记账本：同一序列下 PTY 跟上了 xterm", N.pty(), S(80, 24));
}

/* ── 2. 反向序列：fitAll 先走，doFit 后走 ─────────────────────────────── */
{
  const L = makeLegacy();
  L.doFit(S(100, 30));           // 拖分割线
  L.fitAll(S(80, 24));           // 分屏 / 放大还原
  L.doFit(S(100, 30));           // 拖回原位 —— 又是旧值
  ok("【红】旧实现：fitAll 之后同样漏发", JSON.stringify(L.pty()) !== JSON.stringify(S(100, 30)));

  const N = makeShared("p2");
  N.doFit(S(100, 30));
  N.fitAll(S(80, 24));
  N.doFit(S(100, 30));
  eq("【绿】共享记账本：fitAll 之后也跟上了", N.pty(), S(100, 30));
}

/* ── 3. 发送失败必须回滚 ───────────────────────────────────────────────
   旧实现 .catch(() => {}) 吞掉失败，但 last* 已经改成新值 → 永久不再重发。 */
{
  const L = makeLegacy();
  L.doFit(S(80, 24));
  L.doFit(S(60, 20), false);     // 这次 invoke 失败
  L.doFit(S(60, 20));            // 下一帧又 fit 到同一尺寸 —— 旧实现认为「发过了」
  ok(
    "【红】旧实现：pty_resize 失败后再也不重发",
    JSON.stringify(L.pty()) !== JSON.stringify(S(60, 20)),
    `PTY 以为 ${JSON.stringify(L.pty())}，xterm 实际 60x20`,
  );

  const N = makeShared("p3");
  N.doFit(S(80, 24));
  N.doFit(S(60, 20), false);
  eq("【绿】失败后账退回去了", knownSize("p3"), S(80, 24));
  N.doFit(S(60, 20));            // 下一次 fit 会重发
  eq("【绿】所以下一次同尺寸的 fit 仍然会发", N.pty(), S(60, 20));
}

/* ── 4. 该去重的还是要去重（不能变成每帧都发 SIGWINCH） ─────────────────
   去重不是可选优化：macOS 全屏动画期间每帧 SIGWINCH 会让 shell 重画 prompt
   累积成 N 行，这是当初写去抖的原因，不能因为修 #156 把它丢掉。 */
{
  forgetPane("p4");
  eq("首次必发", claimResize("p4", S(80, 24)).send, true);
  eq("同尺寸不重发", claimResize("p4", S(80, 24)).send, false);
  eq("只有 cols 变也要发", claimResize("p4", S(81, 24)).send, true);
  eq("只有 rows 变也要发", claimResize("p4", S(81, 25)).send, true);
  eq("再问一次还是不发", claimResize("p4", S(81, 25)).send, false);
}

/* ── 5. pane 销毁后不留账 ──────────────────────────────────────────────
   paneId 复用时若继承上一条命的账，第一次 fit 就会被判成「没变」而跳过。 */
{
  forgetPane("p5");
  claimResize("p5", S(80, 24));
  forgetPane("p5");
  eq("forgetPane 之后 knownSize 是 null", knownSize("p5"), null);
  eq("复用同一个 paneId 时首次必发", claimResize("p5", S(80, 24)).send, true);
  forgetPane("p5");
}

/* ── 6. 不变量全序列扫描 ───────────────────────────────────────────────
   把三条路径按所有长度 3 的组合、在两个尺寸之间来回切，逐序列断言不变量。
   旧实现会破在其中一批上（这批就是「不好复现」的真身：要尺寸**回到**旧值才中），
   新实现必须一条都不破。 */
{
  const sizes = [S(80, 24), S(100, 30)];
  const paths = ["doFit", "fit", "fitAll"] as const;
  let legacyBroken = 0, sharedBroken = 0, total = 0;
  for (const a of paths) for (const b of paths) for (const c of paths)
    for (const i of [0, 1]) for (const j of [0, 1]) for (const k of [0, 1]) {
      total++;
      const seq: [typeof paths[number], Size][] = [[a, sizes[i]], [b, sizes[j]], [c, sizes[k]]];
      const L = makeLegacy();
      for (const [p, s] of seq) L[p](s);
      if (JSON.stringify(L.pty()) !== JSON.stringify(seq[2][1])) legacyBroken++;
      const N = makeShared("scan");
      for (const [p, s] of seq) N[p](s);
      if (JSON.stringify(N.pty()) !== JSON.stringify(seq[2][1])) sharedBroken++;
    }
  forgetPane("scan");
  ok(
    `【红】旧实现在 ${total} 条序列里破了 ${legacyBroken} 条不变量`,
    legacyBroken > 0,
    "旧实现居然一条都没破 —— 那说明这个测试没测到点上",
  );
  // 这个比例就是「不好复现」的量化答案：不是每次 resize 都中，得撞上特定的往返序列
  console.log(`  ↳ 旧实现 ${legacyBroken}/${total} 条序列会造出永久不一致（${Math.round((legacyBroken / total) * 100)}%）`);
  eq(`【绿】共享记账本 ${total} 条序列全部守住不变量`, sharedBroken, 0);
}

/* ── 6.5 迟到的失败回执不许盖掉新的成功记账 ─────────────────────────────
   `pty_resize` 是异步的：A 发 100 失败、B 随后发 80 成功，A 的失败回执才回来。
   无条件退账会把账盖回 100，而 PTY 其实在 80 —— 下一次 fit 到 80 又被判「没变」而跳过。
   退账变成 #156 本身。所以退账必须是「账还是我记的那一笔」才生效。 */
{
  forgetPane("p6");
  const a = claimResize("p6", S(100, 30));      // A 发 100，之后会失败
  claimResize("p6", S(80, 24));                 // B 发 80，成功；账现在是 80
  revertResize("p6", S(100, 30), a.prev);       // A 的失败回执迟到
  eq("迟到的退账被忽略", knownSize("p6"), S(80, 24));
  eq("所以同尺寸的下一次 fit 仍然被正确去重", claimResize("p6", S(80, 24)).send, false);
  // 而没有人后发时，退账照常生效
  const c = claimResize("p6", S(120, 40));
  revertResize("p6", S(120, 40), c.prev);
  eq("没人后发时退账生效", knownSize("p6"), S(80, 24));
  forgetPane("p6");
}

/* ── 7. 下限夹取：夹完的尺寸必须能同时给 xterm ─────────────────────────
   旧代码 `Math.max(20, terminal.cols)` 只抬 PTY 那一边 —— 50px 宽的 pane 里 xterm 只有
   7 列却告诉 PTY 20 列，和漏发一次 resize 完全一样的后果。夹取本身要留，
   但它必须是个纯函数，由调用方把 xterm 一起夹过去（`fitSize` in TerminalManager）。 */
{
  eq("正常尺寸不动", clampSize(S(80, 24)), S(80, 24));
  eq("过窄抬到 20 列", clampSize(S(7, 24)), S(20, 24));
  eq("过矮抬到 5 行", clampSize(S(80, 2)), S(80, 5));
  eq("边界值本身不动", clampSize(S(20, 5)), S(20, 5));
  // 夹取是幂等的 —— 否则 fitSize 里的 terminal.resize 会和下一次 fit 互相拉扯
  eq("夹两次和夹一次一样", clampSize(clampSize(S(3, 1))), clampSize(S(3, 1)));
  const from = S(80, 24);
  clampSize(from);
  eq("不改入参", from, S(80, 24));
}

if (fails.length) {
  console.error(`✗ PTY 尺寸记账 ${fails.length} 项失败（通过 ${pass} 项）：`);
  for (const f of fails) console.error("  · " + f);
  process.exit(1);
}
console.log(`✓ PTY 尺寸记账全部通过（${pass} 项）`);
