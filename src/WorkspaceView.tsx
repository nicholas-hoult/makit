import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ContainerView } from "./ContainerView";
import { terminalManager } from "./TerminalManager";
import type { LayoutNode, SplitNode, WorkspaceState, Rect } from "./workspace-types";
import { layoutTree, setSplitRatio } from "./workspace-types";
import { CONTAINER_TAB_MIME, PANE_SPEC_MIME, acceptsPaneDrop } from "./paneDrop";
import { formatPathsForTerminal, parseDroppedPaths } from "./dropPaths";

type DropZone = { dir: "h" | "v"; side: "before" | "after" };

function computeDropZone(rect: DOMRect, mx: number, my: number): DropZone {
  const lx = (mx - rect.left) / rect.width;
  const ly = (my - rect.top) / rect.height;
  const dl = lx, dr = 1 - lx, dt = ly, db = 1 - ly;
  const min = Math.min(dl, dr, dt, db);
  if (min === dl) return { dir: "v", side: "before" };
  if (min === dr) return { dir: "v", side: "after" };
  if (min === dt) return { dir: "h", side: "before" };
  return { dir: "h", side: "after" };
}

type Props = {
  workspace: WorkspaceState;
  onSetActive: (containerId: string) => void;
  onTabClick: (containerId: string, tabId: string) => void;
  onTabReveal?: (containerId: string, tabId: string) => void;
  onTabClose: (containerId: string, tabId: string) => void;
  onSplit: (containerId: string, dir: "h" | "v") => void;
  onNewShell: (containerId: string) => void;
  onMoveTab: (srcContainerId: string, tabId: string, destContainerId: string, targetIdx?: number) => void;
  onTabReorder?: (containerId: string, tabId: string, targetIdx: number) => void;
  onSplitWithTab: (srcContainerId: string, tabId: string, targetContainerId: string, dir: "h" | "v", side: "before" | "after") => void;
  onDropSession?: (containerId: string, spec: { kind: "resume" | "new" | "shell"; cwd: string; initCommand: string | null; sessionId: string | null; sessionShortId: string | null }) => void;
  onSplitWithSession: (containerId: string, dir: "h" | "v", side: "before" | "after", spec: { kind: "resume" | "new" | "shell"; cwd: string; initCommand: string | null; sessionId: string | null; sessionShortId: string | null }) => void;
  onUpdateRoot: (newRoot: LayoutNode) => void;
  onToggleMaximize?: (containerId: string) => void;
  onContextMenu: (containerId: string, x: number, y: number) => void;
  onTabContextMenu?: (containerId: string, tabId: string, x: number, y: number) => void;
  getTabTitle?: (tab: import("./workspace-types").PaneTab) => string;
  getTabStatus?: (tab: import("./workspace-types").PaneTab) => "waiting" | "busy" | "idle" | null;
};

