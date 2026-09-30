//! 浮层用到的图标路径（`native/assets/icons/*.svg`，从 Tauri 版的内联 SVG 原样抽出，文件头注释写着来源）。
//! 文件本身登记在 `crate::assets::FILES`，那里是全应用唯一的 AssetSource。
//! 用法：`svg().path(icons::SEARCH).size(px(16.)).text_color(颜色)`（svg 当遮罩，颜色取 text_color）。

pub const SEARCH: &str = "icons/search.svg";
pub const FILTER: &str = "icons/filter.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource;

    #[test]
    fn every_overlay_icon_is_registered() {
        for p in [SEARCH, FILTER, CHEVRON_RIGHT] {
            assert!(crate::assets::Assets.load(p).unwrap().is_some(), "{p}");
        }
    }
}
