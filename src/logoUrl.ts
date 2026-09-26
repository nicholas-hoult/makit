// 工具 logo：后端给 base64 data URL，前端转一次 Blob，再用短的 blob: URL（#219）。
// 侧栏每行一个 logo <img>，直接用 ~20KB 的 data URL 时 250 行光建 DOM 就 0.3–0.5s。
// 测试见 scripts/test-logo-url.ts。

/** `data:<mime>;base64,<数据>` → Blob；不是这种格式返回 null（调用方直接用原字符串） */
export function dataUrlToBlob(dataUrl: string): Blob | null {
  const m = /^data:([^;,]+);base64,(.*)$/s.exec(dataUrl);
  if (!m) return null;
  const bin = atob(m[2]);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type: m[1] });
}
