//! 测试专用：切换到精选主题（#248）之前的 25 套内置主题的源色。
//!
//! 为什么留着：`derive.rs` 的测试要拿 Tauri 版 `deriveVars` 对这 25 套的推导结果（`tests/fixtures/theme-derived.json`）
//! 逐项对，证明 Rust 的推导算法和 TS 版一致。内置列表换了，算法的这份对照仍然有用，输入就从这里取。

use super::derive::ThemeSource;

const LEGACY_RAW: [(&str, &str, &str, &str, [&str; 16]); 25] = [
    ("vscode-dark", "VS Code Dark", "#1e1e1e", "#d4d4d4", ["#1e1e1e", "#f44747", "#89d185", "#d7ba7d", "#569cd6", "#c586c0", "#4ec9b0", "#d4d4d4", "#808080", "#f14c4c", "#73c991", "#e2c08d", "#6cb6ff", "#d2a8ff", "#58d1c9", "#e5e5e5"]),
    ("github-dark", "GitHub Dark", "#0d1117", "#c9d1d9", ["#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4", "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc"]),
    ("tokyo-night", "Tokyo Night", "#1a1b26", "#c0caf5", ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"]),
    ("dracula", "Dracula", "#282a36", "#f8f8f2", ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"]),
    ("solarized-dark", "Solarized Dark", "#002b36", "#93a1a1", ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#586e75", "#cb4b16", "#859900", "#b58900", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"]),
    ("catppuccin-mocha", "Catppuccin Mocha", "#1e1e2e", "#cdd6f4", ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"]),
    ("nord", "Nord", "#2e3440", "#d8dee9", ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"]),
    ("one-dark", "One Dark", "#282c34", "#abb2bf", ["#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf", "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"]),
    ("monokai-pro", "Monokai Pro", "#2c292d", "#fcfcfa", ["#403e41", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa", "#727072", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa"]),
    ("gruvbox-dark", "Gruvbox Dark", "#282828", "#ebdbb2", ["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"]),
    ("rose-pine", "Rosé Pine", "#191724", "#e0def4", ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4", "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"]),
    ("ayu-mirage", "Ayu Mirage", "#1f2430", "#cbccc6", ["#191e2a", "#ed8274", "#a6cc70", "#fad07b", "#6dcbfa", "#cfbafa", "#90e1c6", "#c7c7c7", "#686868", "#f28779", "#bae67e", "#ffd580", "#73d0ff", "#d4bfff", "#95e6cb", "#ffffff"]),
    ("everforest-dark", "Everforest Dark", "#2b3339", "#d3c6aa", ["#4b565c", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa", "#5c6a72", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#e9e8d2"]),
    ("kanagawa", "Kanagawa", "#1f1f28", "#dcd7ba", ["#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093", "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba"]),
    ("night-owl", "Night Owl", "#011627", "#d6deeb", ["#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff", "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff"]),
    ("snazzy", "Snazzy", "#282a36", "#eff0eb", ["#282a36", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#f1f1f0", "#686868", "#ff5c57", "#5af78e", "#f3f99d", "#57c7ff", "#ff6ac1", "#9aedfe", "#eff0eb"]),
    ("vscode-light", "VS Code Light", "#ffffff", "#1f1f1f", ["#000000", "#cd3131", "#279527", "#949800", "#0451a5", "#bc05bc", "#0598bc", "#555555", "#666666", "#cd3131", "#2fb32f", "#b5ba00", "#0451a5", "#bc05bc", "#0598bc", "#a5a5a5"]),
    ("github-light", "GitHub Light", "#ffffff", "#1f2328", ["#24292f", "#cf222e", "#185c2c", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781", "#57606a", "#a40e26", "#20793a", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f"]),
    ("solarized-light", "Solarized Light", "#fdf6e3", "#586e75", ["#073642", "#dc322f", "#6d7920", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"]),
    ("catppuccin-latte", "Catppuccin Latte", "#eff1f5", "#4c4f69", ["#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be", "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc"]),
    ("ayu-light", "Ayu Light", "#fafafa", "#5c6166", ["#000000", "#f07171", "#738d26", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#c7c7c7", "#686868", "#f07171", "#738d26", "#f2ae49", "#399ee6", "#a37acc", "#4cbf99", "#ffffff"]),
    ("gruvbox-light", "Gruvbox Light", "#fbf1c7", "#3c3836", ["#fbf1c7", "#cc241d", "#8d8c25", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64", "#928374", "#9d0006", "#6b671c", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836"]),
    ("one-light", "One Light", "#fafafa", "#383a42", ["#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#a0a1a7", "#4f525d", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff"]),
    ("everforest-light", "Everforest Light", "#fdf6e3", "#5c6a72", ["#5c6a72", "#f85552", "#748022", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#dfddc8", "#829181", "#f85552", "#748022", "#dfa000", "#3a94c5", "#df69ba", "#35a77c", "#f0eed9"]),
    ("tokyo-night-light", "Tokyo Night Light", "#d5d6db", "#565a6e", ["#0f0f14", "#8c4351", "#485e30", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b58", "#9699a3", "#8c4351", "#485e30", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b58"]),
];


pub fn legacy_themes() -> Vec<ThemeSource> {
    LEGACY_RAW
        .iter()
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
