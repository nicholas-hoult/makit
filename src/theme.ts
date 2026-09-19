// 主题系统 —— 源色推导
//
// 一个主题只提供 iTerm2 .itermcolors 等量的信息：bg / fg / (selection) / ANSI 16 色。
// 其余全部 UI 变量（bg-soft/hover/active、border、fg-muted/subtle、语义色、阴影、滚动条）
// 由 bg 亮度和 fg/ansi 推导得出。
//
// 这样浅色主题的正确性是结构性的：tint() 的方向按 bg 亮度自动翻转，
// 不存在"某个主题忘了覆盖某个变量"这种手写主题块特有的问题。

import { getCurrentWindow } from "@tauri-apps/api/window";

import { parseItermcolors } from "./itermcolors";

export type ThemeSource = {
  id: string;
  name: string;
  bg: string;
  fg: string;
  /** ANSI 16 色，顺序：black red green yellow blue magenta cyan white，再 bright 同序 */
  ansi: readonly string[];
  /** 终端选中区背景；缺省时按亮度用半透黑/白 */
  selection?: string;
};

const ANSI_NAMES = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
  "bright-black", "bright-red", "bright-green", "bright-yellow",
  "bright-blue", "bright-magenta", "bright-cyan", "bright-white",
] as const;

// ---------- 颜色工具 ----------

