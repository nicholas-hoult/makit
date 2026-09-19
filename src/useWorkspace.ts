import { useState, useEffect, useRef, Dispatch, SetStateAction } from "react";
import { terminalManager } from "./TerminalManager";
import {
  WorkspaceState, LayoutNode, ContainerNode, PaneTab,
  makeContainerId, makeTabId, findContainer, collectContainers,
  splitContainer, addTabToContainer, closeTab, moveTab, removeContainer,
  updateContainer, isWorkspaceState, migrateWorkspace, normalizeWorkspace,
  resumeInitCommand, bindSessionToPaneTab, repairResumeTabs,
} from "./workspace-types";

function makeDefaultWorkspace(): WorkspaceState {
  const id = makeContainerId();
  return {
    root: { kind: "container", id, tabs: [], activeTabId: "", tabHistory: [] },
    activeContainerId: id,
  };
}

function loadWorkspace(): WorkspaceState {
  try {
    const raw = localStorage.getItem("makit-workspace");
    if (raw) {
      const parsed = JSON.parse(raw);
      if (isWorkspaceState(parsed)) return parsed;
    }
    // 尝试从旧 openTabs 迁移
    const oldRaw = localStorage.getItem("makit-open-tabs");
    if (oldRaw) {
      const oldTabs = JSON.parse(oldRaw);
      if (Array.isArray(oldTabs) && oldTabs.length > 0) {
        return migrateWorkspace(oldTabs);
      }
    }
  } catch {}
  return makeDefaultWorkspace();
}

