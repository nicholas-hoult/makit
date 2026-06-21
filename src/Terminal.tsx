import { memo, useEffect, useLayoutEffect, useRef } from "react";
import { terminalManager } from "./TerminalManager";
import "@xterm/xterm/css/xterm.css";

type Props = {
  id: string;
  cwd: string;
  visible: boolean;
  isActive?: boolean;
  initCommand?: string | null;
};

// memo: 只有 id/visible/isActive 真正变化时才重渲染（切 tab 时其他 terminal 不重渲染）
export const TerminalView = memo(function TerminalView({ id, cwd, visible, isActive = true, initCommand }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  // 挂载：把 TerminalManager 中的 xterm element appendChild 到 placeholder
  useLayoutEffect(() => {
    if (!containerRef.current) return;
    if (!terminalManager.has(id)) {
      terminalManager.create(id, cwd, initCommand ?? null);
    }
    terminalManager.mount(id, containerRef.current);
  }, [id, cwd, initCommand]);

  // visible/active 切换时 focus（fit 需要等 reflow 完成，放到下面的 useEffect+rAF）
  useLayoutEffect(() => {
    if (!visible || !isActive) return;
    terminalManager.focus(id);
  }, [visible, isActive, id]);

  // 兜底 fit + focus（rAF 等 CSS reflow 完成后再执行）
  useEffect(() => {
    if (!visible || !isActive) return;
    const raf = requestAnimationFrame(() => {
      terminalManager.fit(id);
      terminalManager.focus(id);
    });
    return () => cancelAnimationFrame(raf);
  }, [visible, isActive, id]);

  return (
    <div
      ref={containerRef}
      className="xterm-host"
      style={{ display: visible ? "block" : "none" }}
    />
  );
});
