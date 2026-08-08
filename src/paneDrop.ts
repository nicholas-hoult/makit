/**
 * 拖拽落点的判定：**这次拖拽我们到底接不接**。
 *
 * 抽成独立模块有两个目的，都不是为了"整洁"：
 *
 * 1. **让 MIME 只有一处定义**。setData 用的字符串和 dragover 里检查的字符串必须是
 *    同一个，否则谓词会静静地对我们自己的拖拽返回 false，分屏整个失效。原来这两个
 *    MIME 一个定义在 `App.tsx`、一个定义在 `WorkspaceView.tsx`，还有一处是直接写死
 *    的字面量。
 * 2. **能测**。`scripts/test-pane-drop.ts` 拿这两个谓词跑真实的 types 组合。
 *
 * 为什么需要这道判定（#175）：`dragover` 对**任何**拖拽都会派发，包括从 Finder 拖
 * 进来的文件。原来 `handlePaneDragOver` 无条件 `setHoverDrop(...)`，于是文件悬在终端
 * 上也画出"松手就分屏"的浮层 —— 那是句谎话，drop 端根本不认文件。更要命的是
 * **外部拖拽不派发 `dragstart` / `dragend`**（那两个事件只给页面内发起的拖拽），
 * 所以清 `hoverDrop` 的三条路 —— pane 自己的 `onDrop`、document 捕获阶段的
 * `dragend` / `drop` —— 一条都不走：用户把文件拖出窗口或按 ESC 取消，那层蓝色就
 * 永久留在终端上，只有再拖一次并正常松手才能清掉。
 *
 * 判定必须是**白名单**而不是"排除 Files"式的黑名单：外部拖拽能带进来的 MIME 是个
 * 开放集合（`Files`、`public.file-url`、`text/uri-list`、各种私有 UTI…），数不完。
 */

/** container tab 之间拖动（tab 条内重排 / 跨 container 移动 / 拖到边缘分屏） */
export const CONTAINER_TAB_MIME = "application/x-ccs-container-tab";

/** 侧栏 session 卡片拖进工作区（新开一个 pane） */
export const PANE_SPEC_MIME = "application/x-ccs-pane-spec";

/**
 * `DataTransfer.types` 在 `dragover` 阶段就可读（`getData()` 不行 —— 拖拽进行中
 * 出于隐私被屏蔽），所以判定只能靠它。规范保证里面的 MIME 是小写。
 */
function has(types: readonly string[] | undefined, mime: string): boolean {
  return types ? types.includes(mime) : false;
}

/** tab 条的「插到这里」指示线：只有 tab 拖拽算 —— `handleTabBarDrop` 也只认它 */
export function isTabDrag(types: readonly string[] | undefined): boolean {
  return has(types, CONTAINER_TAB_MIME);
}

/** pane 的落点浮层（`.pane-drop-overlay`）：tab 和 session 卡片都能落 */
export function acceptsPaneDrop(types: readonly string[] | undefined): boolean {
  return isTabDrag(types) || has(types, PANE_SPEC_MIME);
}