export function useWorkspace() {
  // 初始值也要过 normalize：localStorage 里存着的就可能是不一致的状态
  // （上面那 4 条路径已经写进去过），否则一启动就是「焦点在看不见的 pane」
  // repairResumeTabs 在 normalize 之前：它修的是**存档里已有的**坏 tab
  // （kind=resume 但 initCommand=null，见 workspace-types），只在这一次读取时跑
  const [workspace, setWorkspaceRaw] = useState<WorkspaceState>(
    () => normalizeWorkspace(repairResumeTabs(loadWorkspace()))
  );
  const setWorkspace: Dispatch<SetStateAction<WorkspaceState>> = (update) => {
    setWorkspaceRaw((prev) =>
      normalizeWorkspace(typeof update === "function" ? (update as (p: WorkspaceState) => WorkspaceState)(prev) : update)
    );
  };
  const activationStackRef = useRef<string[]>([]);

  // 持久化 debounce 500ms（避免切 tab 等高频操作时同步 stringify 阻塞主线程）
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(() => {
      try { localStorage.setItem("makit-workspace", JSON.stringify(workspace)); } catch {}
    }, 500);
  }, [workspace]);

  // 原来这里有一个「maximize 失效守卫」effect，逻辑和 normalizeWorkspace 的 ①
  // 完全一样，已并进去：normalize 在 set 的那一刻就生效，不用多等一次 re-render
  // 渲染出一帧不一致的画面。（顺带说明它为什么从没生效过：handleTabClose 直接把
  // maximizedContainerId 丢成了 undefined，守卫看到的永远是「本来就没放大」。）

  function toggleMaximize(containerId: string) {
    setWorkspace((ws) => {
      if (!findContainer(ws.root, containerId)) return ws;
      const next = ws.maximizedContainerId === containerId ? null : containerId;
      return { ...ws, maximizedContainerId: next, activeContainerId: containerId };
    });
  }

  // 维护激活栈：每次 active container 变化时推入栈顶
  useEffect(() => {
    if (!workspace.activeContainerId) return;
    const stack = activationStackRef.current;
    activationStackRef.current = [...stack.filter((x) => x !== workspace.activeContainerId), workspace.activeContainerId];
  }, [workspace.activeContainerId]);

  function pickNextContainer(containers: ContainerNode[], closedId: string): string {
    const stack = activationStackRef.current;
    for (let i = stack.length - 1; i >= 0; i--) {
      if (stack[i] !== closedId && containers.some((c) => c.id === stack[i])) {
        return stack[i];
      }
    }
    return containers[0]?.id ?? "";
  }

  function getActiveContainer(): ContainerNode | null {
    return findContainer(workspace.root, workspace.activeContainerId);
  }

  function setActive(containerId: string) {
    setWorkspace((ws) => ({ ...ws, activeContainerId: containerId }));
  }

  function handleTabClick(containerId: string, tabId: string) {
    setWorkspace((ws) => ({
      ...ws,
      activeContainerId: containerId,
      root: updateContainer(ws.root, containerId, (c) => {
        const history = (c.tabHistory || []).filter((id) => id !== tabId);
        history.push(c.activeTabId);
        return { ...c, activeTabId: tabId, tabHistory: history };
      }),
    }));
  }

  function handleTabClose(containerId: string, tabId: string) {
    terminalManager.destroy(tabId);
    setWorkspace((ws) => {
      const result = closeTab(ws.root, containerId, tabId);
      if (!result.root) return makeDefaultWorkspace();
      const containers = collectContainers(result.root);
      const activeStillExists = containers.some((c) => c.id === ws.activeContainerId);
      // `...ws` 不能省：漏了它 maximizedContainerId 会被丢成 undefined，
      // 于是「关掉放大 pane 里的某个 tab」会顺带退出放大态。normalize 救不回来
      // —— 信息已经没了，它只会认为本来就没放大。
      return {
        ...ws,
        root: result.root,
        activeContainerId: activeStillExists ? ws.activeContainerId : pickNextContainer(containers, containerId),
      };
    });
  }

  function handleSplit(containerId: string, dir: "h" | "v") {
    const newId = makeContainerId();
    const container = findContainer(workspace.root, containerId);
    const cwd = container?.tabs.find((t) => t.id === container.activeTabId)?.cwd ?? "~";
    const tabId = makeTabId();
    const newContainer: ContainerNode = {
      kind: "container",
      id: newId,
      tabs: [{ id: tabId, kind: "shell", cwd, initCommand: null, sessionId: null, sessionShortId: null, label: "" }],
      activeTabId: tabId,
      tabHistory: [],
    };
    setWorkspace((ws) => ({
      ...ws,
      root: splitContainer(ws.root, containerId, dir, newContainer),
      activeContainerId: newId,
      maximizedContainerId: null,
    }));
  }

  function handleNewShell(containerId: string) {
    const container = findContainer(workspace.root, containerId);
    const cwd = container?.tabs.find((t) => t.id === container.activeTabId)?.cwd ?? "~";
    const tab: PaneTab = { id: makeTabId(), kind: "shell", cwd, initCommand: null, sessionId: null, sessionShortId: null, label: "" };
    setWorkspace((ws) => ({
      ...ws,
      activeContainerId: containerId,
      root: addTabToContainer(ws.root, containerId, tab),
    }));
  }

  function handleMoveTab(srcContainerId: string, tabId: string, destContainerId: string, targetIdx?: number) {
    setWorkspace((ws) => {
      const src = findContainer(ws.root, srcContainerId);
      const dest = findContainer(ws.root, destContainerId);
      if (!src || !dest) return ws;
      const tab = src.tabs.find((t) => t.id === tabId);
      if (!tab) return ws;
      // 从源摘除
      const srcRemaining = src.tabs.filter((t) => t.id !== tabId);
      let root = ws.root;
      if (srcRemaining.length === 0) {
        root = removeContainer(root, srcContainerId) ?? root;
      } else {
        const nextActive = src.activeTabId === tabId ? srcRemaining[0].id : src.activeTabId;
        root = updateContainer(root, srcContainerId, () => ({ ...src, tabs: srcRemaining, activeTabId: nextActive }));
      }
      // 插入目标 container 指定位置
      const idx = targetIdx == null ? dest.tabs.length : Math.max(0, Math.min(dest.tabs.length, targetIdx));
      const newTabs = [...dest.tabs.slice(0, idx), tab, ...dest.tabs.slice(idx)];
      root = updateContainer(root, destContainerId, () => ({ ...dest, tabs: newTabs, activeTabId: tab.id }));
      const containers = collectContainers(root);
      // `...ws` 同 handleTabClose：漏了会让「把 tab 拖到别的 pane」顺带退出放大态
      return {
        ...ws,
        root,
        activeContainerId: containers.some((c) => c.id === destContainerId) ? destContainerId : (containers[0]?.id ?? ""),
      };
    });
  }

  // 同 container 内重排 tab
  function handleTabReorder(containerId: string, tabId: string, targetIdx: number) {
    setWorkspace((ws) => {
      const c = findContainer(ws.root, containerId);
      if (!c) return ws;
      const curIdx = c.tabs.findIndex((t) => t.id === tabId);
      if (curIdx < 0) return ws;
      // 调整索引：从源位置之后插入时减 1（数组已抽出源）
      let insertIdx = targetIdx;
      if (insertIdx > curIdx) insertIdx--;
      if (insertIdx === curIdx) return ws;
      const newTabs = c.tabs.slice();
      const [tab] = newTabs.splice(curIdx, 1);
      newTabs.splice(insertIdx, 0, tab);
      return { ...ws, root: updateContainer(ws.root, containerId, () => ({ ...c, tabs: newTabs })) };
    });
  }

  function handleSplitWithTab(srcContainerId: string, tabId: string, targetContainerId: string, dir: "h" | "v", side: "before" | "after") {
    const newId = makeContainerId();
    setWorkspace((ws) => {
      const src = findContainer(ws.root, srcContainerId);
      if (!src) return ws;
      const tab = src.tabs.find((t) => t.id === tabId);
      if (!tab) return ws;
      // 从源 container 摘除 tab（tab 被移动不是关闭，不 destroy 终端）
      const srcRemaining = src.tabs.filter((t) => t.id !== tabId);
      let root = ws.root;
      if (srcRemaining.length === 0) {
        root = removeContainer(root, srcContainerId) ?? root;
      } else {
        const nextActive = src.activeTabId === tabId ? srcRemaining[0].id : src.activeTabId;
        root = updateContainer(root, srcContainerId, () => ({ ...src, tabs: srcRemaining, activeTabId: nextActive }));
      }
      // 在目标 container 旁边创建新 container
      const newContainer: ContainerNode = { kind: "container", id: newId, tabs: [tab], activeTabId: tab.id, tabHistory: [] };
      // 把目标 container 替换为 split(target, new) 或 split(new, target)
      root = updateContainer(root, targetContainerId, (target) => ({
        kind: "split" as const,
        dir,
        ratio: 0.5,
        a: side === "before" ? newContainer : target,
        b: side === "before" ? target : newContainer,
      }));
      return { root, activeContainerId: newId, maximizedContainerId: null };
    });
  }

  function handleSplitWithSession(containerId: string, dir: "h" | "v", side: "before" | "after", spec: { kind: "resume" | "new" | "shell"; cwd: string; initCommand: string | null; sessionId: string | null; sessionShortId: string | null }) {
    const newContId = makeContainerId();
    const tabId = makeTabId();
    const tab: PaneTab = { id: tabId, kind: spec.kind, cwd: spec.cwd, initCommand: spec.initCommand, sessionId: spec.sessionId, sessionShortId: spec.sessionShortId, label: spec.sessionShortId ? `[${spec.sessionShortId}]` : "" };
    const newContainer: ContainerNode = { kind: "container", id: newContId, tabs: [tab], activeTabId: tabId, tabHistory: [] };
    setWorkspace((ws) => ({
      ...ws,
      root: updateContainer(ws.root, containerId, (target) => ({
        kind: "split" as const,
        dir,
        ratio: 0.5,
        a: side === "before" ? newContainer : target,
        b: side === "before" ? target : newContainer,
      })),
      activeContainerId: newContId,
      maximizedContainerId: null,
    }));
  }

  function handleDropSession(containerId: string, spec: { kind: "resume" | "new" | "shell"; cwd: string; initCommand: string | null; sessionId: string | null; sessionShortId: string | null }) {
    const tab: PaneTab = { id: makeTabId(), kind: spec.kind, cwd: spec.cwd, initCommand: spec.initCommand, sessionId: spec.sessionId, sessionShortId: spec.sessionShortId, label: spec.sessionShortId ? `[${spec.sessionShortId}]` : "" };
    setWorkspace((ws) => ({
      ...ws,
      activeContainerId: containerId,
      root: addTabToContainer(ws.root, containerId, tab),
    }));
  }

  function handleUpdateRoot(newRoot: LayoutNode) {
    setWorkspace((ws) => ({ ...ws, root: newRoot }));
  }

  // 打开 session（resume）：在 active container 加 tab
  function openSession(sessionId: string, shortId: string, cwd: string, label?: string, tool?: string) {
    // 检查是否已打开
    const containers = collectContainers(workspace.root);
    for (const c of containers) {
      const existing = c.tabs.find((t) => t.sessionId === sessionId);
      if (existing) {
        setWorkspace((ws) => ({ ...ws, activeContainerId: c.id, root: updateContainer(ws.root, c.id, (cc) => ({ ...cc, activeTabId: existing.id })) }));
        return;
      }
    }
    const tab: PaneTab = { id: makeTabId(), kind: "resume", cwd, initCommand: resumeInitCommand(sessionId, tool), sessionId, sessionShortId: shortId, label: label || `[${shortId}]` };
    setWorkspace((ws) => ({
      ...ws,
      root: addTabToContainer(ws.root, ws.activeContainerId, tab),
    }));
  }

  // 打开新 AI 会话（默认 claude，Codex 项目传 tool="codex"）
  function openNewSession(cwd: string, tool?: string) {
    const initCommand = tool === "codex" ? "codex" : "claude";
    const tab: PaneTab = { id: makeTabId(), kind: "new", cwd, initCommand, sessionId: null, sessionShortId: null, label: "新会话" };
    setWorkspace((ws) => ({
      ...ws,
      root: addTabToContainer(ws.root, ws.activeContainerId, tab),
    }));
  }

  // 打开 shell
  function openShell(cwd: string) {
    const tab: PaneTab = { id: makeTabId(), kind: "shell", cwd, initCommand: null, sessionId: null, sessionShortId: null, label: "" };
    setWorkspace((ws) => ({
      ...ws,
      root: addTabToContainer(ws.root, ws.activeContainerId, tab),
    }));
  }

  return {
    workspace,
    setWorkspace,
    getActiveContainer,
    setActive,
    handleTabClick,
    handleTabClose,
    handleSplit,
    handleNewShell,
    handleMoveTab,
    handleTabReorder,
    handleSplitWithTab,
    handleSplitWithSession,
    handleDropSession,
    handleUpdateRoot,
    toggleMaximize,
    openSession,
    openNewSession,
    openShell,
    bindSessionToTab,
    updateTabCwd,
  };

  /// 改掉某个 tab 记着的启动目录。
  ///
  /// 恢复被删目录之后**必须**做这一步：workspace 是持久化的，tab 里那个已经不存在的 cwd
  /// 会一直躺在 localStorage 里，下次启动照样按它 spawn，照样坏。恢复只修当前这次是不够的。
  /// 按 tabId 找，不需要 containerId —— 调用方（PTY 回调）手里只有 paneId。
  function updateTabCwd(tabId: string, cwd: string) {
    setWorkspace((ws) => {
      let root = ws.root;
      for (const c of collectContainers(ws.root)) {
        if (!c.tabs.some((t) => t.id === tabId)) continue;
        root = updateContainer(root, c.id, (cc) => ({
          ...cc,
          tabs: cc.tabs.map((t) => (t.id === tabId ? { ...t, cwd } : t)),
        }));
        break;
      }
      return { ...ws, root };
    });
  }

  // tab 的形状变换在 workspace-types.bindSessionToPaneTab 里（那边有为什么
  // 必须连 initCommand 一起写的说明），这里只负责在树里找到它
  function bindSessionToTab(
    containerId: string,
    tabId: string,
    sessionId: string,
    shortId: string,
    label?: string,
    sessionCwd?: string,
  ) {
    setWorkspace((ws) => ({
      ...ws,
      root: updateContainer(ws.root, containerId, (c) => ({
        ...c,
        tabs: c.tabs.map((t) =>
          t.id === tabId ? bindSessionToPaneTab(t, sessionId, shortId, label, sessionCwd) : t
        ),
      })),
    }));
  }
}
