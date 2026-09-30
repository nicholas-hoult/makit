//! 主题推导（照搬 `src/theme.ts` 的 `deriveVars`）：一个主题只给源色（bg / fg / ANSI 16 色 /
//! 可选 selection），其余 UI 色全部按规则推导出来。
//!
//! 为什么单独测：两套界面（WebView 版、GPUI 版）要对同一个主题 id 画出同样的颜色。
//! 推导里有对比度迭代（`readable` / `readable_on`），公式或舍入差一点，某几套主题的
//! 次级文字 / 徽章颜色就会和 WebView 版对不上 —— 肉眼只会觉得「有点灰」，说不出哪里错。
//! 所以测试直接拿 TS 版对 25 套主题的推导结果（`tests/fixtures/theme-derived.json`）逐项比。
//!
//! 输出是 `--变量名 → 颜色字符串` 的表，键名和 TS 版一致（`--bg-soft`、`--fg-muted`……），
//! 值是 `#rrggbb` 或 `rgba(r,g,b,a)`；GPUI 侧用 `parse_color` 转成 RGBA。

use std::collections::BTreeMap;

/// 一套主题的源色。字段和 TS 版 `ThemeSource` 一一对应（serde 名也一样，导入的主题
/// 从 localStorage `makit-imported-themes` 原样反序列化）。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ThemeSource {
    pub id: String,
    pub name: String,
    pub bg: String,
    pub fg: String,
    /// ANSI 16 色，顺序：black red green yellow blue magenta cyan white，再 bright 同序；缺的用 fg 补
    pub ansi: Vec<String>,
    /// 终端选中区背景；缺省时按亮度用半透黑/白
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
}

pub const ANSI_NAMES: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    "bright-black", "bright-red", "bright-green", "bright-yellow",
    "bright-blue", "bright-magenta", "bright-cyan", "bright-white",
];

// ---------- 颜色工具（逐个对应 theme.ts）----------

/// JS 的 `Math.round`：`floor(x + 0.5)`。和 Rust 的 `round`（远离 0）只在负的 .5 上不同，
/// 这里照 JS 写，免得哪天出现负值时两边差 1。
fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

fn clamp255(n: f64) -> u8 {
    js_round(n).clamp(0.0, 255.0) as u8
}

/// `#rrggbb` → (r,g,b)；格式不对时得 (0,0,0)，和 TS 一样
pub fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return (0, 0, 0);
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
    (p(0), p(2), p(4))
}

fn rgb_hex(r: f64, g: f64, b: f64) -> String {
    format!("#{:02x}{:02x}{:02x}", clamp255(r), clamp255(g), clamp255(b))
}

/// a → b 线性插值，t=0 得 a，t=1 得 b
pub fn mix(a: &str, b: &str, t: f64) -> String {
    let (ar, ag, ab) = hex_to_rgb(a);
    let (br, bg, bb) = hex_to_rgb(b);
    let (ar, ag, ab, br, bg, bb) = (ar as f64, ag as f64, ab as f64, br as f64, bg as f64, bb as f64);
    rgb_hex(ar + (br - ar) * t, ag + (bg - ag) * t, ab + (bb - ab) * t)
}

/// amount > 0 加白，< 0 加黑
pub fn lighten(hex: &str, amount: f64) -> String {
    let (r, g, b) = hex_to_rgb(hex);
    rgb_hex(r as f64 + 255.0 * amount, g as f64 + 255.0 * amount, b as f64 + 255.0 * amount)
}

