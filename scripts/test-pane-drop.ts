/**
 * 拖拽落点提示的出现条件：**只对我们真的能处理的拖拽画浮层**。
 *
 * 这条不变量是 #175 的根因。原来 `handlePaneDragOver` 无条件 `setHoverDrop(...)`，
 * 从 Finder 拖个文件进来也照画。而**外部拖拽根本不派发 `dragstart` / `dragend`**
 * （那两个事件只给页面内发起的拖拽），于是清 `hoverDrop` 的三条路 —— pane 自己的
 * `onDrop`、document 捕获阶段的 `dragend` / `drop` —— 一条都不走：用户把文件拖出窗口
 * 或按 ESC 取消，那层蓝色就永久留在终端上了。
 *
 * 所以谓词必须是**白名单**（"只认这两个 MIME"）而不是黑名单（"排除 Files"）：
 * 外部拖拽能带进来的 MIME 是开放集合，一个都数不完。
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import {
  CONTAINER_TAB_MIME,
  PANE_SPEC_MIME,
  acceptsPaneDrop,
  isTabDrag,
} from "../src/paneDrop.ts";

let pass = 0;
const fails: string[] = [];

function check(name: string, cond: boolean) {
  if (cond) pass++;
  else fails.push(name);
}

// ---- 正面：我们自己发起的两种拖拽必须照旧被接受 --------------------------
// 这两条是回归护栏：谓词写窄了，分屏拖拽会整个失效 —— 那比浮层不消失严重得多。
check("container tab 拖拽被 pane 接受（拖到边缘分屏）", acceptsPaneDrop([CONTAINER_TAB_MIME]));
check("session 卡片拖拽被 pane 接受", acceptsPaneDrop([PANE_SPEC_MIME]));

// ---- 反面：外部拖拽一律不画浮层 ------------------------------------------
check("从 Finder 拖文件（types 里是 Files）不画浮层", !acceptsPaneDrop(["Files"]));
check(
  "Finder 实际会带一整组 MIME，同样不画",
  !acceptsPaneDrop(["Files", "public.file-url", "text/uri-list"]),
);
check("从编辑器拖选中的文本不画浮层", !acceptsPaneDrop(["text/plain"]));
check("拖链接不画浮层", !acceptsPaneDrop(["text/uri-list", "text/plain"]));
check("空 types 不画浮层", !acceptsPaneDrop([]));
check("types 缺失（防御）不画浮层", !acceptsPaneDrop(undefined));

// 白名单语义：长得像我们的 MIME 但不在名单里，也不认。
// 否则以后新加一种 x-ccs-* 拖拽，会在 drop 端还没实现时就先把浮层画出来。
check(
  "白名单：未登记的 x-ccs-* MIME 不画浮层",
  !acceptsPaneDrop(["application/x-ccs-something-new"]),
);

// ---- tab 条的插入线用的是更窄的条件 --------------------------------------
// handleTabBarDrop 只认 CONTAINER_TAB_MIME，所以 session 卡片悬在 tab 条上时
// 画一条「插到这里」的线是在撒谎 —— 松手什么也不会发生。
check("tab 拖拽会画插入线", isTabDrag([CONTAINER_TAB_MIME]));
check("session 卡片不画 tab 插入线（tab 条不处理它）", !isTabDrag([PANE_SPEC_MIME]));
check("文件拖拽不画 tab 插入线", !isTabDrag(["Files"]));

// ---------------------------------------------------------------------------
// 下面扫源码。谓词自己对不对是一回事，真正会复发的是**别处又写了一遍 MIME 字面量**
// 或者**新加的 dragover 忘了问谓词**。
// ---------------------------------------------------------------------------

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (/\.tsx?$/.test(name)) out.push(p);
  }
  return out;
}

const files = walk("src");

// 1) MIME 字面量只许出现在 paneDrop.ts。
// 这不是洁癖：setData 用的字符串和谓词检查的字符串一旦不是同一个，谓词会静静地
// 对我们自己的拖拽返回 false，分屏功能整个失效，而单元测试照样全绿。
const dupMime: string[] = [];
for (const file of files) {
  if (file === "src/paneDrop.ts") continue;
  readFileSync(file, "utf8")
    .split("\n")
    .forEach((line, i) => {
      if (line.includes("application/x-ccs-")) dupMime.push(`${file}:${i + 1}`);
    });
}
check(
  `MIME 字面量只在 paneDrop.ts 里（发现 ${dupMime.length} 处别的：${dupMime.join(", ")}）`,
  dupMime.length === 0,
);

// 2) 凡是会把落点状态设起来的文件，都必须问过谓词。
// 抓的是「新写了一个 onDragOver，直接 setHoverDrop / setDropIdx」这种复发方式。
const unguarded: string[] = [];
for (const file of files) {
  const src = readFileSync(file, "utf8");
  const setsDropState = /setHoverDrop\(\{/.test(src) || /setDropIdx\((?!null)/.test(src);
  if (!setsDropState) continue;
  if (!/acceptsPaneDrop\(|isTabDrag\(/.test(src)) unguarded.push(file);
}
check(
  `设置落点状态的文件都问过谓词（未守卫：${unguarded.join(", ") || "无"}）`,
  unguarded.length === 0,
);

if (fails.length) {
  console.error(`✗ 落点提示规则失败 ${fails.length} 项：`);
  for (const f of fails) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(`✓ 落点提示规则全部通过（${pass} 项）`);
