import { useCallback, useLayoutEffect, useRef } from "react";

/**
 * 身份永远不变、调用时总是执行最新版本的函数（#219）。
 *
 * 用在传给 memo 组件的回调上：App 里的回调大多是内联写的或依赖一堆 state，每次渲染都是
 * 新引用，于是 `memo(SessionTree)` 形同虚设 —— App 里任何 state 变化（toast、hover、
 * 通知）都让整棵侧栏重渲染。只适用于事件回调（点击、拖拽），不能在渲染期间调用。
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function useStableFn<F extends (...args: any[]) => any>(fn: F): F {
  const ref = useRef(fn);
  useLayoutEffect(() => {
    ref.current = fn;
  });
  return useCallback(((...args) => ref.current(...args)) as F, []);
}