/// WCAG 相对亮度
pub fn rel_lum(hex: &str) -> f64 {
    let (r, g, b) = hex_to_rgb(hex);
    let lin = |c: u8| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (rel_lum(a), rel_lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// 按 bg 相对亮度 > 0.35 判为浅色
pub fn is_light(hex: &str) -> bool {
    rel_lum(hex) > 0.35
}

/// 把 color 往远离 bg 的方向推，直到对比度达标；保证徽章/状态文字在任何主题下都可读
pub fn readable(color: &str, bg: &str, min: f64) -> String {
    if contrast(color, bg) >= min {
        return color.to_string();
    }
    let toward_dark = is_light(bg);
    let mut c = color.to_string();
    for _ in 0..25 {
        c = lighten(&c, if toward_dark { -0.04 } else { 0.04 });
        if contrast(&c, bg) >= min {
            return c;
        }
    }
    if toward_dark { "#000000".into() } else { "#ffffff".into() }
}

/// 反过来：色块本身是背景，把它往 base 收，直到 fg 在它上面还读得清
pub fn readable_on(color: &str, fg: &str, base: &str, min: f64) -> String {
    let mut c = color.to_string();
    let mut i = 0;
    while i < 20 && contrast(fg, &c) < min {
        c = mix(&c, base, 0.12);
        i += 1;
    }
    c
}

// ---------- 推导 ----------

/// 和 `theme.ts` 的 `deriveVars` 逐行对应，注释里的「为什么」见 TS 原文。
pub fn derive_vars(src: &ThemeSource) -> BTreeMap<String, String> {
    let bg = src.bg.as_str();
    let fg = src.fg.as_str();
    let light = is_light(bg);
    let mut v: BTreeMap<String, String> = BTreeMap::new();
    v.insert("--bg".into(), bg.into());
    v.insert("--fg".into(), fg.into());
    for (i, n) in ANSI_NAMES.iter().enumerate() {
        // TS: `src.ansi[i] || fg` —— 空串也按缺失处理
        let c = src.ansi.get(i).filter(|s| !s.is_empty()).map(|s| s.as_str()).unwrap_or(fg);
        v.insert(format!("--ansi-{n}"), c.into());
    }
    let ansi = |v: &BTreeMap<String, String>, n: &str| v[&format!("--ansi-{n}")].clone();

    // 深色底：bg 加白往上叠层级；浅色底：bg 加黑
    let tint = |amt: f64| lighten(bg, if light { -amt } else { amt });
    // 浅色底上 bright 系普遍过亮，优先取普通色；深色底反之
    let pick = |v: &BTreeMap<String, String>, base: &str, bright: &str| {
        if light { ansi(v, base) } else { ansi(v, bright) }
    };

    let accent = readable(&pick(&v, "blue", "bright-blue"), bg, 4.0);

    v.insert("--bg-soft".into(), tint(0.055));
    v.insert("--bg-hover".into(), tint(0.105));
    v.insert("--bg-active".into(), mix(bg, &accent, if light { 0.18 } else { 0.26 }));
    v.insert("--border".into(), tint(if light { 0.195 } else { 0.155 }));
    v.insert("--border-strong".into(), tint(if light { 0.36 } else { 0.30 }));
    v.insert("--fg-muted".into(), readable(&mix(fg, bg, 0.42), bg, 3.0));
    v.insert("--fg-subtle".into(), readable(&mix(fg, bg, 0.62), bg, 2.2));
    v.insert("--accent".into(), accent.clone());
    v.insert("--accent-text".into(), accent.clone());
    let accent_fg = if contrast("#ffffff", &accent) >= contrast("#000000", &accent) { "#ffffff" } else { "#000000" };
    v.insert("--accent-fg".into(), accent_fg.into());
    let accent_alt = readable(&pick(&v, "magenta", "bright-magenta"), bg, 4.0);
    v.insert("--accent-alt".into(), accent_alt);
    let danger = readable(&pick(&v, "red", "bright-red"), bg, 4.0);
    let success = readable(&pick(&v, "green", "bright-green"), bg, 4.0);
    let warning = readable(&pick(&v, "yellow", "bright-yellow"), bg, 4.0);
    let info = readable(&pick(&v, "cyan", "bright-cyan"), bg, 4.0);
    v.insert("--danger".into(), danger.clone());
    v.insert("--success".into(), success);
    v.insert("--warning".into(), warning.clone());
    v.insert("--info".into(), info);

    let hl_all = readable(&warning, bg, 4.0);
    let hl_active = readable(&mix(&warning, &danger, 0.55), bg, 4.0);
    v.insert("--search-match-bg".into(), readable_on(&mix(bg, &hl_all, if light { 0.40 } else { 0.32 }), fg, bg, 3.2));
    v.insert(
        "--search-match-active-bg".into(),
        readable_on(&mix(bg, &hl_active, if light { 0.62 } else { 0.52 }), fg, bg, 3.2),
    );
    v.insert("--search-match-border".into(), hl_all);
    v.insert("--search-match-active-border".into(), hl_active);
    let selection = src
        .selection
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| if light { "rgba(0,0,0,0.16)".into() } else { "rgba(255,255,255,0.22)".into() });
    v.insert("--selection-bg".into(), selection);
    v.insert(
        "--selection-bg-inactive".into(),
        if light { "rgba(0,0,0,0.09)" } else { "rgba(255,255,255,0.12)" }.into(),
    );
    v.insert("--shadow".into(), if light { "rgba(0,0,0,0.13)" } else { "rgba(0,0,0,0.45)" }.into());
    v.insert("--shadow-strong".into(), if light { "rgba(0,0,0,0.18)" } else { "rgba(0,0,0,0.60)" }.into());
    v.insert("--scrim".into(), if light { "rgba(0,0,0,0.24)" } else { "rgba(0,0,0,0.48)" }.into());
    v.insert("color-scheme".into(), if light { "light" } else { "dark" }.into());
    v
}

/// `#rrggbb` / `rgba(r,g,b,a)` → (r,g,b,a)，各分量 0–1。认不出来返回 None。
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            let (r, g, b) = hex_to_rgb(s);
            return Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]);
        }
        return None;
    }
    let inner = s.strip_prefix("rgba(")?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').map(|p| p.trim().parse::<f32>()).collect::<Result<_, _>>().ok()?;
    if parts.len() != 4 {
        return None;
    }
    Some([parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, parts[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtin::builtin_themes;

    /// TS 版（`src/theme.ts` 的 `deriveVars`）对 25 套内置主题 + 2 个边界输入的推导结果
    const FIXTURE: &str = include_str!("../../tests/fixtures/theme-derived.json");

    fn fixture() -> BTreeMap<String, BTreeMap<String, String>> {
        serde_json::from_str(FIXTURE).expect("fixture 不是合法 JSON")
    }

    #[test]
    fn every_builtin_theme_derives_exactly_like_the_ts_version() {
        let want = fixture();
        let themes = builtin_themes();
        assert_eq!(themes.len(), 25, "内置 25 套");
        let mut diffs = Vec::new();
        for t in &themes {
            let expected = want.get(&t.id).unwrap_or_else(|| panic!("fixture 里没有 {}", t.id));
            let got = derive_vars(t);
            for (k, ev) in expected {
                match got.get(k) {
                    Some(gv) if gv == ev => {}
                    other => diffs.push(format!("{} {k}: TS={ev} Rust={other:?}", t.id)),
                }
            }
            assert_eq!(got.len(), expected.len(), "{} 的变量个数和 TS 版不同", t.id);
        }
        assert!(diffs.is_empty(), "和 TS 版推导不一致：\n{}", diffs.join("\n"));
    }

    #[test]
    fn edge_inputs_match_ts_too() {
        let want = fixture();
        let vs = builtin_themes()[0].ansi.clone();
        let with_sel = ThemeSource {
            id: "x".into(),
            name: "x".into(),
            bg: "#101820".into(),
            fg: "#e0e0e0".into(),
            ansi: vs,
            selection: Some("#334455".into()),
        };
        assert_eq!(&derive_vars(&with_sel), &want["x-with-selection"]);
        let short = ThemeSource {
            id: "y".into(),
            name: "y".into(),
            bg: "#f0f0f0".into(),
            fg: "#202020".into(),
            ansi: vec!["#000000".into(), "#aa0000".into()],
            selection: None,
        };
        assert_eq!(&derive_vars(&short), &want["x-short-ansi"], "ANSI 缺的用 fg 补");
    }

    #[test]
    fn light_dark_threshold_and_color_parsing() {
        assert!(is_light("#ffffff") && !is_light("#1e1e1e"));
        assert_eq!(parse_color("#ff0080"), Some([1.0, 0.0, 128.0 / 255.0, 1.0]));
        assert_eq!(parse_color("rgba(255,255,255,0.22)"), Some([1.0, 1.0, 1.0, 0.22]));
        assert_eq!(parse_color("light"), None);
        assert_eq!(hex_to_rgb("zz"), (0, 0, 0), "格式不对和 TS 一样当黑色");
    }
}