export function WorkspaceView({
  workspace,
  onSetActive,
  onTabClick,
  onTabReveal,
  onTabClose,
  onSplit,
  onNewShell,
  onMoveTab,
  onTabReorder,
  onSplitWithTab,
  onDropSession: _onDropSession,
  onSplitWithSession,
  onUpdateRoot,
  onToggleMaximize,
  onContextMenu,
  onTabContextMenu,
  getTabTitle,
  getTabStatus,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [sizeReady, setSizeReady] = useState(false);
  const [hoverDrop, setHoverDrop] = useState<{ containerId: string; zone: DropZone } | null>(null);
  const [dragging, setDragging] = useState(false);

  useLayoutEffect(() => {
    if (!containerRef.current) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0].contentRect;
      setSize({ width: r.width, height: r.height });
      if (r.width > 50 && r.height > 20) setSizeReady(true);
    });
    ro.observe(containerRef.current);

    // 直接给 body 加 class（同步生效，不依赖 React 异步 setState）
    // CSS: body.dragging .container-terminal > * { pointer-events: none }
    const onStart = () => { document.body.classList.add("dragging"); setDragging(true); };
    const onEnd = () => { document.body.classList.remove("dragging"); setDragging(false); setHoverDrop(null); };
    document.addEventListener("dragstart", onStart, true);
    document.addEventListener("dragend", onEnd, true);
    document.addEventListener("drop", onEnd, true);

    return () => {
      ro.disconnect();
      document.removeEventListener("dragstart", onStart, true);
      document.removeEventListener("dragend", onEnd, true);
      document.removeEventListener("drop", onEnd, true);
      document.body.classList.remove("dragging");
    };
  }, []);

  const layout = layoutTree(workspace.root, { x: 0, y: 0, width: size.width, height: size.height });
  // maximize 生效条件：id 存在且 layout 里能找到对应 rect（守卫已确保不会有悬空 id，但渲染时再校验一次）
  const maxId = workspace.maximizedContainerId && layout.containers[workspace.maximizedContainerId]
    ? workspace.maximizedContainerId
    : null;

  // workspace layout 变化时 fit 所有终端（用 rAF + 二次 rAF 确保 reflow 完成）
  const fitTimerRef = useRef<number | null>(null);
  useEffect(() => {
    if (fitTimerRef.current) cancelAnimationFrame(fitTimerRef.current);
    fitTimerRef.current = requestAnimationFrame(() => {
      // 第二帧确保 CSS reflow 完成
      fitTimerRef.current = requestAnimationFrame(() => {
        // 拖分割线时布局每帧都在变：列数重排延后到松手（flushResize），否则每帧重排全部 pane 的回滚（#203）
        terminalManager.fitAll(!document.body.classList.contains("pane-resizing"));
      });
    });
    return () => { if (fitTimerRef.current) cancelAnimationFrame(fitTimerRef.current); };
  }, [workspace.root, workspace.maximizedContainerId]);

  function handleTabDragStart(containerId: string, tabId: string, e: React.DragEvent) {
    e.dataTransfer.setData(CONTAINER_TAB_MIME, JSON.stringify({ containerId, tabId }));
    e.dataTransfer.effectAllowed = "move";
  }

  function handleTabBarDrop(containerId: string, e: React.DragEvent, idx?: number) {
    e.preventDefault();
    const raw = e.dataTransfer.getData(CONTAINER_TAB_MIME);
    if (!raw) return;
    try {
      const { containerId: srcId, tabId } = JSON.parse(raw);
      if (srcId === containerId) {
        // 同 container 内重排
        if (idx != null && onTabReorder) onTabReorder(containerId, tabId, idx);
        return;
      }
      // 跨 container 移动到指定位置
      onMoveTab(srcId, tabId, containerId, idx);
    } catch {}
  }

  function handleTabBarDragOver(e: React.DragEvent) {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  }

  function handlePaneDragOver(containerId: string, e: React.DragEvent) {
    // preventDefault 无条件调用，**必须在守卫之前**：它的语义是「本元素接受这次 drop」。
    // 不调用的话 drop 事件根本不会派发到这里，而是走浏览器默认动作 —— 把拖进来的文件
    // 当页面导航过去，等于整个 app 被顶掉。所以外部拖拽照旧「接住并吞掉」。
    e.preventDefault();
    // 同样无条件：文件拖进来也是能落的（落下去会插路径），光标该显示 + 而不是禁止符。
    e.dataTransfer.dropEffect = "copy";
    // 但只有会分屏的拖拽才画落点浮层。文件拖进来时画一个「松手就分屏」的提示是句谎话
    // ——它插的是一段文本，不动布局——而且**外部拖拽不派发 dragend**，浮层清不掉会
    // 永久留在终端上（#175）。
    if (!acceptsPaneDrop(e.dataTransfer.types)) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const zone = computeDropZone(r, e.clientX, e.clientY);
    setHoverDrop({ containerId, zone });
  }

  function handlePaneDrop(containerId: string, e: React.DragEvent) {
    e.preventDefault();
    setHoverDrop(null);
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const zone = computeDropZone(r, e.clientX, e.clientY);

    // tab 跨 container 拖到边缘 → split
    const tabRaw = e.dataTransfer.getData(CONTAINER_TAB_MIME);
    if (tabRaw) {
      try {
        const { containerId: srcId, tabId } = JSON.parse(tabRaw);
        onSplitWithTab(srcId, tabId, containerId, zone.dir, zone.side);
        return;
      } catch {}
    }

    // session 卡片拖入
    const specRaw = e.dataTransfer.getData(PANE_SPEC_MIME);
    if (specRaw) {
      try {
        const spec = JSON.parse(specRaw);
        onSplitWithSession(containerId, zone.dir, zone.side, spec);
      } catch {}
    }

    // 从 Finder 拖文件进来 → 在当前 tab 的命令行里插入路径。放在最后，我们自己的两种
    // 拖拽优先；`zone` 对它没有意义（不分屏，就插一段文本，跟 Terminal.app 一样）。
    if (!tabRaw && !specRaw) {
      const paths = parseDroppedPaths(e.dataTransfer);
      if (paths.length === 0) return;
      const tabId = containerById.get(containerId)?.activeTabId;
      if (!tabId) return;
      if (terminalManager.writeText(tabId, formatPathsForTerminal(paths))) {
        // 焦点跟过去：插完就是要接着打字，否则用户得再点一下终端
        onSetActive(containerId);
        terminalManager.focus(tabId);
      }
    }
  }

  // 索引化：containerId → ContainerNode（避免每次渲染遍历整树）
  const containerById = useMemo(() => {
    const map = new Map<string, import("./workspace-types").ContainerNode>();
    for (const c of collectContainersFromLayout(workspace.root)) map.set(c.id, c);
    return map;
  }, [workspace.root]);

  return (
    <div ref={containerRef} className="workspace-view">{sizeReady && Object.entries(layout.containers).map(([containerId, rect]) => {
        const container = containerById.get(containerId);
        if (!container) return null;
        const hover = hoverDrop && hoverDrop.containerId === containerId ? hoverDrop.zone : null;
        // maximize 时：被最大化容器铺满 workspace；其它容器 display:none 但 DOM 留住（PTY/xterm 不重建）
        const isMaximized = maxId === containerId;
        const hidden = maxId !== null && !isMaximized;
        const renderRect = isMaximized
          ? { x: 0, y: 0, width: size.width, height: size.height }
          : rect;
        return (
          <div
            key={containerId}
            data-container-id={containerId}
            className="workspace-container-wrapper"
            style={{
              position: "absolute",
              left: renderRect.x,
              top: renderRect.y,
              width: renderRect.width,
              height: renderRect.height,
              display: hidden ? "none" : undefined,
            }}
          >
            <ContainerView
              container={container}
              isActive={containerId === workspace.activeContainerId}
              dragging={dragging}
              isMaximized={isMaximized}
              onToggleMaximize={onToggleMaximize ? () => onToggleMaximize(containerId) : undefined}
              getTabTitle={getTabTitle}
              getTabStatus={getTabStatus}
              onActivate={() => onSetActive(containerId)}
              onTabClick={(tabId) => onTabClick(containerId, tabId)}
              onTabReveal={(tabId) => onTabReveal?.(containerId, tabId)}
              onTabClose={(tabId) => onTabClose(containerId, tabId)}
              onSplit={(dir) => onSplit(containerId, dir)}
              onNewShell={() => onNewShell(containerId)}
              onTabDragStart={(e, tabId) => handleTabDragStart(containerId, tabId, e)}
              onTabDrop={(e, idx) => handleTabBarDrop(containerId, e, idx)}
              onTabDragOver={handleTabBarDragOver}
              onPaneDragOver={(e) => handlePaneDragOver(containerId, e)}
              onPaneDrop={(e) => handlePaneDrop(containerId, e)}
              onContextMenu={(e) => {
                // 落在终端里的右键归终端菜单（App 里 window 上的 contextmenu
                // handler 按 data-pane-id 认领）。这里不能抢：container-view 是
                // 终端的祖先，不判断的话两个 handler 会先后各设一次菜单 state，
                // 结果取决于谁最后跑 —— 那种"能用但说不清为什么"的实现下次一定坏。
                // 也别在这儿 preventDefault：那个 handler 自己会做。
                if ((e.target as HTMLElement | null)?.closest?.("[data-pane-id]")) return;
                e.preventDefault();
                onContextMenu(containerId, e.clientX, e.clientY);
              }}
              onTabContextMenu={onTabContextMenu ? (e, tabId) => onTabContextMenu(containerId, tabId, e.clientX, e.clientY) : undefined}
            />
            {hover && <DropOverlay zone={hover} />}
          </div>
        );
      })}
      {sizeReady && maxId === null && layout.splits.map((s) => (
        <SplitResizer
          key={s.id}
          info={s}
          onResize={(newRatio) => {
            const newRoot = setSplitRatio(workspace.root, s.id, newRatio);
            onUpdateRoot(newRoot);
          }}
        />
      ))}
    </div>
  );
}

