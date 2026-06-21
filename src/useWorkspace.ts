import { useState, useEffect, useRef } from "react";
import { terminalManager } from "./TerminalManager";
import {
  WorkspaceState, LayoutNode, ContainerNode, PaneTab,
  makeContainerId, makeTabId, findContainer, collectContainers,
  splitContainer, addTabToContainer, closeTab, moveTab, removeContainer,
  updateContainer, isWorkspaceState, migrateWorkspace,
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
    const raw = localStorage.getItem("ccs-workspace");
    if (raw) {
      const parsed = JSON.parse(raw);
      if (isWorkspaceState(parsed)) return parsed;
    }
    // 尝试从旧 openTabs 迁移
    const oldRaw = localStorage.getItem("ccs-open-tabs");
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
  const [workspace, setWorkspace] = useState<WorkspaceState>(loadWorkspace);
  const activationStackRef = useRef<string[]>([]);

  // 持久化 debounce 500ms（避免切 tab 等高频操作时同步 stringify 阻塞主线程）
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(() => {
      try { localStorage.setItem("ccs-workspace", JSON.stringify(workspace)); } catch {}
    }, 500);
  }, [workspace]);

  // maximize 失效守卫：被最大化的 container 若已不存在（关闭/拖走/合并），清掉
  useEffect(() => {
    if (!workspace.maximizedContainerId) return;
    if (!findContainer(workspace.root, workspace.maximizedContainerId)) {
      setWorkspace((ws) => ({ ...ws, maximizedContainerId: null }));
    }
  }, [workspace.root, workspace.maximizedContainerId]);

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
      return {
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
      return {
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
    setWorkspace((ws) => {
      const src = findContainer(ws.root, srcContainerId);
      if (!src) return ws;
      const tab = src.tabs.find((t) => t.id === tabId);
      if (!tab) return ws;
      // 从源 container 摘除 tab
      const srcRemaining = src.tabs.filter((t) => t.id !== tabId);
      let root = ws.root;
      if (srcRemaining.length === 0) {
        terminalManager.destroy(tabId);
        root = removeContainer(root, srcContainerId) ?? root;
      } else {
        const nextActive = src.activeTabId === tabId ? srcRemaining[0].id : src.activeTabId;
        root = updateContainer(root, srcContainerId, () => ({ ...src, tabs: srcRemaining, activeTabId: nextActive }));
      }
      // 在目标 container 旁边创建新 containerconst newId = makeContainerId();
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
    const resumeCmd = tool === "codex"
      ? `clear && codex resume ${sessionId}`
      : `clear && claude -r ${sessionId}`;
    const tab: PaneTab = { id: makeTabId(), kind: "resume", cwd, initCommand: resumeCmd, sessionId, sessionShortId: shortId, label: label || `[${shortId}]` };
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
  };

  function bindSessionToTab(containerId: string, tabId: string, sessionId: string, shortId: string) {
    setWorkspace((ws) => ({
      ...ws,
      root: updateContainer(ws.root, containerId, (c) => ({
        ...c,
        tabs: c.tabs.map((t) =>
          t.id === tabId
            ? { ...t, kind: "resume" as const, sessionId, sessionShortId: shortId, label: `[${shortId}]` }
            : t
        ),
      })),
    }));
  }
}
