/// 侧栏键盘导航。
///
/// 这里只有一个纯函数，因为导航的难点全在边界，而边界只在数据层可测：
/// 列表为空、选中项被搜索筛掉了（selectedId 还在 state 里但已不在渲染顺序里）、
/// 走到首尾。

export type NavDir = "up" | "down";

/// 在**这一帧真的渲染出来的行**（`order`）里移动选中，返回新的选中 id。
///
/// - `order` 为空 → null：没有可选的东西，选中态必须清掉。
/// - `current` 为 null，或已不在 `order` 里（被搜索/筛选去掉了）→ 回到第一条。
///   两个方向都是第一条：↑ 若去选最后一条，在 192 条的历史列表里会直接把视图甩到
///   底部，用户以为自己按错了键。
/// - 走到首/尾**停住，返回 current 自己**，不循环 —— 长列表里循环会让人失去位置感。
///
/// 不认识「组」：`order` 已经是拉平的渲染顺序，所以跨组连续移动是免费的。
export function moveSelection(order: string[], current: string | null, dir: NavDir): string | null {
  if (order.length === 0) return null;
  const i = current === null ? -1 : order.indexOf(current);
  if (i < 0) return order[0];
  const next = dir === "down" ? i + 1 : i - 1;
  if (next < 0 || next >= order.length) return order[i];
  return order[next];
}
