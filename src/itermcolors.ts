// 解析 iTerm2 .itermcolors 文件（XML plist）→ CSS 变量 map
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

const KEY_MAP: Record<string, string> = {
  "Background Color": "--bg",
  "Foreground Color": "--fg",
  "Selection Color": "--selection-bg-base",
  "Ansi 0 Color": "--ansi-black",
  "Ansi 1 Color": "--ansi-red",
  "Ansi 2 Color": "--ansi-green",
  "Ansi 3 Color": "--ansi-yellow",
  "Ansi 4 Color": "--ansi-blue",
  "Ansi 5 Color": "--ansi-magenta",
  "Ansi 6 Color": "--ansi-cyan",
  "Ansi 7 Color": "--ansi-white",
  "Ansi 8 Color": "--ansi-bright-black",
  "Ansi 9 Color": "--ansi-bright-red",
  "Ansi 10 Color": "--ansi-bright-green",
  "Ansi 11 Color": "--ansi-bright-yellow",
  "Ansi 12 Color": "--ansi-bright-blue",
  "Ansi 13 Color": "--ansi-bright-magenta",
  "Ansi 14 Color": "--ansi-bright-cyan",
  "Ansi 15 Color": "--ansi-bright-white",
};

function clamp01(n: number): number { return Math.max(0, Math.min(1, n)); }
function toHex2(n: number): string { return Math.round(clamp01(n) * 255).toString(16).padStart(2, "0"); }
function rgbToHex(r: number, g: number, b: number): string { return `#${toHex2(r)}${toHex2(g)}${toHex2(b)}`; }

function hexToRgb(hex: string): [number, number, number] {
  const m = hex.match(/^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  if (!m) return [0, 0, 0];
  return [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)];
}
function clamp255(n: number): number { return Math.max(0, Math.min(255, Math.round(n))); }
function rgbHex(r: number, g: number, b: number): string {
  return `#${clamp255(r).toString(16).padStart(2, "0")}${clamp255(g).toString(16).padStart(2, "0")}${clamp255(b).toString(16).padStart(2, "0")}`;
}
function mix(a: string, b: string, t: number): string {
  const [ar, ag, ab] = hexToRgb(a);
  const [br, bg, bb] = hexToRgb(b);
  return rgbHex(ar * (1 - t) + br * t, ag * (1 - t) + ag * t, ab * (1 - t) + bb * t);
}
function lighten(hex: string, amount: number): string {
  const [r, g, b] = hexToRgb(hex);
  return rgbHex(r + 255 * amount, g + 255 * amount, b + 255 * amount);
}
function isLight(hex: string): boolean {
  const [r, g, b] = hexToRgb(hex);
  // YIQ 亮度
  return (r * 299 + g * 587 + b * 114) / 1000 > 140;
}

export type ImportedTheme = {
  name: string;
  vars: Record<string, string>;
};

export function parseItermcolors(xml: string): Record<string, string> {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  if (doc.querySelector("parsererror")) throw new Error("XML 解析失败");
  const root = doc.querySelector("plist > dict");
  if (!root) throw new Error("plist 根节点缺失");

  const out: Record<string, string> = {};
  const kids = Array.from(root.children);
  for (let i = 0; i < kids.length; i++) {
    const node = kids[i];
    if (node.tagName !== "key") continue;
    const keyName = node.textContent?.trim() || "";
    const cssVar = KEY_MAP[keyName];
    const valueNode = kids[i + 1];
    if (!cssVar || !valueNode || valueNode.tagName !== "dict") continue;

    let r = 0, g = 0, b = 0;
    const sub = Array.from(valueNode.children);
    for (let j = 0; j < sub.length; j++) {
      const k = sub[j];
      if (k.tagName !== "key") continue;
      const v = sub[j + 1];
      if (!v) continue;
      const num = parseFloat(v.textContent || "0");
      const subKey = k.textContent?.trim() || "";
      if (subKey === "Red Component") r = num;
      else if (subKey === "Green Component") g = num;
      else if (subKey === "Blue Component") b = num;
    }
    out[cssVar] = rgbToHex(r, g, b);
  }
  if (!out["--bg"] || !out["--fg"]) throw new Error("缺少 Background/Foreground Color");
  return out;
}

// 从 iTerm 颜色推算 UI 配套色（bg-soft/hover/active/border/accent/danger 等）
// .itermcolors 只定义终端 16 色 + bg/fg/selection；UI 色按 bg/fg 衍生。
export function deriveUIColors(termVars: Record<string, string>): Record<string, string> {
  const bg = termVars["--bg"]!;
  const fg = termVars["--fg"]!;
  const light = isLight(bg);
  // 深色：bg 加白；浅色：bg 加黑
  const tint = (amt: number) => light ? lighten(bg, -amt) : lighten(bg, amt);
  const accent = termVars["--ansi-bright-blue"] || termVars["--ansi-blue"] || "#58a6ff";
  const ui: Record<string, string> = {
    ...termVars,
    "--bg-soft": tint(0.03),
    "--bg-hover": tint(0.08),
    "--bg-active": tint(0.18),
    "--border": tint(0.12),
    "--fg-muted": mix(fg, bg, 0.45),
    "--accent": accent,
    "--accent-text": accent,
    "--accent-fg": light ? "#ffffff" : bg,
    "--danger": termVars["--ansi-red"] || "#f48771",
    "--success": termVars["--ansi-green"] || "#3fb950",
    "--warning": termVars["--ansi-yellow"] || "#d29922",
    "--info": termVars["--ansi-cyan"] || "#58a6ff",
    "--selection-bg": light ? "rgba(0,0,0,0.18)" : "rgba(255,255,255,0.22)",
    "--selection-bg-inactive": light ? "rgba(0,0,0,0.10)" : "rgba(255,255,255,0.12)",
  };
  if (light) {
    // 让浅色主题告诉浏览器
    ui["color-scheme"] = "light";
  }
  return ui;
}

// 把导入主题写成 <style id="imported-theme"> 规则，命中 :root[data-theme="imported"]
const STYLE_ID = "imported-theme-style";
const STORAGE_KEY = "ccs-imported-theme";

export function injectImportedTheme(theme: ImportedTheme) {
  const ui = deriveUIColors(theme.vars);
  const css = `:root[data-theme="imported"] { ${Object.entries(ui)
    .map(([k, v]) => k.startsWith("--") ? `${k}: ${v};` : `${k}: ${v};`)
    .join(" ")} }`;
  let style = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = document.createElement("style");
    style.id = STYLE_ID;
    document.head.appendChild(style);
  }
  style.textContent = css;
  localStorage.setItem(STORAGE_KEY, JSON.stringify(theme));
}

export function loadImportedTheme(): ImportedTheme | null {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  try {
    const t = JSON.parse(raw);
    if (t && typeof t === "object" && t.vars && t.name) return t as ImportedTheme;
  } catch {}
  return null;
}

export function clearImportedTheme() {
  localStorage.removeItem(STORAGE_KEY);
  document.getElementById(STYLE_ID)?.remove();
}
