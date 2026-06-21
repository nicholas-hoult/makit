import { useEffect, useRef, useState } from "react";
import { TerminalView } from "./Terminal";
import type { ContainerNode, PaneTab, TabKind } from "./workspace-types";

type Props = {
  container: ContainerNode;
  isActive: boolean;
  dragging?: boolean;
  isMaximized?: boolean;
  onToggleMaximize?: () => void;
  getTabTitle?: (tab: PaneTab) => string;
  getTabStatus?: (tab: PaneTab) => "waiting" | "busy" | "idle" | null;
  onActivate: () => void;
  onTabClick: (tabId: string) => void;
  onTabReveal?: (tabId: string) => void;
  onTabClose: (tabId: string) => void;
  onSplit: (dir: "h" | "v") => void;
  onNewShell: () => void;
  onTabDragStart: (e: React.DragEvent, tabId: string) => void;
  onTabDrop: (e: React.DragEvent, idx?: number) => void;
  onTabDragOver: (e: React.DragEvent) => void;
  onPaneDragOver: (e: React.DragEvent) => void;
  onPaneDrop: (e: React.DragEvent) => void;
  onContextMenu: (e: React.MouseEvent) => void;
};

export function ContainerView({
  container,
  isActive,
  dragging,
  isMaximized,
  onToggleMaximize,
  getTabTitle,
  getTabStatus,
  onActivate,
  onTabClick,
  onTabReveal,
  onTabClose,
  onSplit,
  onNewShell,
  onTabDragStart,
  onTabDrop,
  onTabDragOver,
  onPaneDragOver,
  onPaneDrop,
  onContextMenu,
}: Props) {
  function kindIcon(kind: TabKind) {
    if (kind === "resume") return "↻";
    if (kind === "new") return "✦";
    return "$";
  }

  function tabLabel(t: PaneTab) {
    if (getTabTitle) return getTabTitle(t);
    if (t.label) return t.label;
    if (t.sessionShortId) return `[${t.sessionShortId}]`;
    if (t.kind === "shell") return "shell";
    return t.kind;
  }

  // tab 拖拽视觉反馈：dropIdx = 即将插入的索引位置
  const [dropIdx, setDropIdx] = useState<number | null>(null);
  const [draggingTabId, setDraggingTabId] = useState<string | null>(null);
  const tabsRef = useRef<HTMLDivElement>(null);

  // 懒加载：只有被激活过的 tab 才渲染 TerminalView（启动时只初始化 activeTabId）
  const initializedTabsRef = useRef(new Set<string>());
  if (container.activeTabId) initializedTabsRef.current.add(container.activeTabId);

  useEffect(() => {
    const el = tabsRef.current?.querySelector(".container-tab.active") as HTMLElement | null;
    if (el) {
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
      el.classList.add("tab-flash");
      const timer = setTimeout(() => el.classList.remove("tab-flash"), 600);
      return () => clearTimeout(timer);
    }
  }, [container.activeTabId]);

  return (
    <div
      className={"container-view" + (isActive ? " container-active" : "")}
      onMouseDown={onActivate}
      onContextMenu={onContextMenu}
    >
      {/* Mini tab bar */}
      <div
        className="container-tab-bar"
        onDoubleClick={(e) => {
          if ((e.target as HTMLElement).closest(".container-tab, .container-actions")) return;
          onNewShell();
        }}
        onDragOver={onTabDragOver}
        onDrop={(e) => {
          const idx = dropIdx;
          setDropIdx(null);
          setDraggingTabId(null);
          onTabDrop(e, idx ?? undefined);
        }}
        onDragLeave={(e) => {
          const related = e.relatedTarget as Node | null;
          if (related && (e.currentTarget as HTMLElement).contains(related)) return;
          setDropIdx(null);
        }}
      >
        <div className="container-tabs" ref={tabsRef}>
          {container.tabs.map((t, idx) => (
            <div
              key={t.id}
              className={
                "container-tab"
                + (t.id === container.activeTabId ? " active" : "")
                + (draggingTabId === t.id ? " dragging-source" : "")
                + (dropIdx === idx ? " drop-before" : "")
                + (dropIdx === idx + 1 && idx === container.tabs.length - 1 ? " drop-after" : "")
              }
              onClick={() => onTabClick(t.id)}
              draggable
              onDragStart={(e) => {
                setDraggingTabId(t.id);
                onTabDragStart(e, t.id);
              }}
              onDragEnd={() => { setDraggingTabId(null); setDropIdx(null); }}
              onDragOver={(e) => {
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                const before = e.clientX < r.left + r.width / 2;
                setDropIdx(before ? idx : idx + 1);
              }}
            >
              <span
                className="container-tab-icon"
                onClick={(e) => { e.stopPropagation(); onTabReveal?.(t.id); }}
                title="在侧栏定位 session"
              >{kindIcon(t.kind)}</span>
              <span className="container-tab-label">{tabLabel(t)}</span>
              {(() => {
                const st = getTabStatus?.(t);
                if (st === "waiting") return <span className="tab-dot waiting" title="等待输入">●</span>;
                if (st === "busy") return <span className="tab-dot busy" title="工作中">⚡</span>;
                if (st === "idle") return <span className="tab-dot idle" title="空闲">○</span>;
                return null;
              })()}
              <span
                  className="container-tab-close"
                  onClick={(e) => { e.stopPropagation(); onTabClose(t.id); }}
                >
                  ×
                </span>
            </div>
          ))}
        </div>
        <div className="container-actions">
          <button className="container-btn" onClick={onNewShell} title="新终端 (⌘T)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <line x1="8" y1="3" x2="8" y2="13" />
              <line x1="3" y1="8" x2="13" y2="8" />
            </svg>
          </button>
          <button className="container-btn" onClick={() => onSplit("v")} title="左右分屏 (⌘D)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round">
              <rect x="2" y="3" width="12" height="10" rx="1" />
              <line x1="8" y1="3" x2="8" y2="13" />
            </svg>
          </button>
          <button className="container-btn" onClick={() => onSplit("h")} title="上下分屏 (⌘⇧D)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round">
              <rect x="2" y="3" width="12" height="10" rx="1" />
              <line x1="2" y1="8" x2="14" y2="8" />
            </svg>
          </button>
          {onToggleMaximize && (
            <button
              className={"container-btn" + (isMaximized ? " active" : "")}
              onClick={onToggleMaximize}
              title={isMaximized ? "还原 (⌘⌥↩)" : "最大化 (⌘⌥↩)"}
            >
              {isMaximized ? (
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M9 3v4h4M7 13V9H3" />
                  <path d="M9 7l5-5M7 9l-5 5" />
                </svg>
              ) : (
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M3 7V3h4M13 9v4H9" />
                  <path d="M3 3l5 5M13 13l-5-5" />
                </svg>
              )}
            </button>
          )}
        </div>
      </div>

      {/* Terminal area */}
      <div
        className={"container-terminal" + (dragging ? " dragging" : "")}
        onDragOver={onPaneDragOver}
        onDrop={onPaneDrop}
      >
        {container.tabs.length === 0 ? (
          <div className="container-empty">
            <div className="welcome-card">
              <div className="welcome-title">makit</div>
              <div className="welcome-sub">Claude Code Session 管理 · 工作区终端</div>
              <div className="welcome-tip">点侧栏 session 卡片恢复对话；点项目 worktree 起新会话；⌘ 点击为纯 shell</div>
              <div className="welcome-shortcuts">
                <div className="welcome-group">
                  <div className="welcome-group-title">搜索 · 命令</div>
                  <div className="welcome-row"><kbd>⌘K</kbd><span>全局搜索（命令面板）</span></div>
                  <div className="welcome-row"><kbd>⌘F</kbd><span>当前终端内搜索</span></div>
                  <div className="welcome-row"><kbd>⌘⇧F</kbd><span>聚焦侧栏 session 搜索</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">布局</div>
                  <div className="welcome-row"><kbd>⌘D</kbd><span>左右分屏</span></div>
                  <div className="welcome-row"><kbd>⌘⇧D</kbd><span>上下分屏</span></div>
                  <div className="welcome-row"><kbd>⌘B</kbd> / <kbd>⌘\</kbd><span>折叠项目栏 / 会话栏</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">终端</div>
                  <div className="welcome-row"><kbd>⌘T</kbd><span>新建 shell tab</span></div>
                  <div className="welcome-row"><kbd>⌘W</kbd><span>关闭当前 tab</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">导航</div>
                  <div className="welcome-row"><kbd>⌘1</kbd>~<kbd>⌘9</kbd><span>切到第 N 个 container</span></div>
                  <div className="welcome-row"><kbd>⌘⌥</kbd>+方向键<span>上/下一个 container</span></div>
                </div>
              </div>
            </div>
          </div>
        ) : (
          container.tabs
            .filter((t) => initializedTabsRef.current.has(t.id))
            .map((t) => (
              <TerminalView
                key={t.id}
                id={t.id}
                cwd={t.cwd}
                visible={t.id === container.activeTabId}
                isActive={isActive && t.id === container.activeTabId}
                initCommand={t.initCommand}
              />
            ))
        )}
      </div>
    </div>
  );
}
