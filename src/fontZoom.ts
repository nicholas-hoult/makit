import { isCmd } from "./keys.ts";

/**
 * 终端字号缩放：按键判定 + 字号夹取。纯函数。
 *
 * 单独一个模块的理由：这两件事看着简单但都容易错——macOS 上 `+` 必须按 ⇧ 才打得出，
 * 只判 e.key 会漏掉最常用的 ⌘=；夹取边界；还要排除 ⌘⌃/⌘⌥ 组合，否则会撞上
 * App.tsx 里已有的 ⌘⌥ 快捷键。而它们在界面上的表现是「按一下键字变大」，
 * 肉眼回归不了。和 ptySize.ts / treeOrder.ts / sessionStatus.ts 同一个套路。
 *
 * 修饰键判定（metaKey/ctrlKey）由 keys.ts 的 isCmd() 集中管理，避免全局快捷键的修饰键
 * 规则被局部复刻——过去正是因为这样踩过坑（见 keys.ts 自己的注释）。
 */

/** 现在硬编码在 TerminalManager 里创建终端时用的字号，也是 ⌘0 要回到的值。 */
export const DEFAULT_FONT_SIZE = 13;
export const MIN_FONT_SIZE = 8;
export const MAX_FONT_SIZE = 32;

export type ZoomAction = "in" | "out" | "reset";

/**
 * 只取判定需要的字段。code/altKey 是 fontZoom 自己关心的，
 * 修饰键判定（metaKey、是否 ctrl）由 isCmd 负责。
 */
export type ZoomKey = {
  code: string;
  altKey: boolean;
} & Parameters<typeof isCmd>[0];

/**
 * 按键 → 动作。null 代表这个键跟缩放无关，调用方不要拦它（不要 preventDefault）。
 *
 * 按 e.code 不按 e.key：⌘= 和 ⌘⇧+ 在 macOS 上是同一个物理键（Equal），但
 * e.key 分别是 "=" 和 "+"——只判 e.key === "+" 会漏掉更常按的 ⌘=。
 * shiftKey 故意不检查：忽略它就等于「⌘= 和 ⌘⇧+ 都算 in」，不用分别处理。
 *
 * 修饰键判定（⌘、不⌃、不⌥）由 isCmd() 负责，避免重复实现 src/keys.ts 里
 * 已经集中定下来的全局快捷键修饰键规则。
 */
export function zoomAction(e: ZoomKey): ZoomAction | null {
  if (!isCmd(e) || e.altKey) return null;
  switch (e.code) {
    case "Equal":
    case "NumpadAdd":
      return "in";
    case "Minus":
    case "NumpadSubtract":
      return "out";
    case "Digit0":
    case "Numpad0":
      return "reset";
    default:
      return null;
  }
}

/**
 * 当前字号 + 动作 → 新字号（已夹取到 [MIN_FONT_SIZE, MAX_FONT_SIZE]）。
 *
 * 返回值等于 cur 就是撞到边界了（或者 reset 时已经是默认值）——调用方
 * 靠这个判断要不要跳过后续的 fit()，不用单独暴露一个「有没有变」的布尔值。
 */
export function nextFontSize(cur: number, a: ZoomAction): number {
  const raw = a === "in" ? cur + 1 : a === "out" ? cur - 1 : DEFAULT_FONT_SIZE;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, raw));
}
