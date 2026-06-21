import { useEffect, useRef, useState, startTransition } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type NotificationRecord = {
  id: string;
  sessionId: string;
  sessionName: string;
  projectLabel: string;
  timestamp: number;
  isRead: boolean;
  kind: "waiting";
  source?: "poll" | "hook";
  waitingFor?: string;
};

type SessionLike = {
  session_id: string;
  display_name: string;
  status: string;
  waiting_for: string;
  git_root: string;
  cwd: string;
};

const MAX_RECORDS = 100;
const STORAGE_KEY = "ccs-notifications";
const SYSTEM_KEY = "ccs-notif-system";
const NOTIFY_APPROVAL_KEY = "ccs-notif-approval"; // 等待审批通知开关（默认开）
const NOTIFY_USER_KEY = "ccs-notif-user";          // 等待回答通知开关（默认关）

function load(): NotificationRecord[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.slice(0, MAX_RECORDS) : [];
  } catch {
    return [];
  }
}

function persist(list: NotificationRecord[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(list.slice(0, MAX_RECORDS)));
  } catch {}
}

function projectLabel(git_root: string, cwd: string): string {
  const base = git_root || cwd || "";
  return base.split("/").filter(Boolean).pop() ?? base;
}

export function useNotifications(
  sessions: SessionLike[],
  activeSessionId: string | null,
  onFocusNotify?: () => void,
) {
  const [notifications, setNotifications] = useState<NotificationRecord[]>(load);
  // 只用未读通知初始化，已读的 session 允许重新触发通知（重启去重）
  const notifiedRef = useRef(new Set<string>(load().filter((n) => !n.isRead).map((n) => n.sessionId)));
  // 第一次 poll 时只入中心不弹 macOS 通知（避免重启 flood）
  const isFirstPollRef = useRef(true);
  const activeSessionIdRef = useRef(activeSessionId);
  activeSessionIdRef.current = activeSessionId;
  const onFocusNotifyRef = useRef(onFocusNotify);
  onFocusNotifyRef.current = onFocusNotify;
  const [systemEnabled, setSystemEnabledState] = useState(
    () => localStorage.getItem(SYSTEM_KEY) !== "false"
  );
  const systemEnabledRef = useRef(systemEnabled);
  const [notifyApproval, setNotifyApprovalState] = useState(
    () => localStorage.getItem(NOTIFY_APPROVAL_KEY) !== "false"
  );
  const notifyApprovalRef = useRef(notifyApproval);
  const [notifyUser, setNotifyUserState] = useState(
    () => localStorage.getItem(NOTIFY_USER_KEY) === "true"
  );
  const notifyUserRef = useRef(notifyUser);
  // keep a ref to latest sessions for use inside the hook listener closure
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  // poll-based detection: fires when sessions array changes
  useEffect(() => {
    const isFirstPoll = isFirstPollRef.current;
    isFirstPollRef.current = false;

    const fresh: NotificationRecord[] = [];

    for (const s of sessions) {
      if (s.status === "waiting") {
        const isUserWait = s.waiting_for === "user";
        const shouldNotify = isUserWait ? notifyUserRef.current : notifyApprovalRef.current;
        if (shouldNotify && !notifiedRef.current.has(s.session_id)) {
          notifiedRef.current.add(s.session_id);
          const isActiveSession = activeSessionIdRef.current === s.session_id;

          if (isActiveSession) {
            // 对标产品 isFocusedPanel：active session 永不进通知中心
            if (!isFirstPoll) {
              if (document.hasFocus()) {
                onFocusNotifyRef.current?.();
              } else if (systemEnabledRef.current) {
                const label = projectLabel(s.git_root, s.cwd);
                const waitLabel = isUserWait ? "等待回答" : "等待审批";
                const name = s.display_name || s.session_id.slice(0, 8);
                invoke("show_notification", {
                  title: `Claude ${waitLabel}`,
                  body: name + (label ? ` — ${label}` : ""),
                }).catch(() => {});
              }
            }
          } else {
            const label = projectLabel(s.git_root, s.cwd);
            const waitLabel = isUserWait ? "等待回答" : "等待审批";
            const rec: NotificationRecord = {
              id: `${s.session_id}-${Date.now()}`,
              sessionId: s.session_id,
              sessionName: s.display_name || s.session_id.slice(0, 8),
              projectLabel: label,
              timestamp: Date.now(),
              isRead: false,
              kind: "waiting",
              source: "poll",
              waitingFor: s.waiting_for || undefined,
            };
            fresh.push(rec);
            if (!isFirstPoll && systemEnabledRef.current) {
              invoke("show_notification", {
                title: `Claude ${waitLabel}`,
                body: rec.sessionName + (label ? ` — ${label}` : ""),
              }).catch(() => {});
            }
          }
        } else if (!shouldNotify) {
          notifiedRef.current.delete(s.session_id);
        }
      } else {
        notifiedRef.current.delete(s.session_id);
      }
    }

    if (fresh.length > 0) {
      setNotifications((prev) => {
        // 去重：若中心已有该 session 的未读通知，不重复添加
        const toAdd = fresh.filter((r) => !prev.some((n) => !n.isRead && n.sessionId === r.sessionId));
        if (toAdd.length === 0) return prev;
        const next = [...toAdd, ...prev].slice(0, MAX_RECORDS);
        persist(next);
        return next;
      });
    }
  }, [sessions]);

  // hook-based detection: push from Rust via Unix socket
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<string>("claude-hook", (event) => {
      try {
        const payload = JSON.parse(event.payload);
        // only handle Notification hook events
        if (payload.hook_event_name !== "Notification") return;
        const sessionId: string = payload.session_id ?? "";
        if (!sessionId) return;
        const isUserWait = payload.waiting_for === "user";
        const shouldNotify = isUserWait ? notifyUserRef.current : notifyApprovalRef.current;
        if (!shouldNotify) return;

        // dedup with poll-based detector using the same ref
        if (notifiedRef.current.has(sessionId)) return;
        notifiedRef.current.add(sessionId);

        const isActiveSession = activeSessionIdRef.current === sessionId;

        if (isActiveSession) {
          // 对标产品 isFocusedPanel：active session 永不进通知中心
          if (document.hasFocus()) {
            onFocusNotifyRef.current?.();
          } else if (systemEnabledRef.current) {
            const cur2 = sessionsRef.current;
            const s2 = cur2.find((x) => x.session_id === sessionId);
            const label2 = s2 ? projectLabel(s2.git_root, s2.cwd) : "";
            const name2 = s2?.display_name || sessionId.slice(0, 8);
            const isUserWait2 = payload.waiting_for === "user";
            invoke("show_notification", {
              title: `Claude ${isUserWait2 ? "等待回答" : "等待审批"}`,
              body: name2 + (label2 ? ` — ${label2}` : ""),
            }).catch(() => {});
          }
          return;
        }

        const cur = sessionsRef.current;
        const session = cur.find((s) => s.session_id === sessionId);
        const label = session ? projectLabel(session.git_root, session.cwd) : "";
        const sessionName = session?.display_name || sessionId.slice(0, 8);
        const waitLabel = isUserWait ? "等待回答" : "等待审批";

        const rec: NotificationRecord = {
          id: `${sessionId}-hook-${Date.now()}`,
          sessionId,
          sessionName,
          projectLabel: label,
          timestamp: Date.now(),
          isRead: false,
          kind: "waiting",
          source: "hook",
          waitingFor: payload.waiting_for || undefined,
        };

        startTransition(() => {
          setNotifications((prev) => {
            // 去重：已有未读通知的 session 不重复添加
            if (prev.some((n) => !n.isRead && n.sessionId === sessionId)) return prev;
            const next = [rec, ...prev].slice(0, MAX_RECORDS);
            persist(next);
            return next;
          });
        });

        if (systemEnabledRef.current) {
          invoke("show_notification", {
            title: `Claude ${waitLabel}`,
            body: sessionName + (label ? ` — ${label}` : ""),
          }).catch(() => {});
        }
      } catch {}
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, []); // register once

  function markRead(id: string) {
    setNotifications((prev) => {
      const next = prev.map((n) => (n.id === id ? { ...n, isRead: true } : n));
      persist(next);
      return next;
    });
  }

  function markReadBySession(sessionId: string) {
    setNotifications((prev) => {
      if (!prev.some((n) => !n.isRead && n.sessionId === sessionId)) return prev;
      const next = prev.map((n) => (n.sessionId === sessionId ? { ...n, isRead: true } : n));
      persist(next);
      return next;
    });
  }

  function markAllRead() {
    setNotifications((prev) => {
      const next = prev.map((n) => ({ ...n, isRead: true }));
      persist(next);
      return next;
    });
  }

  function clearOne(id: string) {
    setNotifications((prev) => {
      const next = prev.filter((n) => n.id !== id);
      persist(next);
      return next;
    });
  }

  function clearAll() {
    setNotifications([]);
    persist([]);
  }

  function setSystemEnabled(v: boolean) {
    systemEnabledRef.current = v;
    setSystemEnabledState(v);
    localStorage.setItem(SYSTEM_KEY, String(v));
  }

  function setNotifyApproval(v: boolean) {
    notifyApprovalRef.current = v;
    setNotifyApprovalState(v);
    localStorage.setItem(NOTIFY_APPROVAL_KEY, String(v));
  }

  function setNotifyUser(v: boolean) {
    notifyUserRef.current = v;
    setNotifyUserState(v);
    localStorage.setItem(NOTIFY_USER_KEY, String(v));
  }

  const unreadCount = notifications.filter((n) => !n.isRead).length;

  return {
    notifications,
    unreadCount,
    markRead,
    markReadBySession,
    markAllRead,
    clearOne,
    clearAll,
    systemEnabled,
    setSystemEnabled,
    notifyApproval,
    setNotifyApproval,
    notifyUser,
    setNotifyUser,
  };
}
