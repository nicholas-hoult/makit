/**
 * 工具 logo 从 data URL 转成 Blob（#219）。
 *
 * 为什么单独测：侧栏每一行都有一个 logo <img>。以前 src 直接是后端给的 base64 data URL
 * （15KB 的 .ico → ~20KB 字符串），250 行就是往 DOM 里塞 5MB 属性、解码 250 次 ——
 * WKWebView 实测光建 DOM 就 0.3–0.5s（展开 250 会话的项目卡 1.2s 的一部分）。
 * 改成转一次 Blob → 短的 blob: URL。转错了在 UI 上是 logo 变成裂图，而且没人会想到去查这里。
 */
import { dataUrlToBlob } from "../src/logoUrl.ts";

let n = 0;
function eq(name: string, got: unknown, want: unknown) {
  n++;
  const g = JSON.stringify(got), w = JSON.stringify(want);
  if (g !== w) { console.error(`✗ ${name}\n  got:  ${g}\n  want: ${w}`); process.exit(1); }
}

const bytes = new Uint8Array([0, 0, 1, 0, 255, 128, 7]);
const b64 = Buffer.from(bytes).toString("base64");

const blob = dataUrlToBlob(`data:image/x-icon;base64,${b64}`)!;
eq("MIME 取自 data URL", blob.type, "image/x-icon");
eq("字节原样还原（含 0 和 >127 的字节）", [...new Uint8Array(await blob.arrayBuffer())], [...bytes]);
eq("png 也行", dataUrlToBlob(`data:image/png;base64,${b64}`)!.type, "image/png");
eq("不是 base64 data URL → null（调用方退回原字符串）", dataUrlToBlob("https://example.com/a.png"), null);
eq("空串 → null", dataUrlToBlob(""), null);

console.log(`✓ logo data URL → Blob 全部通过（${n} 项）`);
