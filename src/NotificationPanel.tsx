import { useEffect, useRef, useState } from "react";
import type { NotificationRecord } from "./useNotifications";

function relativeTime(ts: number): string {
  const diff = Math.floor((Date.now() - ts) / 1000);
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86400)}d`;
}

function statusLabel(waitingFor?: string): string {
  if (waitingFor === "user") return "等待回答";
  return "等待审批";
}

type Props = {
  notifications: NotificationRecord[];
  unreadCount: number;
  onMarkRead: (id: string) => void;
  onMarkAllRead: () => void;
  onClearOne: (id: string) => void;
  onClearAll: () => void;
  onNavigate: (sessionId: string) => void;
  onClose: () => void;
  anchorLeft?: number;
};

export function NotificationPanel({
  notifications,
  unreadCount,
  onMarkRead,
  onMarkAllRead,
  onClearOne,
  onClearAll,
  onNavigate,
  onClose,
  anchorLeft,
}: Props) {
  const panelRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [activeIdx, setActiveIdx] = useState(notifications.length > 0 ? 0 : -1);

  // 打开时 focus 面板，允许键盘操作
  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  // notifications 变化时保持 activeIdx 合法
  useEffect(() => {
    setActiveIdx((prev) => (notifications.length > 0 ? Math.min(prev < 0 ? 0 : prev, notifications.length - 1) : -1));
  }, [notifications.length]);

  // 选中项滚动到可见
  useEffect(() => {
    if (activeIdx < 0) return;
    const el = listRef.current?.querySelector(`[data-notif-idx="${activeIdx}"]`) as HTMLElement | null;
    el?.scrollIntoView({ block: "nearest" });
  }, [activeIdx]);

  useEffect(() => {
    function handler(e: MouseEvent) {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose]);

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") { onClose(); return; }
    if (notifications.length === 0) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => (i + 1) % notifications.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => (i - 1 + notifications.length) % notifications.length);
    } else if (e.key === "Enter" && activeIdx >= 0) {
      e.preventDefault();
      const n = notifications[activeIdx];
      if (n) { onMarkRead(n.id); onNavigate(n.sessionId); onClose(); }
    }
  }

  useEffect(() => {
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose, notifications, activeIdx]);

  return (
    <div className="notif-drawer" ref={panelRef} tabIndex={-1} style={anchorLeft !== undefined ? { left: anchorLeft } : undefined}>
      <div className="notif-drawer-header">
        <span className="notif-drawer-title">通知</span>
        <span className="notif-drawer-shortcut">⌘I</span>
        <div className="notif-drawer-actions">
          {unreadCount > 0 && (
            <button className="notif-drawer-btn" onClick={onMarkAllRead}>
              全部已读
            </button>
          )}
          {notifications.length > 0 && (
            <button className="notif-drawer-btn notif-drawer-btn-dim" onClick={onClearAll}>
              全部清除
            </button>
          )}
        </div>
      </div>

      <div className="notif-drawer-list" ref={listRef}>
        {notifications.length === 0 ? (
          <div className="notif-drawer-empty">
            <svg
              className="notif-drawer-empty-icon"
              width="36"
              height="36"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
              <path d="M13.73 21a2 2 0 0 1-3.46 0" />
              <line x1="1" y1="1" x2="23" y2="23" />
            </svg>
            <div className="notif-drawer-empty-title">暂无通知</div>
            <div className="notif-drawer-empty-hint">桌面通知将在此处显示。</div>
          </div>
        ) : (
          notifications.map((n, idx) => (
            <div
              key={n.id}
              data-notif-idx={idx}
              className={"notif-row" + (n.isRead ? " notif-row-read" : "") + (idx === activeIdx ? " notif-row-active" : "")}
              onMouseEnter={() => setActiveIdx(idx)}
              onClick={() => {
                onMarkRead(n.id);
                onNavigate(n.sessionId);
                onClose();
              }}
            >
              <div className="notif-row-dot-wrap">
                {!n.isRead && <span className="notif-row-dot" />}
              </div>
              <div className="notif-row-body">
                <div className="notif-row-top">
                  <span className="notif-row-name">{n.sessionName}</span>
                  <span className="notif-row-time">{relativeTime(n.timestamp)}</span>
                </div>
                <div className="notif-row-sub">
                  {n.projectLabel && (
                    <span className="notif-row-project">{n.projectLabel}</span>
                  )}
                  <span className="notif-row-status">{statusLabel(n.waitingFor)}</span>
                </div>
              </div>
              <button
                className="notif-row-clear"
                title="移除"
                onClick={(e) => {
                  e.stopPropagation();
                  onClearOne(n.id);
                }}
              >
                ×
              </button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
