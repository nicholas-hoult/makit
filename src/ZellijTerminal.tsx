import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Props = {
  sessionName: string;
  token: string;
  port?: number;
};

// Zellij Web 全屏 iframe：一个 iframe = 一个 zellij session 的完整视图
// zellij 自己管 tab/pane/split/move/resize，我们只做 session 管理 + iframe 容器
// 优势：PTY 由 zellij 管理 → 永不死；pane 操作原生支持 → 不需要 DnD hack
export function ZellijWorkspace({ sessionName, token, port = 8082 }: Props) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [authenticated, setAuthenticated] = useState(false);

  // 首次加载时自动认证（POST /command/login 带 token）
  useEffect(() => {
    const baseUrl = `http://127.0.0.1:${port}`;
    fetch(`${baseUrl}/command/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ auth_token: token, remember_me: true }),
      credentials: "include",
    })
      .then((res) => {
        if (res.ok) setAuthenticated(true);
        else console.error("Zellij auth failed:", res.status);
      })
      .catch((e) => console.error("Zellij auth error:", e));
  }, [token, port]);

  const zellijUrl = `http://127.0.0.1:${port}/${sessionName}`;

  if (!authenticated) {
    return (
      <div style={{ width: "100%", height: "100%", display: "flex", alignItems: "center", justifyContent: "center", color: "var(--fg-muted)" }}>
        连接 Zellij...
      </div>
    );
  }

  return (
    <iframe
      ref={iframeRef}
      src={zellijUrl}
      style={{
        width: "100%",
        height: "100%",
        border: "none",
        background: "var(--bg)",
      }}
      allow="clipboard-read; clipboard-write"
    />
  );
}

// 通过 Rust 后端调 zellij action 在当前 session 里创建新 pane
export async function zellijNewPane(opts: {
  direction?: "right" | "down";
  cwd?: string;
  command?: string;
}) {
  return invoke("zellij_action", {
    action: "new-pane",
    args: [
      ...(opts.direction ? ["--direction", opts.direction] : []),
      ...(opts.cwd ? ["--cwd", opts.cwd] : []),
      ...(opts.command ? ["--", ...opts.command.split(" ")] : []),
    ],
  });
}

