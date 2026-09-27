//! 内置资源（`native/assets/`），编进二进制，GPUI 用 `svg().path("icons/xxx.svg")` 读。
//!
//! 图标是从 Tauri 版的内联 SVG 原样抽出来的（TRD #226「视觉一致」），每个文件开头注释写着来源。
//! GPUI 把 SVG 当蒙版画、颜色取元素的 `text_color`，所以 `currentColor` 的图标照样跟主题走。
//!
//! **加一个图标**：文件放进 `native/assets/icons/`，在 `FILES` 里自己那一段加一行。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! asset {
    ($path:literal) => {
        ($path, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/", $path)) as &[u8])
    };
}

const FILES: &[(&str, &[u8])] = &[
    // ---- C 工作区（ContainerView.tsx / App.tsx 标题栏）----
    asset!("icons/container-new-terminal.svg"),
    asset!("icons/container-split-v.svg"),
    asset!("icons/container-split-h.svg"),
    asset!("icons/container-maximize.svg"),
    asset!("icons/container-restore.svg"),
    asset!("icons/titlebar-sidebar-toggle.svg"),
    asset!("icons/titlebar-bell.svg"),
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
    fn every_icon_loads_and_is_an_svg() {
        for (p, _) in FILES {
            let b = Assets.load(p).unwrap().expect("列在表里就要能读到");
            let s = std::str::from_utf8(&b).unwrap();
            assert!(s.contains("<svg") && s.contains("viewBox=\"0 0 16 16\""), "{p} 不是原来那个 16×16 的 svg");
        }
        assert!(Assets.load("icons/没有.svg").unwrap().is_none());
    }
}