function clamp255(n: number): number {
  return Math.max(0, Math.min(255, Math.round(n)));
}
function hexToRgb(hex: string): [number, number, number] {
  const m = hex.match(/^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  if (!m) return [0, 0, 0];
  return [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)];
}
function rgbHex(r: number, g: number, b: number): string {
  const h = (n: number) => clamp255(n).toString(16).padStart(2, "0");
  return `#${h(r)}${h(g)}${h(b)}`;
}
/** a → b 线性插值，t=0 得 a，t=1 得 b */
function mix(a: string, b: string, t: number): string {
  const [ar, ag, ab] = hexToRgb(a);
  const [br, bg, bb] = hexToRgb(b);
  return rgbHex(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t);
}
/** amount > 0 加白，< 0 加黑 */
function lighten(hex: string, amount: number): string {
  const [r, g, b] = hexToRgb(hex);
  return rgbHex(r + 255 * amount, g + 255 * amount, b + 255 * amount);
}
/** WCAG 相对亮度 */
function relLum(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(a: string, b: string): number {
  const la = relLum(a), lb = relLum(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
function isLight(hex: string): boolean {
  return relLum(hex) > 0.35;
}
/** 把 color 往远离 bg 的方向推，直到对比度达标；保证徽章/状态文字在任何主题下都可读 */
function readable(color: string, bg: string, min = 4.0): string {
  if (contrast(color, bg) >= min) return color;
  const towardDark = isLight(bg);
  let c = color;
  for (let i = 0; i < 25; i++) {
    c = lighten(c, towardDark ? -0.04 : 0.04);
    if (contrast(c, bg) >= min) return c;
  }
  return towardDark ? "#000000" : "#ffffff";
}
/** 反过来：色块本身是背景，把它往 base 收，直到 fg 在它上面还读得清 */
function readableOn(color: string, fg: string, base: string, min = 3.2): string {
  let c = color;
  for (let i = 0; i < 20 && contrast(fg, c) < min; i++) c = mix(c, base, 0.12);
  return c;
}

// ---------- 推导 ----------

export function deriveVars(src: ThemeSource): Record<string, string> {
  const { bg, fg } = src;
  const light = isLight(bg);
  const vars: Record<string, string> = { "--bg": bg, "--fg": fg };
  ANSI_NAMES.forEach((n, i) => { vars[`--ansi-${n}`] = src.ansi[i] || fg; });

  // 深色底：bg 加白往上叠层级；浅色底：bg 加黑
  const tint = (amt: number) => lighten(bg, light ? -amt : amt);
  // 浅色底上 bright 系普遍过亮，优先取普通色；深色底反之
  const pick = (base: string, bright: string) =>
    light ? vars[`--ansi-${base}`] : vars[`--ansi-${bright}`] || vars[`--ansi-${base}`];

  const accent = readable(pick("blue", "bright-blue"), bg);

  // 一条边界要么靠"面"的色阶、要么靠"线"。优先用面：色阶抬高只会让 chrome 层次更清楚，
  // 而线一多就立刻变吵。所以 bg-soft（chrome 面：侧栏 / tab bar / pane 标题条）给到对 bg
  // 约 1.2:1（IDEA 侧栏对编辑区量级），让大部分分界不必再画线。
  vars["--bg-soft"] = tint(0.055);
  vars["--bg-hover"] = tint(0.105);
  vars["--bg-active"] = mix(bg, accent, light ? 0.18 : 0.26);
  // 两级边框，用途分工严格：
  // --border 是面板"内部"的细分隔线（搜索框、header/footer、列表分组），要安静，25 套主题实测 1.6~1.9:1；
  // --border-strong 只给结构级边界（侧栏↔工作区）和浮层轮廓，实测 2.5~3.2:1。
  // 浅色底整体亮，同样的 tint 量看起来更弱，所以给更大的量。
  vars["--border"] = tint(light ? 0.195 : 0.155);
  vars["--border-strong"] = tint(light ? 0.36 : 0.30);
  // 次级/三级文字：先按比例混向 bg，再用对比度下限兜住——
  // 低对比度色板（Solarized Light、Tokyo Night Light 等）单靠比例混合会混到看不见。
  vars["--fg-muted"] = readable(mix(fg, bg, 0.42), bg, 3.0);
  vars["--fg-subtle"] = readable(mix(fg, bg, 0.62), bg, 2.2);
  vars["--accent"] = accent;
  vars["--accent-text"] = accent;
  vars["--accent-fg"] =
    contrast("#ffffff", accent) >= contrast("#000000", accent) ? "#ffffff" : "#000000";
  // 次强调色（归档等"另一类"状态），取 magenta 以和 accent 的蓝拉开
  vars["--accent-alt"] = readable(pick("magenta", "bright-magenta"), bg);
  vars["--danger"] = readable(pick("red", "bright-red"), bg);
  vars["--success"] = readable(pick("green", "bright-green"), bg);
  vars["--warning"] = readable(pick("yellow", "bright-yellow"), bg);
  vars["--info"] = readable(pick("cyan", "bright-cyan"), bg);
  // ⌘F 终端内搜索的命中底色。必须是**不透明** #RRGGBB —— xterm search addon 的
  // decoration 只认这个格式（见 ISearchDecorationOptions 注释），rgba() 会被丢掉。
  //
  // 用 warning（黄）系而不是 accent（蓝）系：命中高亮要和"选中区"（--selection-bg）
  // 分得开，而 accent 已经被 active pane 边框、active tab 顶条、focused session 用掉了。
  // 荧光笔黄也是浏览器 / 编辑器的通用编码，不用学。
  //
  // 「所有命中」和「当前命中」靠**色相**区分，不靠明暗 —— 这是 Chrome / iTerm2 的做法
  // （全部黄、当前橙）。明暗区分在这里行不通：decoration 画在字的**后面**，前景色仍是 fg，
  // 所以两级底色都必须被 readableOn 按同一个可读性下限往 bg 收。实测 25 套主题里有 5 套
  // （one-dark / solarized-light / everforest-light / ayu-light / tokyo-night-light）
  // 两级会收到对比度 1.01~1.04 —— 肉眼完全同色。色相差不受这个钳制影响。
  //
  // 橙色由 yellow + red 合成而不是新增源色：任何色板都有这两个，"当前命中偏橙"这条
  // 就对 25 套主题一致成立，不需要逐套手填。
  // 两级都描边，不只当前那个。底色的可见度是被 readableOn 钳住的上限 —— 低对比色板
  // （tokyo-night-light / solarized-light）实测底色对 bg 只有 1.4:1，光靠填充根本"不明显"，
  // 而再深就压到正文读不出。饱和的 1px 轮廓不占前景对比度预算，所以可见度交给它。
  const hlAll = readable(vars["--warning"], bg);
  const hlActive = readable(mix(vars["--warning"], vars["--danger"], 0.55), bg);
  vars["--search-match-bg"] = readableOn(mix(bg, hlAll, light ? 0.40 : 0.32), fg, bg);
  vars["--search-match-active-bg"] = readableOn(mix(bg, hlActive, light ? 0.62 : 0.52), fg, bg);
  vars["--search-match-border"] = hlAll;
  vars["--search-match-active-border"] = hlActive;
  vars["--selection-bg"] =
    src.selection || (light ? "rgba(0,0,0,0.16)" : "rgba(255,255,255,0.22)");
  vars["--selection-bg-inactive"] = light ? "rgba(0,0,0,0.09)" : "rgba(255,255,255,0.12)";
  vars["--shadow"] = light ? "rgba(0,0,0,0.13)" : "rgba(0,0,0,0.45)";
  vars["--shadow-strong"] = light ? "rgba(0,0,0,0.18)" : "rgba(0,0,0,0.60)";
  // 模态遮罩：浅色主题下压太黑会很突兀
  vars["--scrim"] = light ? "rgba(0,0,0,0.24)" : "rgba(0,0,0,0.48)";
  // 滚动条不再有自定义色（改用系统 overlay 条），它的深浅由 color-scheme 决定
  vars["color-scheme"] = light ? "light" : "dark";
  return vars;
}

export function isLightTheme(src: ThemeSource): boolean {
  return isLight(src.bg);
}

// ---------- 内置配色（源色取自 iTerm2-Color-Schemes 等开放色板）----------

export const BUILTIN_THEMES: readonly ThemeSource[] = [
  {
    id: "vscode-dark", name: "VS Code Dark", bg: "#1e1e1e", fg: "#d4d4d4",
    ansi: ["#1e1e1e", "#f44747", "#89d185", "#d7ba7d", "#569cd6", "#c586c0", "#4ec9b0", "#d4d4d4",
           "#808080", "#f14c4c", "#73c991", "#e2c08d", "#6cb6ff", "#d2a8ff", "#58d1c9", "#e5e5e5"],
  },
  {
    id: "github-dark", name: "GitHub Dark", bg: "#0d1117", fg: "#c9d1d9",
    ansi: ["#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
           "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc"],
  },
  {
    id: "tokyo-night", name: "Tokyo Night", bg: "#1a1b26", fg: "#c0caf5",
    ansi: ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
           "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"],
  },
  {
    id: "dracula", name: "Dracula", bg: "#282a36", fg: "#f8f8f2",
    ansi: ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
           "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
  },
  {
    id: "solarized-dark", name: "Solarized Dark", bg: "#002b36", fg: "#93a1a1",
    ansi: ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
           "#586e75", "#cb4b16", "#859900", "#b58900", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
  },
  {
    id: "catppuccin-mocha", name: "Catppuccin Mocha", bg: "#1e1e2e", fg: "#cdd6f4",
    ansi: ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
           "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"],
  },
  {
    id: "nord", name: "Nord", bg: "#2e3440", fg: "#d8dee9",
    ansi: ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
           "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
  },
  {
    id: "one-dark", name: "One Dark", bg: "#282c34", fg: "#abb2bf",
    ansi: ["#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
           "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"],
  },
  {
    id: "monokai-pro", name: "Monokai Pro", bg: "#2c292d", fg: "#fcfcfa",
    ansi: ["#403e41", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa",
           "#727072", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa"],
  },
  {
    id: "gruvbox-dark", name: "Gruvbox Dark", bg: "#282828", fg: "#ebdbb2",
    ansi: ["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
           "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"],
  },
  {
    id: "rose-pine", name: "Rosé Pine", bg: "#191724", fg: "#e0def4",
    ansi: ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
           "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"],
  },
  {
    id: "ayu-mirage", name: "Ayu Mirage", bg: "#1f2430", fg: "#cbccc6",
    ansi: ["#191e2a", "#ed8274", "#a6cc70", "#fad07b", "#6dcbfa", "#cfbafa", "#90e1c6", "#c7c7c7",
           "#686868", "#f28779", "#bae67e", "#ffd580", "#73d0ff", "#d4bfff", "#95e6cb", "#ffffff"],
  },
  {
    id: "everforest-dark", name: "Everforest Dark", bg: "#2b3339", fg: "#d3c6aa",
    ansi: ["#4b565c", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
           "#5c6a72", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#e9e8d2"],
  },
  {
    id: "kanagawa", name: "Kanagawa", bg: "#1f1f28", fg: "#dcd7ba",
    ansi: ["#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
           "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba"],
  },
  {
    id: "night-owl", name: "Night Owl", bg: "#011627", fg: "#d6deeb",
    ansi: ["#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff",
           "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff"],
  },
  {
    id: "snazzy", name: "Snazzy", bg: "#282a36", fg: "#eff0eb",
    ansi: ["#282a36", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#f1f1f0",
           "#686868", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#eff0eb"],
  },
  {
    id: "vscode-light", name: "VS Code Light", bg: "#ffffff", fg: "#1f1f1f",
    // green/bright-green 原版 #00bc00/#14ce14 是饱和度 100%/82% 的纯绿，实心背景块（比如
    // diff 新增行）在白底上显得像荧光笔。这里和其它 8 套浅色主题一起，把饱和度统一压到
    // ≤58%（色相/明度不动），压到哪一档见 src/theme.ts 顶部这轮改动的说明。
    ansi: ["#000000", "#cd3131", "#279527", "#949800", "#0451a5", "#bc05bc", "#0598bc", "#555555",
           "#666666", "#cd3131", "#2fb32f", "#b5ba00", "#0451a5", "#bc05bc", "#0598bc", "#a5a5a5"],
  },
  {
    id: "github-light", name: "GitHub Light", bg: "#ffffff", fg: "#1f2328",
    ansi: ["#24292f", "#cf222e", "#185c2c", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781",
           "#57606a", "#a40e26", "#20793a", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f"],
  },
  {
    id: "solarized-light", name: "Solarized Light", bg: "#fdf6e3", fg: "#586e75",
    // bright-green（#586e75）不动：Solarized 的设计本来就是故意让 bright-green 复用
    // base01（灰蓝），不是一处需要"调绿"的漏改。
    ansi: ["#073642", "#dc322f", "#6d7920", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
           "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
  },
  {
    id: "catppuccin-latte", name: "Catppuccin Latte", bg: "#eff1f5", fg: "#4c4f69",
    ansi: ["#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
           "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc"],
  },
  {
    id: "ayu-light", name: "Ayu Light", bg: "#fafafa", fg: "#5c6166",
    ansi: ["#000000", "#f07171", "#738d26", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#c7c7c7",
           "#686868", "#f07171", "#738d26", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#ffffff"],
  },
  {
    id: "gruvbox-light", name: "Gruvbox Light", bg: "#fbf1c7", fg: "#3c3836",
    ansi: ["#fbf1c7", "#cc241d", "#8d8c25", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64",
           "#928374", "#9d0006", "#6b671c", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836"],
  },
  {
    id: "one-light", name: "One Light", bg: "#fafafa", fg: "#383a42",
    ansi: ["#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#a0a1a7",
           "#4f525d", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"],
  },
  {
    id: "everforest-light", name: "Everforest Light", bg: "#fdf6e3", fg: "#5c6a72",
    ansi: ["#5c6a72", "#f85552", "#748022", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#dfddc8",
           "#829181", "#f85552", "#748022", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#f0eed9"],
  },
  {
    id: "tokyo-night-light", name: "Tokyo Night Light", bg: "#d5d6db", fg: "#565a6e",
    ansi: ["#0f0f14", "#8c4351", "#485e30", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b58",
           "#9699a3", "#8c4351", "#485e30", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b58"],
  },
];

// ---------- 导入的 .itermcolors ----------

const IMPORTED_KEY = "makit-imported-themes";
const THEME_KEY = "makit-theme";
const STYLE_ID = "theme-vars";

function readImported(): ThemeSource[] {
  try {
    const raw = localStorage.getItem(IMPORTED_KEY);
    if (!raw) return [];
    const arr = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr.filter((t) => t && t.id && t.bg && t.fg && Array.isArray(t.ansi));
  } catch {
    return [];
  }
}

function writeImported(list: ThemeSource[]) {
  localStorage.setItem(IMPORTED_KEY, JSON.stringify(list));
}

export function listImportedThemes(): ThemeSource[] {
  return readImported();
}

/** 解析 .itermcolors 并存为一个可选主题；同名覆盖 */
export function importItermcolors(fileName: string, xml: string): ThemeSource {
  const parsed = parseItermcolors(xml);
  const name = fileName.replace(/\.itermcolors$/i, "").trim() || "导入的配色";
  const theme: ThemeSource = {
    id: `imported:${name}`,
    name,
    bg: parsed.bg,
    fg: parsed.fg,
    ansi: parsed.ansi,
    selection: parsed.selection,
  };
  const list = readImported().filter((t) => t.id !== theme.id);
  list.push(theme);
  writeImported(list);
  return theme;
}

export function removeImportedTheme(id: string) {
  writeImported(readImported().filter((t) => t.id !== id));
}

// ---------- 应用 ----------

export function getTheme(id: string): ThemeSource {
  return (
    BUILTIN_THEMES.find((t) => t.id === id) ||
    readImported().find((t) => t.id === id) ||
    BUILTIN_THEMES[0]
  );
}

export function savedThemeId(): string {
  return localStorage.getItem(THEME_KEY) || BUILTIN_THEMES[0].id;
}

/**
 * 把主题变量注入 <style id="theme-vars">，并设置 data-theme。
 * 在 render 前同步调用可避免首屏闪烁。
 */
export function applyTheme(id: string) {
  const src = getTheme(id);
  const vars = deriveVars(src);
  const body = Object.entries(vars).map(([k, v]) => `  ${k}: ${v};`).join("\n");
  // JSON.stringify 产出带引号且已转义的字符串，正好是 CSS 属性选择器需要的形式
  const css = `:root[data-theme=${JSON.stringify(src.id)}] {\n${body}\n}`;

  let style = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = document.createElement("style");
    style.id = STYLE_ID;
    document.head.appendChild(style);
  }
  style.textContent = css;
  document.documentElement.dataset.theme = src.id;
  localStorage.setItem(THEME_KEY, src.id);
  syncNativeBackground(src.bg);
}

/**
 * 把原生窗口底色也设成主题的 bg。
 *
 * CSS 只能管 webview **画出来**的那一层。窗口尺寸变化时（双击标题栏放大、拖边框）
 * 原生层先按新尺寸铺底、webview 重绘滞后一帧，露出来的就是原生底色 —— 不设的话
 * 那是系统默认白，于是深色主题下双击放大会闪一下白屏。
 *
 * `tauri.conf.json` 里的 `backgroundColor` 只兜得住首帧（那时 JS 还没跑，读不到
 * localStorage 里存的主题），主题切换后的同步必须在这里做。
 */
function syncNativeBackground(bg: string) {
  try {
    void getCurrentWindow().setBackgroundColor(bg).catch(() => {});
  } catch {
    // 非 Tauri 环境（uipreview.html 在普通浏览器里跑）没有窗口，忽略
  }
}
