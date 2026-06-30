// 解析 iTerm2 .itermcolors 文件（XML plist）→ 源色
// 文件结构：
//   <plist version="1.0"><dict>
//     <key>Background Color</key><dict>
//       <key>Red Component</key><real>0.1</real>
//       <key>Green Component</key><real>0.2</real>
//       <key>Blue Component</key><real>0.3</real>
//     </dict>
//     <key>Ansi 0 Color</key>...
//   </dict></plist>
// key/value 是兄弟节点，按顺序配对。
//
// 只负责解析；UI 配套色的推导在 theme.ts。

/** ANSI 0..15 在 plist 里的 key 顺序 */
const ANSI_KEYS = Array.from({ length: 16 }, (_, i) => `Ansi ${i} Color`);

export type ParsedItermColors = {
  bg: string;
  fg: string;
  selection?: string;
  /** 16 色，顺序同 ANSI 0..15 */
  ansi: string[];
};

function clamp01(n: number): number { return Math.max(0, Math.min(1, n)); }
function toHex2(n: number): string { return Math.round(clamp01(n) * 255).toString(16).padStart(2, "0"); }
function rgbToHex(r: number, g: number, b: number): string { return `#${toHex2(r)}${toHex2(g)}${toHex2(b)}`; }

/** 读一个 <dict> 里的 Red/Green/Blue Component → hex */
function readColorDict(dict: Element): string {
  let r = 0, g = 0, b = 0;
  const kids = Array.from(dict.children);
  for (let i = 0; i < kids.length; i++) {
    const k = kids[i];
    if (k.tagName !== "key") continue;
    const v = kids[i + 1];
    if (!v) continue;
    const num = parseFloat(v.textContent || "0");
    switch (k.textContent?.trim()) {
      case "Red Component": r = num; break;
      case "Green Component": g = num; break;
      case "Blue Component": b = num; break;
    }
  }
  return rgbToHex(r, g, b);
}

export function parseItermcolors(xml: string): ParsedItermColors {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  if (doc.querySelector("parsererror")) throw new Error("XML 解析失败");
  const root = doc.querySelector("plist > dict");
  if (!root) throw new Error("plist 根节点缺失");

  const colors: Record<string, string> = {};
  const kids = Array.from(root.children);
  for (let i = 0; i < kids.length; i++) {
    const node = kids[i];
    if (node.tagName !== "key") continue;
    const valueNode = kids[i + 1];
    if (!valueNode || valueNode.tagName !== "dict") continue;
    colors[node.textContent?.trim() || ""] = readColorDict(valueNode);
  }

  const bg = colors["Background Color"];
  const fg = colors["Foreground Color"];
  if (!bg || !fg) throw new Error("缺少 Background/Foreground Color");

  return {
    bg,
    fg,
    selection: colors["Selection Color"],
    ansi: ANSI_KEYS.map((k) => colors[k] || fg),
  };
}
