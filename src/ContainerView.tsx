import { useEffect, useRef, useState } from "react";
import { STATUS_LABEL } from "./sessionStatus";
import { TerminalView } from "./Terminal";
import type { ContainerNode, PaneTab, TabKind } from "./workspace-types";
import { isTabDrag } from "./paneDrop";

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
  onTabContextMenu?: (e: React.MouseEvent, tabId: string) => void;
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
  onTabContextMenu,
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

  // active tab 溢出到视野外时滚回来（tab 条窄、tab 多时必需）。
  // 这里曾经还加过 tab-flash：切 tab 时整块刷 0.6s 饱和 accent。已删 —— 见 App.css 里
  // tab-flash 那段注释，简言之首次挂载也会触发，启动时满屏蓝块，而它本身没有信息量。
  useEffect(() => {
    const el = tabsRef.current?.querySelector(".container-tab.active") as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
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
              // stopPropagation 是这里的要点：不拦的话事件冒到 container-view
              // 根节点（下面 onContextMenu），弹出来的是 pane 菜单，而那个菜单的
              // 「关闭」关的是 activeTabId —— 在非激活 tab 上右键点关闭会关掉别人。
              onContextMenu={(e) => {
                if (!onTabContextMenu) return;
                e.preventDefault();
                e.stopPropagation();
                onTabContextMenu(e, t.id);
              }}
              draggable
              onDragStart={(e) => {
                setDraggingTabId(t.id);
                onTabDragStart(e, t.id);
              }}
              onDragEnd={() => { setDraggingTabId(null); setDropIdx(null); }}
              onDragOver={(e) => {
                e.preventDefault();
                // 同 #175：只有 tab 拖拽才画「插到这里」的竖线。tab 条的 drop 只认
                // CONTAINER_TAB_MIME，所以文件或 session 卡片悬在这儿时画线是句谎话，
                // 而外部拖拽按 ESC 取消不派发 dragend，那条线会留着。
                if (!isTabDrag(e.dataTransfer.types)) return;
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
                if (st === "waiting") return <span className="tab-dot waiting" title="需要回应">●</span>;
                if (st === "busy") return <span className="tab-dot busy" title={STATUS_LABEL.busy}>⚡</span>;
                if (st === "idle") return <span className="tab-dot idle" title={STATUS_LABEL.idle}>○</span>;
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
              {/* 键位表按"轴"分组而不是按功能堆：⌘ = tab 轴，⌥⌘ = pane 轴。
                  这样表格本身就在教那条规则，不用逐条记。
                  每行左列固定宽度右对齐（见 .welcome-row 的 grid），否则键帽宽度不等、
                  右边的说明文字会参差不齐。 */}
              <div className="welcome-shortcuts">
                <div className="welcome-group">
                  <div className="welcome-group-title">搜索 · 命令</div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘K</kbd></span><span>全局搜索（命令面板）</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘F</kbd></span><span>当前终端内搜索</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘⇧F</kbd></span><span>侧栏 session 搜索</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘I</kbd></span><span>通知中心</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘R</kbd></span><span>刷新 session 列表</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">Tab · ⌘ 轴</div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘T</kbd></span><span>新建 shell tab</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘W</kbd></span><span>关闭当前 tab</span></div>
                  {/* 修饰键和方向键分成两个键帽：四个箭头挤在一个键帽里，← → 会连成一根长箭头 */}
                  <div className="welcome-row"><span className="keys"><kbd>⌘</kbd><kbd>← → ↑ ↓</kbd></span><span>上 / 下一个 tab</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘[</kbd><kbd>⌘]</kbd></span><span>同上（浏览器习惯）</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌃⇧Tab</kbd><kbd>⌃Tab</kbd></span><span>同上（VS Code 习惯）</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘1</kbd><i>–</i><kbd>⌘9</kbd></span><span>第 N 个 tab</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">Pane · ⌥⌘ 轴</div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘D</kbd></span><span>左右分屏</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘⇧D</kbd></span><span>上下分屏</span></div>
                  {/* 修饰键和方向键分成两个键帽：四个箭头挤在一个键帽里，← → 会连成一根长箭头 */}
                  <div className="welcome-row"><span className="keys"><kbd>⌥⌘</kbd><kbd>← → ↑ ↓</kbd></span><span>切到相邻 pane</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌥⌘1</kbd><i>–</i><kbd>⌥⌘9</kbd></span><span>第 N 个 pane</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌥⌘↩</kbd></span><span>最大化 / 还原 pane</span></div>
                </div>
                <div className="welcome-group">
                  <div className="welcome-group-title">侧栏 · 面板</div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘B</kbd></span><span>折叠项目列表</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘L</kbd></span><span>在侧栏定位当前 session</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>↑</kbd><kbd>↓</kbd></span><span>面板内上下选择</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>⌘↩</kbd></span><span>命令面板：在 split 打开</span></div>
                  <div className="welcome-row"><span className="keys"><kbd>Esc</kbd></span><span>关闭面板 / 取消</span></div>
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
                // resume tab 的启动目录不能悄悄换：换了就等于换了 claude 的存储键，
                // 会话直接找不到。目录没了就让它启动失败，交给恢复流程。
                allowCwdFallback={t.kind !== "resume"}
              />
            ))
        )}
      </div>
    </div>
  );
}
