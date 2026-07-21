import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { clampMenuPosition } from "./menuPosition";

/**
 * 全 app 唯一的右键菜单外壳。
 *
 * 抽出来的理由是它承担的四件事**每个菜单都要做，而且已经做分叉了**：
 * 屏幕边界翻转（原来只有侧栏菜单有，pane 菜单没有，右下角右键就出屏）、
 * 点外面关闭、Esc 关闭（原来两处都没有）、统一长相（原来两套 CSS 的边框、
 * 阴影、min-width、按钮 padding、z-index 全不一样）。
 *
 * 调用方只负责一件事：给一个菜单项数组。
 */

/// 分隔线用 `{ sep: true }`，普通项给 label + onClick。
/// disabled 而不是"不显示"：菜单项数量跟着上下文跳动的话，用户每次都要重新
/// 找那一项在第几行；灰掉能同时传达"这里有这个功能"和"这次用不了"。
export type MenuItem =
  | { sep: true }
  | { sep?: false; label: string; onClick: () => void; disabled?: boolean };

type Props = {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
};

export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  // useLayoutEffect：测量要在 paint 之前做完，否则菜单会先画在鼠标处再跳一下。
  // deps 里放 items.length 而不是 items —— items 每次渲染都是新数组，会和
  // setPos 组成无限循环；而菜单内容对尺寸的影响只有项数（宽度由 CSS 的
  // min-width 和最长一项决定，同一个菜单里不会变）。
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos(clampMenuPosition(x, y, r.width, r.height, window.innerWidth, window.innerHeight));
  }, [x, y, items.length]);

  // 捕获阶段：菜单开着的时候 Esc 只归菜单。冒泡阶段挂的话，Esc 会被同时按到的
  // 别的 handler（搜索条、面板）一起吃掉，看起来就是"按一下关了两个东西"。
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div
      className="context-menu-backdrop"
      onClick={onClose}
      // 菜单开着时在别处右键：先关掉。底层元素收不到这次 contextmenu（被 backdrop
      // 挡住了），所以是"关掉旧菜单"而不是"换一个新菜单"，两次右键才能挪位置。
      // 够用，且比"右键穿透 backdrop"简单得多。
      onContextMenu={(e) => { e.preventDefault(); onClose(); }}
    >
      <div
        ref={ref}
        className="context-menu"
        style={{ left: pos.x, top: pos.y }}
        onClick={(e) => e.stopPropagation()}
      >
        {items.map((item, i) =>
          item.sep ? (
            <div key={i} className="context-menu-sep" />
          ) : (
            <button
              key={i}
              disabled={item.disabled}
              onClick={() => { item.onClick(); onClose(); }}
            >
              {item.label}
            </button>
          )
        )}
      </div>
    </div>
  );
}
