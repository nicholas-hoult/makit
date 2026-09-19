/**
 * 路径里的 home 缩写成 `~`（#192）。home 由调用方运行时取（`@tauri-apps/api/path` 的 `homeDir()`），
 * 这里只做纯字符串判断：必须按路径段匹配，`/Users/me` 不能吃掉 `/Users/meg/x`。
 */
export function shortenHome(path: string, home: string): string {
  const h = home.replace(/\/+$/, "");
  if (!h) return path;
  if (path === h) return "~";
  return path.startsWith(h + "/") ? "~" + path.slice(h.length) : path;
}
