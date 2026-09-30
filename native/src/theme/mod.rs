//! 主题：`Theme` 全局（GPUI Global）。
//!
//! 取色：`cx.theme().bg_soft` / `cx.theme().ansi[4]` …（`use crate::theme::ActiveTheme;`）。
//! 字段名对应 TS 版的 CSS 变量（`--bg-soft` → `bg_soft`）；没列成字段的变量用 `theme.var("--xxx")`。
//! 切主题：`theme::set_theme(id, cx)`（会刷新所有窗口；持久化由调用方写 `prefs.theme.id`）。
//!
//! 纯逻辑：`derive`（推导规则，和 TS 版逐项对齐测试）、`builtin`（25 套源色）、`itermcolors`（导入）。

pub mod builtin;
pub mod derive;
pub mod itermcolors;

use std::collections::BTreeMap;

use gpui::{App, Global, Hsla, Rgba};

use derive::{derive_vars, parse_color, ThemeSource};

#[derive(Clone, Debug)]
pub struct Theme {
    pub source: ThemeSource,
    pub light: bool,
    pub bg: Hsla,
    pub fg: Hsla,
    pub bg_soft: Hsla,
    pub bg_hover: Hsla,
    pub bg_active: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub fg_muted: Hsla,
    pub fg_subtle: Hsla,
    pub accent: Hsla,
    pub accent_fg: Hsla,
    pub accent_alt: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub selection_bg: Hsla,
    pub selection_bg_inactive: Hsla,
    pub search_match_bg: Hsla,
    pub search_match_active_bg: Hsla,
    pub shadow: Hsla,
    pub scrim: Hsla,
    /// ANSI 16 色（终端调色板用）
    pub ansi: [Hsla; 16],
    /// 全部推导结果（`--变量名` → 颜色字符串）
    pub vars: BTreeMap<String, String>,
}

impl Global for Theme {}

pub fn to_hsla(s: &str) -> Hsla {
    let [r, g, b, a] = parse_color(s).unwrap_or([1.0, 0.0, 1.0, 1.0]); // 认不出来画成品红，一眼能看到
    Rgba { r, g, b, a }.into()
}

impl Theme {
    pub fn from_source(source: ThemeSource) -> Self {
        let vars = derive_vars(&source);
        let c = |k: &str| to_hsla(vars.get(k).map(|s| s.as_str()).unwrap_or(""));
        let ansi = std::array::from_fn(|i| c(&format!("--ansi-{}", derive::ANSI_NAMES[i])));
        Self {
            light: derive::is_light(&source.bg),
            bg: c("--bg"),
            fg: c("--fg"),
            bg_soft: c("--bg-soft"),
            bg_hover: c("--bg-hover"),
            bg_active: c("--bg-active"),
            border: c("--border"),
            border_strong: c("--border-strong"),
            fg_muted: c("--fg-muted"),
            fg_subtle: c("--fg-subtle"),
            accent: c("--accent"),
            accent_fg: c("--accent-fg"),
            accent_alt: c("--accent-alt"),
            danger: c("--danger"),
            success: c("--success"),
            warning: c("--warning"),
            info: c("--info"),
            selection_bg: c("--selection-bg"),
            selection_bg_inactive: c("--selection-bg-inactive"),
            search_match_bg: c("--search-match-bg"),
            search_match_active_bg: c("--search-match-active-bg"),
            shadow: c("--shadow"),
            scrim: c("--scrim"),
            ansi,
            source,
            vars,
        }
    }

    /// 按 id 找主题：内置 → 导入的 → 默认（和 TS 版 getTheme 一样）
    pub fn by_id(id: &str, imported: &[ThemeSource]) -> Self {
        let src = builtin::builtin_themes()
            .into_iter()
            .find(|t| t.id == id)
            .or_else(|| imported.iter().find(|t| t.id == id).cloned())
            .unwrap_or_else(|| builtin::builtin_themes().remove(0));
        Self::from_source(src)
    }

    pub fn var(&self, name: &str) -> Hsla {
        to_hsla(self.vars.get(name).map(|s| s.as_str()).unwrap_or(""))
    }
}

/// `cx.theme()` 语法糖
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// 换主题并重画所有窗口
pub fn set_theme(id: &str, imported: &[ThemeSource], cx: &mut App) {
    cx.set_global(Theme::by_id(id, imported));
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_id_falls_back_to_default_and_imported_is_found() {
        assert_eq!(Theme::by_id("没有这个", &[]).source.id, builtin::DEFAULT_THEME_ID);
        let imp = ThemeSource { id: "imported:x".into(), name: "x".into(), bg: "#ffffff".into(), fg: "#000000".into(), ansi: vec![], selection: None };
        let t = Theme::by_id("imported:x", &[imp]);
        assert!(t.light);
        assert_eq!(t.bg, to_hsla("#ffffff"));
    }
}
