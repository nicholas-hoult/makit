/**
 * 右键菜单的落点：把「鼠标坐标」修正成「不出屏幕的菜单左上角」。
 *
 * 单独抽出来是因为它**曾经只有一处有**：侧栏 session 菜单写了边界翻转
 * （SessionTree.tsx），pane 容器菜单没写（App.tsx）—— 在屏幕右下角右键，
 * 那个菜单一半在屏幕外，够不着最后一项。菜单从 2 处变 4 处，靠"记得抄"
 * 必然再漏，所以规则收成一个纯函数、由 ContextMenu 组件统一调用。
 */

/// 菜单和屏幕边缘之间留的空隙。8px 是原来 SessionTree 用的值，照搬。
const MARGIN = 8;

export type MenuPos = { x: number; y: number };

/// 输入鼠标位置和菜单实测尺寸，输出修正后的左上角。
///
/// 顺序有讲究：**先推回来，再压住上/左边界**。菜单比视口还高时（菜单项很多 +
/// 窗口很矮），第一步会算出负数，第二步把它按回 MARGIN —— 此时菜单底部仍然超出，
/// 但那由 CSS 的 max-height + overflow-y 接管，滚动比"顶部看不见"好：
/// 顶部被切掉的话第一项永远够不着，而且菜单第一项通常是最常用的那个。
export function clampMenuPosition(
  x: number,
  y: number,
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
): MenuPos {
  let nx = x;
  let ny = y;
  if (nx + width > viewportWidth - MARGIN) nx = viewportWidth - width - MARGIN;
  if (ny + height > viewportHeight - MARGIN) ny = viewportHeight - height - MARGIN;
  if (nx < MARGIN) nx = MARGIN;
  if (ny < MARGIN) ny = MARGIN;
  return { x: nx, y: ny };
}