// --- Helper: collect containers from LayoutNode ---
function collectContainersFromLayout(node: LayoutNode): import("./workspace-types").ContainerNode[] {
  if (node.kind === "container") return [node];
  return [...collectContainersFromLayout(node.a), ...collectContainersFromLayout(node.b)];
}

// --- DropOverlay ---
function DropOverlay({ zone }: { zone: DropZone }) {
  const style: React.CSSProperties = { position: "absolute", pointerEvents: "none" };
  if (zone.dir === "v") {
    style.top = 0; style.bottom = 0;
    if (zone.side === "before") { style.left = 0; style.width = "50%"; }
    else { style.right = 0; style.width = "50%"; }
  } else {
    style.left = 0; style.right = 0;
    if (zone.side === "before") { style.top = 0; style.height = "50%"; }
    else { style.bottom = 0; style.height = "50%"; }
  }
  return <div className="pane-drop-overlay" style={style} />;
}

// --- SplitResizer ---
type SplitInfo = { id: string; dir: "h" | "v"; rect: Rect; outerRect: Rect; node: SplitNode };

function SplitResizer({ info, onResize }: { info: SplitInfo; onResize: (ratio: number) => void }) {
  const divRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);
  const startXRef = useRef(0);
  const startYRef = useRef(0);
  const startRatioRef = useRef(0);
  const totalRef = useRef(0);

  useEffect(() => {
    const div = divRef.current;
    if (!div) return;

    const isV = info.dir === "v";

    const onPointerDown = (e: PointerEvent) => {
      e.preventDefault();
      div.setPointerCapture(e.pointerId);
      draggingRef.current = true;
      startXRef.current = e.clientX;
      startYRef.current = e.clientY;
      startRatioRef.current = info.node.ratio;
      totalRef.current = isV ? info.outerRect.width - 6 : info.outerRect.height - 6;
      document.body.style.cursor = isV ? "ew-resize" : "ns-resize";
      document.body.style.userSelect = "none";
      document.body.classList.add("pane-resizing");
      div.classList.add("is-dragging"); // 分隔线拖动时高亮（见 App.css .pane-resizer.is-dragging）
    };

    const onPointerMove = (e: PointerEvent) => {
      if (!draggingRef.current) return;
      const delta = isV ? e.clientX - startXRef.current : e.clientY - startYRef.current;
      const rawRatio = startRatioRef.current + delta / totalRef.current;
      const newRatio = Math.max(0.1, Math.min(0.9, rawRatio));
      onResize(newRatio);
    };

    const onPointerUp = (e: PointerEvent) => {
      if (!draggingRef.current) return;
      draggingRef.current = false;
      if (div.hasPointerCapture(e.pointerId)) {
        div.releasePointerCapture(e.pointerId);
      }
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.body.classList.remove("pane-resizing");
      terminalManager.flushResize(); // 松手：延后的列数重排立即做掉（#203）
      div.classList.remove("is-dragging");
    };

    const onPointerCancel = () => {
      draggingRef.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      document.body.classList.remove("pane-resizing");
      terminalManager.flushResize(); // 松手：延后的列数重排立即做掉（#203）
      div.classList.remove("is-dragging");
    };

    div.addEventListener("pointerdown", onPointerDown);
    div.addEventListener("pointermove", onPointerMove);
    div.addEventListener("pointerup", onPointerUp);
    div.addEventListener("pointercancel", onPointerCancel);

    return () => {
      div.removeEventListener("pointerdown", onPointerDown);
      div.removeEventListener("pointermove", onPointerMove);
      div.removeEventListener("pointerup", onPointerUp);
      div.removeEventListener("pointercancel", onPointerCancel);
    };
  }, [info.node.ratio, info.dir, info.outerRect, onResize]);

  // RESIZER_PX=0 → info.rect 在分割方向上厚度为 0。给 resizer 一个真实的 10px 抓取盒
  // 横跨边界（不再依赖 0 宽父盒 + ::after 溢出，WKWebView 下溢出伪元素命中不可靠）。
  const HIT = 10;
  const isV = info.dir === "v";
  const boxStyle: React.CSSProperties = isV
    ? { left: info.rect.x - HIT / 2, top: info.rect.y, width: HIT, height: info.rect.height }
    : { left: info.rect.x, top: info.rect.y - HIT / 2, width: info.rect.width, height: HIT };

  return (
    <div
      ref={divRef}
      className={"pane-resizer pane-resizer-" + info.dir}
      style={{
        position: "absolute",
        ...boxStyle,
        touchAction: "none", // 防止触摸滚动干扰
      }}
    />
  );
}
