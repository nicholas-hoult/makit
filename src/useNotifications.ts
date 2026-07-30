import { useEffect, useRef, useState, startTransition } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

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

/**
 * 系统通知的 **OS 授权状态**，和应用内的 `systemEnabled` 开关是两件独立的事：
 * 开关是「我想要收」，这个是「系统允不允许发」。两者都为真才有横幅。
 *
 * 旧的 osascript 方案根本没有这个概念，于是出现过「三个开关都勾着、一条横幅都不来」：
 * `display notification` 的投递身份是 Script Editor 而不是本应用，本机从未授权过这个身份，
 * 横幅被静默丢弃，osascript 还退出 0。设置面板于是在说谎而自己不知道。
 */
export type NotifyPermission = "unknown" | "granted" | "denied";

export function useNotifications(
  sessions: SessionLike[],
  /** 「这个 session 正摆在眼前」——见 App.tsx 里 isSessionOnScreen 的注释 */
  isSessionOnScreen: (sessionId: string) => boolean,
  onFocusNotify?: () => void,
) {
  const [notifications, setNotifications] = useState<NotificationRecord[]>(load);
  // 只用未读通知初始化，已读的 session 允许重新触发通知（重启去重）
  const notifiedRef = useRef(new Set<string>(load().filter((n) => !n.isRead).map((n) => n.sessionId)));
  /**
   * 「只闪了窗口、没真的通知过」的 session。
   *
   * 这一层是为了堵住一个单向门：原来你正看着某个 pane 时，事件会被
   * `notifiedRef.add()` **消费掉**，然后只闪一下窗口 —— 既不入通知中心也不发横幅。
   * 于是「你瞥了一眼、没处理、走开了」这个再普通不过的情况下，铃铛永远不会亮：
   * session 还在 waiting，但去重集合里已经有它了。
   *
   * 拆成两个集合之后，抑制不再消费事件：看着的时候只记进 flashedRef（作用仅限于
   * 防止每次 poll 都闪一遍），等它不在眼前了，poll 发现 notifiedRef 里没有它，
   * 该亮的铃铛照样亮。两个集合都在 session 离开 waiting 时清空。
   */
  const flashedRef = useRef(new Set<string>());
  // 第一次 poll 时只入中心不弹 macOS 通知（避免重启 flood）
  const isFirstPollRef = useRef(true);
  const isOnScreenRef = useRef(isSessionOnScreen);
  isOnScreenRef.current = isSessionOnScreen;
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

  const [permission, setPermissionState] = useState<NotifyPermission>("unknown");
  const permissionRef = useRef<NotifyPermission>("unknown");
  function setPermission(p: NotifyPermission) {
    permissionRef.current = p;
    setPermissionState(p);
  }

  // 启动时读一次真实授权状态，好让设置面板显示的是系统的答案而不是我们的猜测
  useEffect(() => {
    isPermissionGranted()
      .then((g) => setPermission(g ? "granted" : "denied"))
      .catch(() => setPermission("unknown"));
  }, []);

  /** 弹出系统授权框；返回是否拿到授权。设置面板的「请求授权」按钮用 */
  async function requestSystemPermission(): Promise<boolean> {
    try {
      const r = await requestPermission();
      setPermission(r === "granted" ? "granted" : "denied");
      return r === "granted";
    } catch {
      setPermission("unknown");
      return false;
    }
  }

  /**
   * 系统通知的唯一出口。原来 4 个调用点各自 `invoke("show_notification")` 并各自
   * 判一遍 systemEnabled，开关判断和发送逻辑散在四处；现在都走这里。
   * 未授权时先请求一次（macOS 首次会弹框），拿不到就记成 denied 并放弃 ——
   * 不再出现「发了但被系统丢掉、调用方以为成功」的情况。
   */
  async function postSystem(title: string, body: string) {
    if (!systemEnabledRef.current) return;
    try {
      if (permissionRef.current !== "granted") {
        if (!(await isPermissionGranted())) {
          if (!(await requestSystemPermission())) return;
        } else {
          setPermission("granted");
        }
      }
      sendNotification({ title, body });
    } catch (e) {
      // 静默失败是这个 bug 能藏这么久的直接原因，留一条 console 便于下次排查
      console.warn("[notify] 系统通知发送失败", e);
    }
  }

  /** 设置面板的「发送测试通知」——不必等真有 session 进入等待态才能验证链路 */
  async function sendTestNotification() {
    if (permissionRef.current !== "granted" && !(await requestSystemPermission())) return false;
    try {
      sendNotification({ title: "makit 通知自检", body: "能看到这条横幅，说明系统通知链路是通的" });
      return true;
    } catch {
      return false;
    }
  }

  // poll-based detection: fires when sessions array changes
  useEffect(() => {
    const isFirstPoll = isFirstPollRef.current;
    isFirstPollRef.current = false;

    const fresh: NotificationRecord[] = [];

    for (const s of sessions) {
      // 离开 waiting、或对应开关被关掉：两个集合一起清，下一次进 waiting 重新算一次
      if (s.status !== "waiting") {
        notifiedRef.current.delete(s.session_id);
        flashedRef.current.delete(s.session_id);
        continue;
      }
      const isUserWait = s.waiting_for === "user";
      const shouldNotify = isUserWait ? notifyUserRef.current : notifyApprovalRef.current;
      if (!shouldNotify) {
        notifiedRef.current.delete(s.session_id);
        flashedRef.current.delete(s.session_id);
        continue;
      }

      // 正摆在眼前：只闪一下窗口，不入中心不发横幅 —— 但**不占用** notifiedRef，
      // 这样等你切走之后它还能正常通知（对标产品 isFocusedPanel 的本意是"别重复告知"，
      // 不是"这次就算了"）
      if (isOnScreenRef.current(s.session_id)) {
        if (!flashedRef.current.has(s.session_id)) {
          flashedRef.current.add(s.session_id);
          if (!isFirstPoll) onFocusNotifyRef.current?.();
        }
        continue;
      }

      if (notifiedRef.current.has(s.session_id)) continue;
      notifiedRef.current.add(s.session_id);

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
      if (!isFirstPoll) {
        postSystem(`Claude ${waitLabel}`, rec.sessionName + (label ? ` — ${label}` : ""));
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

        // 正摆在眼前：同 poll 那条分支 —— 只闪窗口，不占 notifiedRef
        if (isOnScreenRef.current(sessionId)) {
          if (!flashedRef.current.has(sessionId)) {
            flashedRef.current.add(sessionId);
            onFocusNotifyRef.current?.();
          }
          return;
        }

        // 和 poll 探测器共用同一个去重集合
        if (notifiedRef.current.has(sessionId)) return;
        notifiedRef.current.add(sessionId);

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

        postSystem(`Claude ${waitLabel}`, sessionName + (label ? ` — ${label}` : ""));
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
    permission,
    requestSystemPermission,
    sendTestNotification,
  };
}
