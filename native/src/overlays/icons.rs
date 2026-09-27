//! 界面图标（`native/assets/icons/*.svg`，从 Tauri 版的内联 SVG 原样抽出，文件头注释写着来源）。
//! 编进二进制（include_bytes），启动时 `Application::new().with_assets(Assets)` 装上；
//! 用法：`svg().path(icons::SEARCH).size(px(16.)).text_color(颜色)`（svg 当遮罩，颜色取 text_color）。
//!
//! 别的包要加图标：把 .svg 放进 `native/assets/icons/`，在下面 `FILES` 里加一行。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub const SEARCH: &str = "icons/search.svg";
pub const FILTER: &str = "icons/filter.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";

const FILES: &[(&str, &[u8])] = &[
    (SEARCH, include_bytes!("../../assets/icons/search.svg")),
    (FILTER, include_bytes!("../../assets/icons/filter.svg")),
    (CHEVRON_RIGHT, include_bytes!("../../assets/icons/chevron-right.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses_as_svg() {
        for (p, b) in FILES {
            let s = std::str::from_utf8(b).unwrap();
            assert!(s.contains("<svg") && s.contains("</svg>"), "{p}");
            assert!(Assets.load(p).unwrap().is_some());
        }
        assert!(Assets.load("icons/没有.svg").unwrap().is_none());
    }
}
