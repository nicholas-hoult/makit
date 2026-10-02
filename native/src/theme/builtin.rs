//! 内置 21 套主题的源色（#248）：深色 11 套（Embark 在最前，是默认）+ 浅色 10 套。
//!
//! 来源：<https://github.com/mbadolato/iTerm2-Color-Schemes>（MIT）里的 `.itermcolors`，经 `itermcolors::parse_itermcolors`
//! 解析（P3 已换算成 sRGB）得到的十六进制。合集本身是 MIT，但**单个主题的原作者授权没有逐个核实**（站上标 SPDX 不明），发布前的授权梳理见 #255。
//! 挑选标准：六种主色色相正确且不发灰、12 个常用文字色对比度 ≥ 3:1、正文 ≥ 7:1（`tests::every_builtin_theme_is_readable` 守着）。
//! 切换前的旧 25 套留在 `legacy_fixture.rs`（仅测试用，给 Tauri 版推导结果对照）。

use super::derive::ThemeSource;

/// (id, 名字, bg, fg, ANSI 16 色)
const RAW: [(&str, &str, &str, &str, [&str; 16]); 21] = [
    ("embark", "Embark", "#1e1c31", "#eeffff", ["#1e1c31", "#f0719b", "#a1efd3", "#ffe9aa", "#57c7ff", "#c792ea", "#87dfeb", "#f8f8f2", "#585273", "#f02e6e", "#2ce592", "#ffb378", "#1da0e2", "#a742ea", "#63f2f1", "#a6b3cc"]),
    ("catppuccin-mocha", "Catppuccin Mocha", "#1e1e2e", "#cdd6f4", ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f7aec2", "#c2ecbf", "#fcd682", "#aeccfc", "#f398da", "#b1eae1", "#a6adc8"]),
    ("catppuccin-macchiato", "Catppuccin Macchiato", "#24273a", "#cad3f5", ["#494d64", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca", "#b8c0e0", "#5b6078", "#f2a7b2", "#bde3b0", "#f4e3c1", "#adc5f7", "#f493da", "#a5ded6", "#a5adcb"]),
    ("tokyonight", "TokyoNight", "#1a1b26", "#c0caf5", ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"]),
    ("github-dark-default", "GitHub Dark Default", "#0d1117", "#e6edf3", ["#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4", "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#ffffff"]),
    ("solarized-osaka-night", "Solarized Osaka Night", "#1a1b26", "#c0caf5", ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"]),
    ("ayu-mirage", "Ayu Mirage", "#1f2430", "#cccac2", ["#171b24", "#ed8274", "#87d96c", "#facc6e", "#6dcbfa", "#dabafa", "#90e1c6", "#c7c7c7", "#686868", "#f28779", "#d5ff80", "#ffd173", "#73d0ff", "#dfbfff", "#95e6cb", "#ffffff"]),
    ("carbonfox", "Carbonfox", "#161616", "#f2f4f8", ["#282828", "#ee5396", "#25be6a", "#08bdba", "#78a9ff", "#be95ff", "#33b1ff", "#dfdfe0", "#484848", "#f16da6", "#46c880", "#2dc7c4", "#8cb6ff", "#c8a5ff", "#52bdff", "#e4e4e5"]),
    ("moonfly", "Moonfly", "#080808", "#bdbdbd", ["#323437", "#ff5454", "#8cc85f", "#e3c78a", "#80a0ff", "#cf87e8", "#79dac8", "#c6c6c6", "#949494", "#ff5189", "#36c692", "#c6c684", "#74b2ff", "#ae81ff", "#85dc85", "#e4e4e4"]),
    ("everblush", "Everblush", "#141b1e", "#dadada", ["#232a2d", "#e57474", "#8ccf7e", "#e5c76b", "#67b0e8", "#c47fd5", "#6cbfbf", "#b3b9b8", "#2d3437", "#ef7e7e", "#96d988", "#f4d67a", "#71baf2", "#ce89df", "#67cbe7", "#bdc3c2"]),
    ("modus-vivendi-tinted", "Modus Vivendi Tinted", "#0d0e1c", "#ffffff", ["#0d0e1c", "#ff5f59", "#44bc44", "#d0bc00", "#2fafff", "#feacd0", "#00d3d0", "#a6a6a6", "#595959", "#ff6b55", "#00c06f", "#fec43f", "#79a8ff", "#b6a0ff", "#6ae4b9", "#ffffff"]),
    ("github-light-default", "GitHub Light Default", "#ffffff", "#1f2328", ["#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781", "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f"]),
    ("github-light-high-contrast", "GitHub Light High Contrast", "#ffffff", "#0e1116", ["#0e1116", "#a0111f", "#024c1a", "#3f2200", "#0349b4", "#622cbc", "#1b7c83", "#66707b", "#4b535d", "#86061d", "#055d20", "#4e2c00", "#1168e3", "#844ae7", "#3192aa", "#88929d"]),
    ("modus-operandi-tinted", "Modus Operandi Tinted", "#fbf7f0", "#000000", ["#000000", "#a60000", "#006800", "#6f5500", "#0031a9", "#721045", "#005e8b", "#a6a6a6", "#595959", "#972500", "#00663f", "#884900", "#3548cf", "#531ab6", "#005f5f", "#595959"]),
    ("dayfox", "Dayfox", "#f6f2ee", "#3d2b5a", ["#352c24", "#a5222f", "#396847", "#ac5402", "#2848a9", "#6e33ce", "#287980", "#f2e9e1", "#534c45", "#b3434e", "#577f63", "#b86e28", "#4863b6", "#8452d5", "#488d93", "#f4ece6"]),
    ("zenbones-light", "Zenbones Light", "#f0edec", "#2c363c", ["#f0edec", "#a8334c", "#4f6c31", "#944927", "#286486", "#88507d", "#3b8992", "#2c363c", "#cfc1ba", "#94253e", "#3f5a22", "#803d1c", "#1d5573", "#7b3b70", "#2b747c", "#4f5e68"]),
    ("porcelain", "Porcelain", "#fbfbfd", "#2a2e37", ["#2a2e37", "#c60018", "#157424", "#855700", "#004cc8", "#761bc3", "#006873", "#5a6170", "#828896", "#d60027", "#1b842d", "#af2700", "#005bdb", "#862ad2", "#007f8f", "#1b1e25"]),
    ("letterpress", "Letterpress", "#f3ecdf", "#3a352c", ["#3a352c", "#b0202a", "#506b24", "#8a5e0a", "#2a5c8a", "#6e4b2e", "#176b5f", "#aeaba4", "#8e886f", "#b0202a", "#506b24", "#8a4fa0", "#2a5c8a", "#176b5f", "#176b5f", "#3a352c"]),
    ("moonwalk", "Moonwalk", "#e4e2e0", "#061f4a", ["#080808", "#7a0047", "#145b0e", "#733a11", "#002fa7", "#5400a8", "#00566b", "#9c958d", "#494440", "#af1608", "#4c6129", "#7a5000", "#0d50c5", "#952197", "#006092", "#ffffff"]),
    ("patina-light", "Patina Light", "#ddd7c4", "#2e2a24", ["#2e2a24", "#9b3b3b", "#34644c", "#6e5817", "#2f626f", "#7a4f47", "#2e6260", "#5a5248", "#625a51", "#833333", "#2d5839", "#634e14", "#2c5660", "#6d463e", "#285654", "#393a34"]),
    ("nvim-light", "Nvim Light", "#e0e2ea", "#14161b", ["#07080d", "#590008", "#005523", "#6b5300", "#004c73", "#470045", "#007373", "#eef1f8", "#4f5258", "#590008", "#005523", "#6b5300", "#004c73", "#470045", "#007373", "#eef1f8"]),
];

pub const DEFAULT_THEME_ID: &str = "embark";

pub fn builtin_themes() -> Vec<ThemeSource> {
    RAW.iter()
        .map(|(id, name, bg, fg, ansi)| ThemeSource {
            id: id.to_string(),
            name: name.to_string(),
            bg: bg.to_string(),
            fg: fg.to_string(),
            ansi: ansi.iter().map(|s| s.to_string()).collect(),
            selection: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::derive::is_light;

    fn lum(hex: &str) -> f64 {
        let c = |i: usize| {
            let v = u8::from_str_radix(&hex[i..i + 2], 16).unwrap() as f64 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * c(1) + 0.7152 * c(3) + 0.0722 * c(5)
    }

    /// WCAG 对比度
    fn contrast(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// 为什么要测（#248）：内置主题要在白底 / 深底上都读得清。以前的浅色主题里黄、橙、浅绿在白底上只有 1.7:1，
    /// 终端输出的警告、diff 绿色行几乎看不见。这条规则让以后加主题时直接被挡住，不用靠肉眼挑
    #[test]
    fn every_builtin_theme_is_readable() {
        let mut bad = Vec::new();
        for t in builtin_themes() {
            let body = contrast(&t.fg, &t.bg);
            // 红绿黄蓝紫青 + 亮色版：终端里最常拿来显示文字的 12 个色
            let worst = [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14].iter().map(|&i| contrast(&t.ansi[i], &t.bg)).fold(f64::MAX, f64::min);
            if body < 7.0 || worst < 3.0 {
                bad.push(format!("{}：正文 {body:.1}:1、最差的文字色 {worst:.1}:1", t.name));
            }
        }
        assert!(bad.is_empty(), "这些主题看不清（正文要 ≥ 7:1、常用文字色要 ≥ 3:1）：\n{}", bad.join("\n"));
    }

    #[test]
    fn ids_are_unique_and_the_first_one_is_the_default() {
        let themes = builtin_themes();
        let mut ids: Vec<_> = themes.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), themes.len(), "id 不能重复");
        assert_eq!(themes[0].id, DEFAULT_THEME_ID, "第一个就是默认主题");
        assert!(!is_light(&themes[0].bg), "默认主题是深色（Embark）");
    }

    #[test]
    fn dark_themes_come_first_then_light() {
        let themes = builtin_themes();
        let (dark, light) = (themes.iter().filter(|t| !is_light(&t.bg)).count(), themes.iter().filter(|t| is_light(&t.bg)).count());
        assert_eq!((dark, light), (11, 10), "深色 11（Embark + 10）、浅色 10");
    }
}
