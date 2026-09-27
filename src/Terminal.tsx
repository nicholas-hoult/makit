import { memo, useEffect, useLayoutEffect, useRef } from "react";
import { terminalManager } from "./TerminalManager";
import "@xterm/xterm/css/xterm.css";

type Props = {
  id: string;
  cwd: string;
  visible: boolean;
  isActive?: boolean;
  initCommand?: string | null;
  /// 启动目录不存在时允不允许退到上级目录。resume tab 必须传 false：
  /// 换了目录就换了 claude 的存储键，会话直接找不到（见 TerminalInstance 上的注释）。
  allowCwdFallback?: boolean;
};

// memo: 只有 id/visible/isActive 真正变化时才重渲染（切 tab 时其他 terminal 不重渲染）
export const TerminalView = memo(function TerminalView({ id, cwd, visible, isActive = true, initCommand, allowCwdFallback = true }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  // 挂载：把 TerminalManager 中的 xterm element appendChild 到 placeholder
  useLayoutEffect(() => {
    if (!containerRef.current) return;
    if (!terminalManager.has(id)) {
      terminalManager.create(id, cwd, initCommand ?? null, allowCwdFallback);
    }
    terminalManager.mount(id, containerRef.current);
  }, [id, cwd, initCommand]);

  // 可见性交给 TerminalManager 分配 WebGL 预算（#230）
  useEffect(() => {
    terminalManager.setVisible(id, visible);
  }, [visible, id]);

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
      // 终端右键菜单靠这个属性从事件 target 反查是哪个 pane（App 里的全局
      // contextmenu handler 走 closest("[data-pane-id]")）。xterm 的 DOM 是
      // addon 生成的、层级不固定，从上往下找一个我们自己写的锚点最稳。
      data-pane-id={id}
      style={{ display: visible ? "block" : "none" }}
    />
  );
});
